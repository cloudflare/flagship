package flagship

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/rand"
	"net/http"
	"net/url"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

type eventStreamDescriptor struct {
	Type     string `json:"type"`
	URL      string `json:"url"`
	Endpoint *struct {
		RequestURI string `json:"requestUri"`
		Origin     string `json:"origin"`
	} `json:"endpoint"`
	InactivityDelaySec int `json:"inactivityDelaySec"`
}

type configurationHint struct {
	Type         string          `json:"type"`
	ETag         string          `json:"etag"`
	LastModified json.RawMessage `json:"lastModified"`
}

func streamURL(descriptors []eventStreamDescriptor, base string) (string, error) {
	urls := streamURLs(descriptors, base)
	if len(urls) == 0 {
		return "", errors.New("no usable configuration stream")
	}
	return urls[0], nil
}

// streamURLs resolves every usable SSE descriptor, ignoring unknown types and
// URLs that are neither HTTPS nor loopback HTTP.
func streamURLs(descriptors []eventStreamDescriptor, base string) []string {
	var urls []string
	for _, descriptor := range descriptors {
		if descriptor.Type != "sse" {
			continue
		}
		raw := descriptor.URL
		resolvedBase := base
		if raw == "" && descriptor.Endpoint != nil {
			raw = descriptor.Endpoint.RequestURI
			if descriptor.Endpoint.Origin != "" {
				resolvedBase = descriptor.Endpoint.Origin
			}
		}
		reference, err := url.Parse(raw)
		if err != nil || raw == "" {
			continue
		}
		origin, err := url.Parse(resolvedBase)
		if err != nil {
			continue
		}
		target := origin.ResolveReference(reference)
		if target.User != nil || target.Fragment != "" || target.Hostname() == "" ||
			(target.Scheme != "https" && !(target.Scheme == "http" && (target.Hostname() == "localhost" || target.Hostname() == "127.0.0.1"))) {
			continue
		}
		if address := target.String(); !slices.Contains(urls, address) {
			urls = append(urls, address)
		}
	}
	return urls
}

// streamInactivityDelay returns the first advertised inactivityDelaySec.
func streamInactivityDelay(descriptors []eventStreamDescriptor) time.Duration {
	for _, descriptor := range descriptors {
		if descriptor.InactivityDelaySec >= 1 {
			return time.Duration(descriptor.InactivityDelaySec) * time.Second
		}
	}
	return 0
}

// lastModified validates an event's lastModified as Unix seconds or an RFC 3339
// date-time, returning its query-parameter form or "" when invalid.
func (h configurationHint) lastModified() string {
	if len(h.LastModified) == 0 {
		return ""
	}
	var seconds int64
	if err := json.Unmarshal(h.LastModified, &seconds); err == nil {
		if seconds < 0 {
			return ""
		}
		return strconv.FormatInt(seconds, 10)
	}
	var date string
	if err := json.Unmarshal(h.LastModified, &date); err != nil || len(date) > 256 {
		return ""
	}
	if _, err := time.Parse(time.RFC3339Nano, date); err != nil {
		return ""
	}
	return date
}

func (c *FlagshipClient) bulkEndpoint() (string, error) {
	u, err := url.Parse(c.endpoint)
	if err != nil || !strings.HasSuffix(u.Path, "/evaluate") {
		return "", errors.New("configuration events require an endpoint ending in /evaluate")
	}
	u.Path = strings.TrimSuffix(u.Path, "/evaluate") + "/ofrep/v1/evaluate/flags"
	u.RawQuery = ""
	u.Fragment = ""
	return u.String(), nil
}

