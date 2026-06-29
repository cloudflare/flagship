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
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

// FlagshipClient evaluates flags through the Flagship HTTP API.
type FlagshipClient struct {
	endpoint       string
	httpClient     *http.Client
	timeout        time.Duration
	retries        int
	retryDelay     time.Duration
	authToken      string
	headers        http.Header
	headersFactory HeaderFactory
}

// NewClient constructs a Flagship HTTP client.
func NewClient(options Options) (*FlagshipClient, error) {
	endpoint, err := resolveEndpoint(options)
	if err != nil {
		return nil, err
	}

	timeout := options.Timeout
	if timeout == 0 {
		timeout = defaultTimeout
	}
	if timeout < 0 {
		timeout = 0
	}

	retries := options.Retries
	if options.DisableRetries {
		retries = 0
	} else if retries < 0 {
		retries = 0
	} else if retries == 0 {
		retries = defaultRetries
	}
	if retries > maxRetries {
		retries = maxRetries
	}

	retryDelay := options.RetryDelay
	if retryDelay == 0 {
		retryDelay = defaultRetryDelay
	}
	if retryDelay < 0 {
		retryDelay = 0
	}
	if retryDelay > maxRetryDelay {
		retryDelay = maxRetryDelay
	}

	httpClient := options.HTTPClient
	if httpClient == nil {
		httpClient = http.DefaultClient
	}

	return &FlagshipClient{
		endpoint:       endpoint,
		httpClient:     httpClient,
		timeout:        timeout,
		retries:        retries,
		retryDelay:     retryDelay,
		authToken:      options.AuthToken,
		headers:        cloneHeader(options.Headers),
		headersFactory: options.HeadersFactory,
	}, nil
}

// Endpoint returns the resolved evaluation endpoint.
func (c *FlagshipClient) Endpoint() string {
	return c.endpoint
}

// Retries returns the configured retry attempts after capping.
func (c *FlagshipClient) Retries() int {
	return c.retries
}

// RetryDelay returns the configured retry delay after capping.
func (c *FlagshipClient) RetryDelay() time.Duration {
	return c.retryDelay
}

// Evaluate evaluates a flag using an OpenFeature EvaluationContext.
func (c *FlagshipClient) Evaluate(ctx context.Context, flagKey string, evalCtx openfeature.EvaluationContext) (EvaluationResponse, error) {
	flatCtx := openfeature.FlattenedContext{}
	if targetingKey := evalCtx.TargetingKey(); targetingKey != "" {
		flatCtx[openfeature.TargetingKey] = targetingKey
	}
	for key, value := range evalCtx.Attributes() {
		flatCtx[key] = value
	}
	return c.EvaluateFlat(ctx, flagKey, flatCtx)
}

// EvaluateFlat evaluates a flag using the flattened context passed to
// OpenFeature providers.
func (c *FlagshipClient) EvaluateFlat(ctx context.Context, flagKey string, flatCtx openfeature.FlattenedContext) (EvaluationResponse, error) {
	u, err := c.buildURL(flagKey, flatCtx)
	if err != nil {
		return EvaluationResponse{}, err
	}
	return c.fetchWithRetry(ctx, u)
}

func (c *FlagshipClient) buildURL(flagKey string, flatCtx openfeature.FlattenedContext) (string, error) {
	u, err := url.Parse(c.endpoint)
	if err != nil {
		return "", newError(ErrorCodeGeneral, fmt.Sprintf("invalid endpoint URL: %s", c.endpoint), 0, err)
	}

	params, err := contextToQueryParams(flatCtx)
	if err != nil {
		return "", err
	}
	params.Set("flagKey", flagKey)
	u.RawQuery = params.Encode()
	return u.String(), nil
}

