package flagship

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
)

// definitionsSnapshot is an immutable view of the last successful definitions
// fetch. Flag entries stay as raw JSON and are decoded lazily on first use so a
// single malformed flag yields PARSE_ERROR without invalidating the snapshot.
type definitionsSnapshot struct {
	etag  string
	flags map[string]json.RawMessage

	mu    sync.Mutex
	cache map[string]cachedFlag
}

type cachedFlag struct {
	flag evalFlag
	err  error
}

func newDefinitionsSnapshot(etag string, flags map[string]json.RawMessage) *definitionsSnapshot {
	return &definitionsSnapshot{
		etag:  etag,
		flags: flags,
		cache: make(map[string]cachedFlag, len(flags)),
	}
}

func (s *definitionsSnapshot) get(flagKey string) (evalFlag, bool, error) {
	raw, ok := s.flags[flagKey]
	if !ok {
		return evalFlag{}, false, nil
	}

	s.mu.Lock()
	defer s.mu.Unlock()
	if cached, hit := s.cache[flagKey]; hit {
		return cached.flag, true, cached.err
	}

	flag, err := decodeEvalFlag(raw)
	s.cache[flagKey] = cachedFlag{flag: flag, err: err}
	return flag, true, err
}

// definitionsFetchResult is the outcome of a definitions HTTP request.
type definitionsFetchResult struct {
	NotModified bool
	ETag        string
	Flags       map[string]json.RawMessage
}

// fetchDefinitions downloads the app's flag definitions for local evaluation.
//
// Sends If-None-Match when etag is non-empty. A 304 yields NotModified; a 200
// yields the parsed flags map and the new ETag. Reuses the client's retry,
// timeout, and header logic.
func (c *FlagshipClient) fetchDefinitions(ctx context.Context, etag string) (definitionsFetchResult, error) {
	if c.definitionsEndpoint == "" {
		return definitionsFetchResult{}, fmt.Errorf(
			"Flagship: definitions endpoint is not configured. Provide appID+accountID, or an endpoint ending in /evaluate",
		)
	}

	request := evaluationRequest{
		method: http.MethodGet,
		url:    c.definitionsEndpoint,
	}
	return fetchWithRetry(ctx, c, request, etag, parseDefinitionsResponse)
}

func parseDefinitionsResponse(resp *http.Response) (definitionsFetchResult, error) {
	if resp.StatusCode == http.StatusNotModified {
		return definitionsFetchResult{NotModified: true}, nil
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return definitionsFetchResult{}, newError(ErrorCodeNetwork, fmt.Sprintf("failed to read response body: %v", err), resp.StatusCode, err)
	}

	switch {
	case resp.StatusCode == http.StatusNotFound:
		return definitionsFetchResult{}, newError(ErrorCodeFlagNotFound, errorDetail(body, resp.Status), resp.StatusCode, nil)
	case resp.StatusCode == http.StatusBadRequest:
		return definitionsFetchResult{}, newError(ErrorCodeBadRequest, errorDetail(body, resp.Status), resp.StatusCode, nil)
	case resp.StatusCode >= http.StatusBadRequest:
		return definitionsFetchResult{}, newError(ErrorCodeGeneral, fmt.Sprintf("HTTP %d: %s", resp.StatusCode, resp.Status), resp.StatusCode, nil)
	}

	decoder := json.NewDecoder(bytes.NewReader(body))
	decoder.UseNumber()

	var payload struct {
		Flags map[string]json.RawMessage `json:"flags"`
	}
	if err := decoder.Decode(&payload); err != nil {
		return definitionsFetchResult{}, newError(ErrorCodeParse, fmt.Sprintf("invalid definitions JSON response: %v", err), resp.StatusCode, err)
	}
	// Only validate the top-level shape; individual malformed flags surface at evaluation time.
	if payload.Flags == nil {
		return definitionsFetchResult{}, newError(ErrorCodeParse, "Invalid definitions response from Flagship API", resp.StatusCode, nil)
	}

	return definitionsFetchResult{
		ETag:  resp.Header.Get("ETag"),
		Flags: payload.Flags,
	}, nil
}

