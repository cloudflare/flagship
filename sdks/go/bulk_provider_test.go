package flagship

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

const bulkPath = "/v1/acct/apps/app/ofrep/v1/evaluate/flags"

type bulkServer struct {
	*httptest.Server
	mu       sync.Mutex
	requests []*http.Request
	bodies   []map[string]any
	handle   func(w http.ResponseWriter, r *http.Request, call int)
}

func newBulkServer(t *testing.T, handle func(w http.ResponseWriter, r *http.Request, call int)) *bulkServer {
	t.Helper()
	server := &bulkServer{handle: handle}
	server.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != bulkPath {
			server.handle(w, r, -1)
			return
		}
		var body map[string]any
		_ = json.NewDecoder(r.Body).Decode(&body)
		server.mu.Lock()
		server.requests = append(server.requests, r.Clone(context.Background()))
		server.bodies = append(server.bodies, body)
		call := len(server.requests)
		server.mu.Unlock()
		server.handle(w, r, call)
	}))
	t.Cleanup(server.Close)
	return server
}

func (s *bulkServer) calls() []*http.Request {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]*http.Request(nil), s.requests...)
}

func (s *bulkServer) endpoint() string { return s.URL + "/v1/acct/apps/app/evaluate" }

func writeBulk(w http.ResponseWriter, etag, version string, flags []map[string]any, streams []map[string]any) {
	if flags == nil {
		flags = []map[string]any{}
	}
	body := map[string]any{"flags": flags, "metadata": map[string]any{"version": version}}
	if streams != nil {
		body["eventStreams"] = streams
	}
	w.Header().Set("ETag", etag)
	_ = json.NewEncoder(w).Encode(body)
}

func boolFlag(key string, value bool) map[string]any {
	return map[string]any{"key": key, "value": value, "variant": fmt.Sprint(value), "reason": "TARGETING_MATCH"}
}

func waitFor(t *testing.T, condition func() bool) {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for !condition() {
		if time.Now().After(deadline) {
			t.Fatal("condition not met")
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func nextEvent(t *testing.T, provider *ClientProvider, eventType openfeature.EventType) openfeature.Event {
	t.Helper()
	timeout := time.After(3 * time.Second)
	for {
		select {
		case event := <-provider.EventChannel():
			if event.EventType == eventType {
				return event
			}
		case <-timeout:
			t.Fatalf("no %s event", eventType)
		}
	}
}

func newTestClientProvider(t *testing.T, options ClientProviderOptions) *ClientProvider {
	t.Helper()
	provider, err := NewClientProvider(options)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(provider.Shutdown)
	return provider
}

func TestClientProviderCachesOneBulkEvaluation(t *testing.T) {
	server := newBulkServer(t, func(w http.ResponseWriter, r *http.Request, call int) {
		writeBulk(w, `"e1"`, "v1", []map[string]any{
			boolFlag("bool", true),
			{"key": "num", "value": 3, "variant": "three", "reason": "SPLIT"},
			{"key": "off", "reason": "DISABLED"},
			{"key": "broken", "errorCode": "PARSE_ERROR", "errorDetails": "bad config"},
		}, nil)
	})
	provider := newTestClientProvider(t, ClientProviderOptions{Endpoint: server.endpoint(), AuthToken: "token", ChangeDetection: ChangeDetectionNone})
	evalCtx := openfeature.NewEvaluationContext("user-1", map[string]any{"plan": "pro"})
	if err := provider.InitWithContext(context.Background(), evalCtx); err != nil {
		t.Fatal(err)
	}
	flat := openfeature.FlattenedContext{openfeature.TargetingKey: "user-1", "plan": "pro"}

	if got := provider.BooleanEvaluation(context.Background(), "bool", false, flat); !got.Value || got.Variant != "true" || got.Reason != openfeature.TargetingMatchReason {
		t.Fatalf("bool = %#v", got)
	}
	if got := provider.IntEvaluation(context.Background(), "num", 0, nil); got.Value != 3 || got.Reason != openfeature.SplitReason {
		t.Fatalf("num = %#v", got)
	}
	if got := provider.BooleanEvaluation(context.Background(), "off", true, nil); !got.Value || got.Reason != openfeature.DisabledReason {
		t.Fatalf("disabled = %#v", got)
	}
	if got := provider.BooleanEvaluation(context.Background(), "broken", false, nil); got.ResolutionDetail().ErrorCode != openfeature.ParseErrorCode {
		t.Fatalf("broken = %#v", got.ResolutionDetail())
	}
	if got := provider.StringEvaluation(context.Background(), "bool", "", nil); got.ResolutionDetail().ErrorCode != openfeature.TypeMismatchCode {
		t.Fatalf("mismatch = %#v", got.ResolutionDetail())
	}
	if got := provider.BooleanEvaluation(context.Background(), "missing", false, nil); got.ResolutionDetail().ErrorCode != openfeature.FlagNotFoundCode {
		t.Fatalf("missing = %#v", got.ResolutionDetail())
	}
	other := openfeature.FlattenedContext{openfeature.TargetingKey: "user-2"}
	if got := provider.BooleanEvaluation(context.Background(), "bool", false, other); got.ResolutionDetail().ErrorCode != openfeature.InvalidContextCode {
		t.Fatalf("other context = %#v", got.ResolutionDetail())
	}
	calls := server.calls()
	if len(calls) != 1 || calls[0].Header.Get("Authorization") != "Bearer token" {
		t.Fatalf("calls = %d", len(calls))
	}
	if context := server.bodies[0]["context"].(map[string]any); context["targetingKey"] != "user-1" || context["plan"] != "pro" {
		t.Fatalf("body = %#v", server.bodies[0])
	}
}

func TestClientProviderOptions(t *testing.T) {
	var header string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/ofrep/v1/evaluate/flags" {
			http.NotFound(w, r)
			return
		}
		header = r.Header.Get("X-API-Key")
		writeBulk(w, `"e"`, "v", nil, nil)
	}))
	defer server.Close()
	provider := newTestClientProvider(t, ClientProviderOptions{
		OFREPBaseURL: server.URL + "/api/", Headers: http.Header{"X-API-Key": []string{"k"}}, ChangeDetection: ChangeDetectionNone,
	})
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil || header != "k" {
		t.Fatalf("err = %v, header = %q", err, header)
	}
	if provider.client.timeout != 10*time.Second {
		t.Fatalf("timeout = %s", provider.client.timeout)
	}
	if _, err := NewClientProvider(ClientProviderOptions{OFREPBaseURL: server.URL, Endpoint: server.URL + "/evaluate"}); err == nil {
		t.Fatal("expected conflicting endpoints to fail")
	}
	if _, err := NewClientProvider(ClientProviderOptions{OFREPBaseURL: "ftp://flags.example.com"}); err == nil {
		t.Fatal("expected a non-HTTP base URL to fail")
	}
}