func (c *FlagshipClient) fetchBulkVersion(ctx context.Context, hint string, discover bool) (string, []eventStreamDescriptor, error) {
	endpoint, err := c.bulkEndpoint()
	if err != nil {
		return "", nil, err
	}
	u, _ := url.Parse(endpoint)
	if hint != "" {
		query := u.Query()
		query.Set("flagConfigEtag", hint)
		u.RawQuery = query.Encode()
	}
	request := evaluationRequest{method: http.MethodPost, url: u.String(), body: []byte(`{"context":{}}`)}
	if discover {
		request.extraHeaders = http.Header{"Flagship-Event-Stream": []string{"sse"}}
	}
	type bulkResult struct {
		Metadata struct {
			Version string `json:"version"`
		} `json:"metadata"`
		EventStreams []eventStreamDescriptor `json:"eventStreams"`
	}
	parse := func(resp *http.Response) (bulkResult, error) {
		if resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden || resp.StatusCode == http.StatusNotFound {
			return bulkResult{}, newError(ErrorCodeBadRequest, fmt.Sprintf("configuration discovery returned HTTP %d", resp.StatusCode), resp.StatusCode, nil)
		}
		if resp.StatusCode != http.StatusOK {
			return bulkResult{}, newError(ErrorCodeNetwork, fmt.Sprintf("configuration discovery returned HTTP %d", resp.StatusCode), resp.StatusCode, nil)
		}
		var result bulkResult
		if err := json.NewDecoder(io.LimitReader(resp.Body, 16<<20)).Decode(&result); err != nil {
			return bulkResult{}, newError(ErrorCodeParse, "invalid configuration discovery response", resp.StatusCode, err)
		}
		if result.Metadata.Version == "" {
			return bulkResult{}, newError(ErrorCodeParse, "configuration version missing from discovery response", resp.StatusCode, nil)
		}
		return result, nil
	}
	result, err := fetchWithRetry(ctx, c, request, "", parse)
	return result.Metadata.Version, result.EventStreams, err
}

func (c *FlagshipClient) openStream(ctx context.Context, address string) (*http.Response, context.CancelFunc, error) {
	streamCtx, cancel := context.WithCancel(ctx)
	req, err := http.NewRequestWithContext(streamCtx, http.MethodGet, address, nil)
	if err != nil {
		cancel()
		return nil, nil, errors.New("invalid configuration stream URL")
	}
	req.Header.Set("Accept", "text/event-stream")
	client := *c.httpClient
	client.Timeout = 0
	client.CheckRedirect = func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }
	timeout := c.timeout
	if timeout <= 0 {
		timeout = defaultTimeout
	}
	timer := time.AfterFunc(timeout, cancel)
	resp, err := client.Do(req)
	timer.Stop()
	if err != nil {
		cancel()
		return nil, nil, errors.New("configuration stream connection failed")
	}
	if resp.StatusCode != http.StatusOK || !strings.HasPrefix(strings.ToLower(resp.Header.Get("Content-Type")), "text/event-stream") {
		resp.Body.Close()
		cancel()
		if resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden {
			return nil, nil, newError(ErrorCodeBadRequest, "configuration stream authorization failed", resp.StatusCode, nil)
		}
		return nil, nil, fmt.Errorf("configuration stream returned HTTP %d", resp.StatusCode)
	}
	return resp, cancel, nil
}

func readConfigurationEvents(body io.Reader, onHint func(configurationHint) error) error {
	scanner := bufio.NewScanner(body)
	scanner.Buffer(make([]byte, 4<<10), 64<<10)
	var data bytes.Buffer
	eventType := ""
	dispatch := func() error {
		if data.Len() == 0 || (eventType != "" && eventType != "message") {
			data.Reset()
			eventType = ""
			return nil
		}
		var hint configurationHint
		err := json.Unmarshal(data.Bytes(), &hint)
		data.Reset()
		eventType = ""
		if err != nil || hint.Type != "refetchEvaluation" || len(hint.ETag) > 256 {
			return nil
		}
		return onHint(hint)
	}
	for scanner.Scan() {
		line := strings.TrimSuffix(scanner.Text(), "\r")
		if line == "" {
			if err := dispatch(); err != nil {
				return err
			}
		} else if strings.HasPrefix(line, "data:") {
			if data.Len()+len(line) > 64<<10 {
				return errors.New("configuration event too large")
			}
			if data.Len() > 0 {
				data.WriteByte('\n')
			}
			data.WriteString(strings.TrimPrefix(strings.TrimPrefix(line, "data:"), " "))
		} else if strings.HasPrefix(line, "event:") {
			eventType = strings.TrimSpace(strings.TrimPrefix(line, "event:"))
		}
	}
	if err := scanner.Err(); err != nil {
		return errors.New("configuration stream read failed")
	}
	return io.EOF
}