func (c *FlagshipClient) fetchWithRetry(ctx context.Context, requestURL string) (EvaluationResponse, error) {
	var lastErr error
	for attempt := 0; attempt <= c.retries; attempt++ {
		result, err := c.fetch(ctx, requestURL)
		if err == nil {
			return result, nil
		}
		lastErr = err
		if !isRetryable(err) || attempt == c.retries {
			return EvaluationResponse{}, err
		}
		if err := sleepWithContext(ctx, c.retryDelay); err != nil {
			return EvaluationResponse{}, err
		}
	}
	return EvaluationResponse{}, lastErr
}

func (c *FlagshipClient) fetch(ctx context.Context, requestURL string) (EvaluationResponse, error) {
	requestCtx := ctx
	cancel := func() {}
	if c.timeout > 0 {
		requestCtx, cancel = context.WithTimeout(ctx, c.timeout)
	}
	defer cancel()

	req, err := http.NewRequestWithContext(requestCtx, http.MethodGet, requestURL, nil)
	if err != nil {
		return EvaluationResponse{}, newError(ErrorCodeGeneral, fmt.Sprintf("failed to build request: %v", err), 0, err)
	}

	headers, err := c.requestHeaders(ctx)
	if err != nil {
		return EvaluationResponse{}, err
	}
	req.Header = headers

	resp, err := c.httpClient.Do(req)
	if err != nil {
		if errors.Is(err, context.DeadlineExceeded) || errors.Is(requestCtx.Err(), context.DeadlineExceeded) {
			return EvaluationResponse{}, newError(ErrorCodeTimeout, fmt.Sprintf("request timeout after %s", c.timeout), 0, err)
		}
		return EvaluationResponse{}, newError(ErrorCodeNetwork, fmt.Sprintf("network error: %v", err), 0, err)
	}
	defer resp.Body.Close()

	return parseResponse(resp)
}

func (c *FlagshipClient) requestHeaders(ctx context.Context) (http.Header, error) {
	headers := cloneHeader(c.headers)
	if c.authToken != "" && headers.Get("Authorization") == "" {
		headers.Set("Authorization", "Bearer "+c.authToken)
	}

	if c.headersFactory != nil {
		dynamicHeaders, err := c.headersFactory(ctx)
		if err != nil {
			return nil, newError(ErrorCodeGeneral, fmt.Sprintf("headers factory failed: %v", err), 0, err)
		}
		for key, values := range dynamicHeaders {
			headers.Del(key)
			for _, value := range values {
				headers.Add(key, value)
			}
		}
	}

	return headers, nil
}

func parseResponse(resp *http.Response) (EvaluationResponse, error) {
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return EvaluationResponse{}, newError(ErrorCodeNetwork, fmt.Sprintf("failed to read response body: %v", err), resp.StatusCode, err)
	}

	switch {
	case resp.StatusCode == http.StatusNotFound:
		return EvaluationResponse{}, newError(ErrorCodeFlagNotFound, errorDetail(body, resp.Status), resp.StatusCode, nil)
	case resp.StatusCode == http.StatusBadRequest:
		return EvaluationResponse{}, newError(ErrorCodeBadRequest, errorDetail(body, resp.Status), resp.StatusCode, nil)
	case resp.StatusCode >= http.StatusBadRequest:
		return EvaluationResponse{}, newError(ErrorCodeGeneral, fmt.Sprintf("HTTP %d: %s", resp.StatusCode, resp.Status), resp.StatusCode, nil)
	}

	decoder := json.NewDecoder(bytes.NewReader(body))
	decoder.UseNumber()

	var raw map[string]json.RawMessage
	if err := decoder.Decode(&raw); err != nil {
		return EvaluationResponse{}, newError(ErrorCodeParse, fmt.Sprintf("invalid JSON response: %v", err), resp.StatusCode, err)
	}

	flagKeyRaw, ok := raw["flagKey"]
	if !ok {
		return EvaluationResponse{}, newError(ErrorCodeParse, "invalid response format from Flagship API", resp.StatusCode, nil)
	}
	valueRaw, ok := raw["value"]
	if !ok {
		return EvaluationResponse{}, newError(ErrorCodeParse, "invalid response format from Flagship API", resp.StatusCode, nil)
	}

	var flagKey string
	if err := json.Unmarshal(flagKeyRaw, &flagKey); err != nil {
		return EvaluationResponse{}, newError(ErrorCodeParse, "invalid response format from Flagship API", resp.StatusCode, err)
	}

	valueDecoder := json.NewDecoder(bytes.NewReader(valueRaw))
	valueDecoder.UseNumber()
	var value any
	if err := valueDecoder.Decode(&value); err != nil {
		return EvaluationResponse{}, newError(ErrorCodeParse, fmt.Sprintf("invalid response value: %v", err), resp.StatusCode, err)
	}

	var variant string
	if variantRaw, ok := raw["variant"]; ok {
		if err := json.Unmarshal(variantRaw, &variant); err != nil {
			return EvaluationResponse{}, newError(ErrorCodeParse, "invalid response variant", resp.StatusCode, err)
		}
	}

	reason := ReasonDefault
	if reasonRaw, ok := raw["reason"]; ok {
		var reasonString string
		if err := json.Unmarshal(reasonRaw, &reasonString); err != nil {
			return EvaluationResponse{}, newError(ErrorCodeParse, "invalid response reason", resp.StatusCode, err)
		}
		reason = EvaluationReason(reasonString)
	}

	return EvaluationResponse{
		FlagKey: flagKey,
		Value:   value,
		Variant: variant,
		Reason:  reason,
	}, nil
}