func TestClientProviderIdentifyIsUnconditionalAndFailsClosed(t *testing.T) {
	server := newBulkServer(t, func(w http.ResponseWriter, r *http.Request, call int) {
		switch call {
		case 1:
			writeBulk(w, `"e1"`, "v1", []map[string]any{boolFlag("f", false)}, nil)
		case 2:
			writeBulk(w, `"e2"`, "v1", []map[string]any{boolFlag("f", true)}, nil)
		default:
			w.WriteHeader(http.StatusInternalServerError)
		}
	})
	provider := newTestClientProvider(t, ClientProviderOptions{Endpoint: server.endpoint(), ChangeDetection: ChangeDetectionNone, DisableRetries: true})
	if err := provider.Init(openfeature.NewTargetlessEvaluationContext(map[string]any{"user": "a"})); err != nil {
		t.Fatal(err)
	}
	if err := provider.Identify(context.Background(), openfeature.NewTargetlessEvaluationContext(map[string]any{"user": "b"})); err != nil {
		t.Fatal(err)
	}
	event := nextEvent(t, provider, openfeature.ProviderConfigChange)
	if len(event.FlagChanges) != 1 || event.FlagChanges[0] != "f" {
		t.Fatalf("event = %#v", event)
	}
	if got := provider.BooleanEvaluation(context.Background(), "f", false, openfeature.FlattenedContext{"user": "b"}); !got.Value {
		t.Fatalf("identified = %#v", got)
	}
	if server.calls()[1].Header.Get("If-None-Match") != "" {
		t.Fatal("identify must not revalidate with the previous context's ETag")
	}
	if err := provider.Identify(context.Background(), openfeature.NewTargetlessEvaluationContext(map[string]any{"user": "c"})); err == nil {
		t.Fatal("expected identify to fail")
	}
	if got := provider.BooleanEvaluation(context.Background(), "f", false, nil); got.ResolutionDetail().ErrorCode != openfeature.ProviderNotReadyCode {
		t.Fatalf("after failure = %#v", got.ResolutionDetail())
	}
}

