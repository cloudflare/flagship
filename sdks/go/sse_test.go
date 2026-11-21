package flagship

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

func TestConfigurationEventParser(t *testing.T) {
	var hints []string
	input := ": keepalive\n\nevent: unknown\ndata: {\"type\":\"refetchEvaluation\",\"etag\":\"ignored\"}\n\n" +
		"data: {\"type\":\"unknown\"}\n\ndata: {\"type\":\"refetchEvaluation\",\n" +
		"data: \"etag\":\"new-version\"}\n\n"
	err := readConfigurationEvents(strings.NewReader(input), func(hint configurationHint) error {
		hints = append(hints, hint.ETag)
		return nil
	})
	if err == nil || len(hints) != 1 || hints[0] != "new-version" {
		t.Fatalf("hints = %v, error = %v", hints, err)
	}
}

func TestConfigurationEventStreamURL(t *testing.T) {
	descriptors := []eventStreamDescriptor{{Type: "unknown", URL: "https://other.example.com"}, {Type: "sse", URL: "https://events.example.com/stream?ticket=one"}}
	got, err := streamURL(descriptors, "https://api.example.com/definitions")
	if err != nil || got != "https://events.example.com/stream?ticket=one" {
		t.Fatalf("url = %q, error = %v", got, err)
	}
	if _, err := streamURL([]eventStreamDescriptor{{Type: "sse", URL: "http://example.com/stream"}}, "https://api.example.com"); err == nil {
		t.Fatal("expected an insecure stream URL to be rejected")
	}
}