func (p *ServerProvider) configurationEventsLoop(ctx context.Context, epoch uint64) {
	defer p.refreshWG.Done()
	delay := 500 * time.Millisecond
	connectedBefore := false
	for ctx.Err() == nil && p.epoch.Load() == epoch {
		discovery := p.client.definitionsEndpoint
		var descriptors []eventStreamDescriptor
		var discoveredVersion string
		var err error
		if p.localEvaluation {
			var result definitionsFetchResult
			result, err = p.client.fetchDefinitionsHint(ctx, "", "", true)
			descriptors = result.EventStreams
		} else {
			discoveredVersion, descriptors, err = p.client.fetchBulkVersion(ctx, "", true)
			discovery, _ = p.client.bulkEndpoint()
		}
		var address string
		if err == nil {
			address, err = streamURL(descriptors, discovery)
		}
		if err == nil {
			var response *http.Response
			var cancel context.CancelFunc
			response, cancel, err = p.client.openStream(ctx, address)
			if err == nil {
				if !p.localEvaluation && !connectedBefore {
					p.snapshotMu.Lock()
					p.remoteVersion = discoveredVersion
					p.snapshotMu.Unlock()
				}
				if p.refreshFromHint(ctx, epoch, "", connectedBefore) {
					connectedBefore = true
					delay = 500 * time.Millisecond
					err = readConfigurationEvents(response.Body, func(hint configurationHint) error {
						if !p.refreshFromHint(ctx, epoch, hint.ETag, true) {
							return errors.New("configuration refresh failed")
						}
						return nil
					})
				}
				response.Body.Close()
				cancel()
			}
		}
		if flagshipErr, ok := asFlagshipError(err); ok &&
			(flagshipErr.StatusCode == http.StatusUnauthorized || flagshipErr.StatusCode == http.StatusForbidden || flagshipErr.StatusCode == http.StatusNotFound) {
			if p.localEvaluation {
				p.snapshotMu.Lock()
				if p.epoch.Load() == epoch {
					p.snapshot.Store(nil)
					p.etag.Store("")
					p.stale.Store(true)
					if p.terminal.CompareAndSwap(false, true) {
						p.emit(openfeature.ProviderError, openfeature.ProviderEventDetails{
							Message: "Configuration stream unavailable", ErrorCode: openfeature.ProviderFatalCode,
						})
					}
				}
				p.snapshotMu.Unlock()
			} else if p.cache != nil {
				p.cache.clear()
			}
		}
		if ctx.Err() != nil || p.epoch.Load() != epoch {
			return
		}
		if (p.localEvaluation || p.cache != nil) && p.stale.CompareAndSwap(false, true) {
			p.emit(openfeature.ProviderStale, openfeature.ProviderEventDetails{Message: "Configuration stream interrupted"})
		}
		if p.logging {
			p.logger.WarnContext(ctx, "Flagship configuration stream interrupted")
		}
		wait := delay + time.Duration(rand.Int63n(int64(delay)/2+1))
		if sleepWithContext(ctx, wait) != nil {
			return
		}
		delay = min(30*time.Second, delay*2)
	}
}

func (p *ServerProvider) refreshFromHint(ctx context.Context, epoch uint64, hint string, notification bool) bool {
	for attempt := 0; attempt < 5 && ctx.Err() == nil && p.epoch.Load() == epoch; attempt++ {
		var err error
		if p.localEvaluation {
			err = p.refreshSnapshot(ctx, false, epoch, hint, true, notification)
		} else {
			var version string
			version, _, err = p.client.fetchBulkVersion(ctx, hint, false)
			if err == nil && hint != "" && version != hint {
				err = errors.New("configuration version has not propagated")
			}
			if err == nil && p.epoch.Load() == epoch {
				p.snapshotMu.Lock()
				if p.epoch.Load() == epoch {
					if p.cache != nil {
						p.cache.clear()
					}
					if notification && version != p.remoteVersion {
						p.emit(openfeature.ProviderConfigChange, openfeature.ProviderEventDetails{})
					}
					p.remoteVersion = version
					if p.stale.CompareAndSwap(true, false) {
						p.emit(openfeature.ProviderReady, openfeature.ProviderEventDetails{})
					}
				}
				p.snapshotMu.Unlock()
			}
		}
		if err == nil {
			return true
		}
		if flagshipErr, ok := asFlagshipError(err); ok &&
			(flagshipErr.StatusCode == http.StatusUnauthorized || flagshipErr.StatusCode == http.StatusForbidden || flagshipErr.StatusCode == http.StatusNotFound) {
			if p.cache != nil {
				p.cache.clear()
			}
			return false
		}
		if sleepWithContext(ctx, time.Second<<attempt) != nil {
			return false
		}
	}
	return false
}