// resolveDefinitionsEndpoint derives the definitions URL from provider options.
//
//   - With AppID: …/apps/{appId}/definitions
//   - With Endpoint: replace a trailing /evaluate with /definitions
//
// Returns an error when the URL cannot be derived.
func resolveDefinitionsEndpoint(options Options) (string, error) {
	if options.AppID != "" && options.Endpoint != "" {
		return "", fmt.Errorf(`Flagship: provide either "appID" or "endpoint", not both`)
	}

	if options.Endpoint != "" {
		u, err := url.Parse(options.Endpoint)
		if err != nil || u.Scheme == "" || u.Host == "" {
			return "", fmt.Errorf("Flagship: invalid endpoint URL: %s", options.Endpoint)
		}
		path := strings.TrimRight(u.Path, "/")
		if !strings.HasSuffix(path, "/evaluate") {
			return "", fmt.Errorf(
				`Flagship: when LocalEvaluation is enabled with "endpoint", the URL path must end in "/evaluate" so the definitions URL can be derived`,
			)
		}
		u.Path = path[:len(path)-len("/evaluate")] + "/definitions"
		u.RawQuery = ""
		u.Fragment = ""
		return u.String(), nil
	}

	if options.AppID == "" {
		return "", fmt.Errorf(`Flagship: either "appID" or "endpoint" is required`)
	}
	if options.AccountID == "" {
		return "", fmt.Errorf(`Flagship: "accountID" is required when using "appID"`)
	}

	baseURL := options.BaseURL
	if baseURL == "" {
		baseURL = DefaultBaseURL
	}
	baseURL = strings.TrimRight(baseURL, "/")

	resolved := fmt.Sprintf(
		"%s/client/v4/accounts/%s/flagship/apps/%s/definitions",
		baseURL,
		url.PathEscape(options.AccountID),
		url.PathEscape(options.AppID),
	)

	u, err := url.Parse(resolved)
	if err != nil || u.Scheme == "" || u.Host == "" {
		return "", fmt.Errorf("Flagship: resolved definitions endpoint is not a valid URL: %s", resolved)
	}
	return resolved, nil
}

func tryResolveDefinitionsEndpoint(options Options) string {
	resolved, err := resolveDefinitionsEndpoint(options)
	if err != nil {
		return ""
	}
	return resolved
}

// fetchWithRetry issues request with the client's retry/timeout settings and
// parses the response via parse. optionalETag, when non-empty, is sent as
// If-None-Match.
func fetchWithRetry[T any](
	ctx context.Context,
	c *FlagshipClient,
	request evaluationRequest,
	optionalETag string,
	parse func(*http.Response) (T, error),
) (T, error) {
	var zero T
	var lastErr error
	for attempt := 0; attempt <= c.retries; attempt++ {
		result, err := fetchOnce(ctx, c, request, optionalETag, parse)
		if err == nil {
			return result, nil
		}
		lastErr = err
		if !isRetryable(err) || attempt == c.retries {
			return zero, err
		}
		if err := sleepWithContext(ctx, c.retryDelay); err != nil {
			return zero, err
		}
	}
	return zero, lastErr
}

func fetchOnce[T any](
	ctx context.Context,
	c *FlagshipClient,
	request evaluationRequest,
	optionalETag string,
	parse func(*http.Response) (T, error),
) (T, error) {
	var zero T
	requestCtx := ctx
	cancel := func() {}
	if c.timeout > 0 {
		requestCtx, cancel = context.WithTimeout(ctx, c.timeout)
	}
	defer cancel()

	var body io.Reader
	if request.body != nil {
		body = bytes.NewReader(request.body)
	}
	req, err := http.NewRequestWithContext(requestCtx, request.method, request.url, body)
	if err != nil {
		return zero, newError(ErrorCodeGeneral, fmt.Sprintf("failed to build request: %v", err), 0, err)
	}

	headers, err := c.requestHeaders(ctx)
	if err != nil {
		return zero, err
	}
	req.Header = headers
	if request.method == http.MethodPost && req.Header.Get("Content-Type") == "" {
		req.Header.Set("Content-Type", "application/json")
	}
	if optionalETag != "" {
		req.Header.Set("If-None-Match", optionalETag)
	}

	resp, err := c.httpClient.Do(req)
	if err != nil {
		if errors.Is(err, context.DeadlineExceeded) || errors.Is(requestCtx.Err(), context.DeadlineExceeded) {
			return zero, newError(ErrorCodeTimeout, fmt.Sprintf("request timeout after %s", c.timeout), 0, err)
		}
		return zero, newError(ErrorCodeNetwork, fmt.Sprintf("network error: %v", err), 0, err)
	}
	defer resp.Body.Close()

	return parse(resp)
}