func TestLocalConfigurationEventsRefreshBeforeNotification(t *testing.T) {
	versionOne, versionTwo := strings.Repeat("a", 64), strings.Repeat("b", 64)
	var current atomic.Int32
	opened := make(chan struct{}, 1)
	publish := make(chan struct{})
	remove := make(chan struct{})
	var streamAuth atomic.Bool
	var server *httptest.Server
	server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/definitions":
			if current.Load() == 3 {
				w.WriteHeader(http.StatusNotFound)
				return
			}
			version := versionOne
			if current.Load() == 2 {
				version = versionTwo
			}
			w.Header().Set("ETag", version)
			payload := map[string]any{
				"flags": map[string]any{"enabled": map[string]any{
					"key": "enabled", "enabled": true, "default_variation": "on",
					"variations": map[string]any{"on": current.Load() == 2}, "rules": []any{},
				}},
				"metadata": map[string]any{"version": version},
			}
			if r.Header.Get("Flagship-Event-Stream") == "sse" {
				payload["eventStreams"] = []map[string]string{{"type": "sse", "url": server.URL + "/events?ticket=one"}}
			}
			_ = json.NewEncoder(w).Encode(payload)
		case "/events":
			streamAuth.Store(r.Header.Get("Authorization") != "")
			w.Header().Set("Content-Type", "text/event-stream")
			_, _ = fmt.Fprint(w, ": connected\n\n")
			w.(http.Flusher).Flush()
			opened <- struct{}{}
			select {
			case <-publish:
				_, _ = fmt.Fprintf(w, "data: {\"type\":\"refetchEvaluation\",\"etag\":\"%s\"}\n\n", versionTwo)
				w.(http.Flusher).Flush()
				select {
				case <-remove:
					_, _ = fmt.Fprint(w, "data: {\"type\":\"refetchEvaluation\"}\n\n")
					w.(http.Flusher).Flush()
				case <-r.Context().Done():
				}
				<-r.Context().Done()
			case <-r.Context().Done():
			}
		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	defer server.Close()
	provider, err := NewProvider(Options{
		Endpoint: server.URL + "/evaluate", AccountID: "acct", AuthToken: "private",
		LocalEvaluation: true, ConfigurationEvents: true, RefreshInterval: time.Hour, DisableRetries: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
		t.Fatal(err)
	}
	defer provider.Shutdown()
	select {
	case <-opened:
	case <-time.After(3 * time.Second):
		t.Fatal("configuration stream did not connect")
	}
	if streamAuth.Load() {
		t.Fatal("stream received the discovery bearer token")
	}
	current.Store(2)
	close(publish)
	select {
	case event := <-provider.EventChannel():
		if event.EventType != openfeature.ProviderConfigChange {
			t.Fatalf("unexpected event: %#v", event)
		}
		details := provider.BooleanEvaluation(context.Background(), "enabled", false, nil)
		if !details.Value {
			t.Fatalf("event emitted before refreshed snapshot was installed: %#v", details)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("configuration change was not emitted")
	}
	current.Store(3)
	close(remove)
	select {
	case event := <-provider.EventChannel():
		if event.EventType != openfeature.ProviderError || provider.snapshot.Load() != nil {
			t.Fatalf("deleted configuration did not invalidate local state: %#v", event)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("deleted configuration did not report provider error")
	}
}

func TestRemoteConfigurationEventsRefreshCacheAndReplaceTicket(t *testing.T) {
	versionOne, versionTwo := strings.Repeat("c", 64), strings.Repeat("d", 64)
	var current atomic.Int32
	var discovery atomic.Int32
	var evaluations atomic.Int32
	opened := make(chan struct{}, 2)
	publish := make(chan struct{})
	var server *httptest.Server
	server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/ofrep/v1/evaluate/flags":
			if r.Header.Get("Flagship-Event-Stream") == "sse" {
				ticket := discovery.Add(1)
				_ = json.NewEncoder(w).Encode(map[string]any{
					"metadata":     map[string]any{"version": versionOne},
					"eventStreams": []map[string]string{{"type": "sse", "url": fmt.Sprintf("%s/events?ticket=%d", server.URL, ticket)}},
				})
				return
			}
			version := versionOne
			if current.Load() == 2 {
				version = versionTwo
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"metadata": map[string]any{"version": version}})
		case "/events":
			if r.Header.Get("Authorization") != "" {
				t.Error("stream received discovery credentials")
			}
			w.Header().Set("Content-Type", "text/event-stream")
			_, _ = fmt.Fprint(w, ": connected\n\n")
			w.(http.Flusher).Flush()
			opened <- struct{}{}
			if r.URL.Query().Get("ticket") == "1" {
				select {
				case <-publish:
					_, _ = fmt.Fprintf(w, "data: {\"type\":\"refetchEvaluation\",\"etag\":\"%s\"}\n\n", versionTwo)
					w.(http.Flusher).Flush()
				case <-r.Context().Done():
				}
			} else {
				<-r.Context().Done()
			}
		case "/evaluate":
			evaluations.Add(1)
			writeEvaluationResponse(w, current.Load() == 2, "on", "DEFAULT")
		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	defer server.Close()
	provider, err := NewProvider(Options{
		Endpoint: server.URL + "/evaluate", CacheTTL: time.Hour, AuthToken: "private",
		ConfigurationEvents: true, DisableRetries: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
		t.Fatal(err)
	}
	defer provider.Shutdown()
	select {
	case <-opened:
	case <-time.After(3 * time.Second):
		t.Fatal("remote stream did not connect")
	}
	_ = provider.BooleanEvaluation(context.Background(), "flag", false, nil)
	_ = provider.BooleanEvaluation(context.Background(), "flag", false, nil)
	if evaluations.Load() != 1 {
		t.Fatalf("expected a cached evaluation, got %d requests", evaluations.Load())
	}
	current.Store(2)
	close(publish)
	deadline := time.After(3 * time.Second)
	for {
		select {
		case event := <-provider.EventChannel():
			if event.EventType == openfeature.ProviderConfigChange {
				details := provider.BooleanEvaluation(context.Background(), "flag", false, nil)
				if !details.Value || evaluations.Load() != 2 {
					t.Fatalf("remote cache was not invalidated before event: %#v, requests=%d", details, evaluations.Load())
				}
				select {
				case <-opened:
					if discovery.Load() < 2 {
						t.Fatal("reconnect did not request a fresh ticket")
					}
				case <-time.After(3 * time.Second):
					t.Fatal("reconnect did not open a fresh stream")
				}
				return
			}
		case <-deadline:
			t.Fatal("remote configuration event not received")
		}
	}
}