func resolveEndpoint(options Options) (string, error) {
	if options.AppID != "" && options.Endpoint != "" {
		return "", fmt.Errorf(`Flagship: provide either "appID" or "endpoint", not both`)
	}
	if options.AppID == "" && options.Endpoint == "" {
		return "", fmt.Errorf(`Flagship: either "appID" or "endpoint" is required`)
	}

	if options.Endpoint != "" {
		u, err := url.Parse(options.Endpoint)
		if err != nil || u.Scheme == "" || u.Host == "" {
			return "", fmt.Errorf("Flagship: invalid endpoint URL: %s", options.Endpoint)
		}
		return options.Endpoint, nil
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
		"%s/client/v4/accounts/%s/flagship/apps/%s/evaluate",
		baseURL,
		url.PathEscape(options.AccountID),
		url.PathEscape(options.AppID),
	)

	u, err := url.Parse(resolved)
	if err != nil || u.Scheme == "" || u.Host == "" {
		return "", fmt.Errorf("Flagship: resolved endpoint is not a valid URL: %s", resolved)
	}
	return resolved, nil
}

func errorDetail(body []byte, fallback string) string {
	if len(body) == 0 {
		return fallback
	}

	var data struct {
		ErrorDetails any `json:"errorDetails"`
	}
	if err := json.Unmarshal(body, &data); err == nil && data.ErrorDetails != nil {
		return fmt.Sprint(data.ErrorDetails)
	}

	text := strings.TrimSpace(string(body))
	if text != "" {
		return text
	}
	return fallback
}

func isRetryable(err error) bool {
	flagshipErr, ok := asFlagshipError(err)
	if !ok {
		return false
	}
	switch flagshipErr.Code {
	case ErrorCodeFlagNotFound, ErrorCodeBadRequest, ErrorCodeParse, ErrorCodeInvalidContext:
		return false
	default:
		return true
	}
}

func sleepWithContext(ctx context.Context, delay time.Duration) error {
	if delay <= 0 {
		return nil
	}
	timer := time.NewTimer(delay)
	defer timer.Stop()
	select {
	case <-timer.C:
		return nil
	case <-ctx.Done():
		return newError(ErrorCodeGeneral, ctx.Err().Error(), 0, ctx.Err())
	}
}

func cloneHeader(headers http.Header) http.Header {
	clone := http.Header{}
	for key, values := range headers {
		copied := make([]string, len(values))
		copy(copied, values)
		clone[key] = copied
	}
	return clone
}