func TestClientProviderPollingRevalidatesWithResponseETag(t *testing.T) {
	server := newBulkServer(t, func(w http.ResponseWriter, r *http.Request, call int) {
		switch call {
		case 1:
			writeBulk(w, `"e1"`, "v1", []map[string]any{boolFlag("f", true)}, nil)
		case 2:
			w.WriteHeader(http.StatusNotModified)
		default:
			writeBulk(w, `"e2"`, "v2", []map[string]any{boolFlag("f", false)}, nil)
		}
	})
	provider := newTestClientProvider(t, ClientProviderOptions{Endpoint: server.endpoint(), ChangeDetection: ChangeDetectionPolling, PollInterval: 50 * time.Millisecond})
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatal(err)
	}
	event := nextEvent(t, provider, openfeature.ProviderConfigChange)
	if len(event.FlagChanges) != 1 {
		t.Fatalf("event = %#v", event)
	}
	if got := server.calls()[1].Header.Get("If-None-Match"); got != `"e1"` {
		t.Fatalf("If-None-Match = %q", got)
	}
	if got := provider.BooleanEvaluation(context.Background(), "f", true, nil); got.Value {
		t.Fatal("expected the refreshed value")
	}

	idle := newBulkServer(t, func(w http.ResponseWriter, r *http.Request, call int) { writeBulk(w, `"e"`, "v", nil, nil) })
	quiet := newTestClientProvider(t, ClientProviderOptions{Endpoint: idle.endpoint()})
	if err := quiet.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatal(err)
	}
	time.Sleep(200 * time.Millisecond)
	if len(idle.calls()) != 1 {
		t.Fatalf("default provider polled %d times", len(idle.calls())-1)
	}
}

func TestClientProviderAuthFailureIsFatal(t *testing.T) {
	server := newBulkServer(t, func(w http.ResponseWriter, r *http.Request, call int) {
		if call == 1 {
			writeBulk(w, `"e1"`, "v1", nil, nil)
			return
		}
		w.WriteHeader(http.StatusUnauthorized)
	})
	provider := newTestClientProvider(t, ClientProviderOptions{Endpoint: server.endpoint(), PollInterval: 50 * time.Millisecond})
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatal(err)
	}
	if event := nextEvent(t, provider, openfeature.ProviderError); event.ErrorCode != openfeature.ProviderFatalCode {
		t.Fatalf("event = %#v", event)
	}
	time.Sleep(200 * time.Millisecond)
	if len(server.calls()) != 2 {
		t.Fatalf("calls after fatal = %d", len(server.calls()))
	}

	rejected := newBulkServer(t, func(w http.ResponseWriter, r *http.Request, call int) { w.WriteHeader(http.StatusForbidden) })
	initial := newTestClientProvider(t, ClientProviderOptions{Endpoint: rejected.endpoint()})
	err := initial.Init(openfeature.EvaluationContext{})
	if initErr, ok := err.(*openfeature.ProviderInitError); !ok || initErr.ErrorCode != openfeature.ProviderFatalCode {
		t.Fatalf("init error = %#v", err)
	}
}

func TestClientProviderHonorsRetryAfter(t *testing.T) {
	server := newBulkServer(t, func(w http.ResponseWriter, r *http.Request, call int) {
		switch call {
		case 1:
			writeBulk(w, `"e1"`, "v1", nil, nil)
		case 2:
			w.Header().Set("Retry-After", "1")
			w.WriteHeader(http.StatusTooManyRequests)
		default:
			w.WriteHeader(http.StatusNotModified)
		}
	})
	provider := newTestClientProvider(t, ClientProviderOptions{Endpoint: server.endpoint(), PollInterval: 50 * time.Millisecond, Retries: 3, RetryDelay: time.Millisecond})
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatal(err)
	}
	nextEvent(t, provider, openfeature.ProviderStale)
	time.Sleep(500 * time.Millisecond)
	if got := len(server.calls()); got != 2 {
		t.Fatalf("calls during Retry-After = %d", got)
	}
	nextEvent(t, provider, openfeature.ProviderReady)
	if got := parseRetryAfter("Fri, 20 Feb 2026 21:28:18 GMT", time.Date(2026, 2, 20, 21, 28, 0, 0, time.UTC)); got != 18*time.Second {
		t.Fatalf("HTTP-date Retry-After = %s", got)
	}
}

func TestClientProviderSSEForwardsMetadataAcrossStreams(t *testing.T) {
	var server *bulkServer
	server = newBulkServer(t, func(w http.ResponseWriter, r *http.Request, call int) {
		if call == -1 {
			w.Header().Set("Content-Type", "text/event-stream")
			fmt.Fprintf(w, "data: {\"type\":\"unknown\"}\n\ndata: {\"type\":\"refetchEvaluation\",\"etag\":\"v2\",\"lastModified\":1771622898}\n\n")
			w.(http.Flusher).Flush()
			<-r.Context().Done()
			return
		}
		if r.URL.Query().Get("flagConfigEtag") == "v2" {
			writeBulk(w, `"e2"`, "v2", []map[string]any{boolFlag("f", true)}, nil)
			return
		}
		writeBulk(w, `"e1"`, "v1", []map[string]any{boolFlag("f", false)}, []map[string]any{
			{"type": "sse", "url": server.URL + "/a?ticket=1"},
			{"type": "websocket", "url": "wss://ignored.example.com"},
			{"type": "sse", "endpoint": map[string]any{"requestUri": "/b?ticket=2"}},
		})
	})
	provider := newTestClientProvider(t, ClientProviderOptions{Endpoint: server.endpoint()})
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatal(err)
	}
	nextEvent(t, provider, openfeature.ProviderConfigChange)
	if got := provider.BooleanEvaluation(context.Background(), "f", false, nil); !got.Value {
		t.Fatal("the cache must be refreshed before the event")
	}
	calls := server.calls()
	if calls[0].Header.Get("Flagship-Event-Stream") != "sse" {
		t.Fatal("initialization must request event streams")
	}
	var hinted []*http.Request
	for _, call := range calls {
		if call.URL.Query().Get("flagConfigEtag") == "v2" {
			hinted = append(hinted, call)
		}
	}
	if len(hinted) != 1 || hinted[0].URL.Query().Get("flagConfigLastModified") != "1771622898" || hinted[0].Header.Get("If-None-Match") != `"e1"` {
		t.Fatalf("hinted requests = %d", len(hinted))
	}
}

func TestClientProviderReconnectsWithFreshTicket(t *testing.T) {
	var tickets atomic.Int32
	var opened sync.Map
	var server *bulkServer
	server = newBulkServer(t, func(w http.ResponseWriter, r *http.Request, call int) {
		if call == -1 {
			opened.Store(r.URL.Query().Get("ticket"), true)
			w.Header().Set("Content-Type", "text/event-stream")
			return
		}
		ticket := tickets.Add(1)
		writeBulk(w, `"e"`, "v1", nil, []map[string]any{{"type": "sse", "url": fmt.Sprintf("%s/s?ticket=%d", server.URL, ticket)}})
	})
	provider := newTestClientProvider(t, ClientProviderOptions{Endpoint: server.endpoint()})
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatal(err)
	}
	waitFor(t, func() bool { _, ok := opened.Load("2"); return ok })
	second := server.calls()[1]
	if second.Header.Get("If-None-Match") != "" || second.Header.Get("Flagship-Event-Stream") != "sse" {
		t.Fatal("rediscovery must be unconditional and request a stream")
	}
}

func TestClientProviderPauseAndResume(t *testing.T) {
	var server *bulkServer
	server = newBulkServer(t, func(w http.ResponseWriter, r *http.Request, call int) {
		if call == -1 {
			w.Header().Set("Content-Type", "text/event-stream")
			fmt.Fprint(w, ": connected\n\n")
			w.(http.Flusher).Flush()
			<-r.Context().Done()
			return
		}
		writeBulk(w, `"e"`, "v1", nil, []map[string]any{{"type": "sse", "url": server.URL + "/s?ticket=t", "inactivityDelaySec": 1}})
	})
	provider := newTestClientProvider(t, ClientProviderOptions{Endpoint: server.endpoint(), InactivityDelay: 50 * time.Millisecond})
	if err := provider.Init(openfeature.EvaluationContext{}); err != nil {
		t.Fatal(err)
	}
	waitFor(t, func() bool { return provider.session != nil && provider.session.connected.Load() })
	provider.Pause()
	waitFor(t, func() bool { return !provider.session.connected.Load() })
	time.Sleep(100 * time.Millisecond)
	if len(server.calls()) != 1 {
		t.Fatalf("calls while paused = %d", len(server.calls()))
	}
	provider.Resume(context.Background())
	waitFor(t, func() bool { return len(server.calls()) == 2 })
	if server.calls()[1].Header.Get("If-None-Match") != "" {
		t.Fatal("resuming after inactivity must refetch unconditionally")
	}
	select {
	case event := <-provider.EventChannel():
		if event.EventType == openfeature.ProviderStale {
			t.Fatal("a pause must not report STALE")
		}
	default:
	}
}

func TestConfigurationHintLastModified(t *testing.T) {
	for raw, want := range map[string]string{
		`1771622898`:             "1771622898",
		`"2026-02-20T21:28:18Z"`: "2026-02-20T21:28:18Z",
		`"yesterday"`:            "",
		`-1`:                     "",
		``:                       "",
	} {
		if got := (configurationHint{LastModified: json.RawMessage(raw)}).lastModified(); got != want {
			t.Errorf("lastModified(%s) = %q, want %q", raw, got, want)
		}
	}
	urls := streamURLs([]eventStreamDescriptor{
		{Type: "sse", URL: "https://a.example.com/s"},
		{Type: "sse", URL: "https://a.example.com/s"},
		{Type: "other", URL: "https://b.example.com"},
	}, "https://api.example.com")
	if strings.Join(urls, ",") != "https://a.example.com/s" {
		t.Fatalf("urls = %v", urls)
	}
}
