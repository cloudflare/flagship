package flagship

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

var localDefinitions = map[string]any{
	"flags": map[string]any{
		"bool_flag": map[string]any{
			"key":               "bool_flag",
			"enabled":           true,
			"default_variation": "off",
			"variations":        map[string]any{"on": true, "off": false},
			"rules": []any{
				map[string]any{
					"conditions": []any{
						map[string]any{"attribute": "plan", "operator": "equals", "value": "pro"},
					},
					"serve_variation": "on",
				},
			},
		},
		"string_flag": map[string]any{
			"key":               "string_flag",
			"enabled":           true,
			"default_variation": "a",
			"variations":        map[string]any{"a": "hello", "b": "world"},
			"rules":             []any{},
		},
		"disabled_flag": map[string]any{
			"key":               "disabled_flag",
			"enabled":           false,
			"default_variation": "off",
			"variations":        map[string]any{"on": true, "off": false},
			"rules":             []any{},
		},
		"bad_flag": map[string]any{
			"key":               "bad_flag",
			"enabled":           true,
			"default_variation": "missing",
			"variations":        map[string]any{"on": true},
			"rules":             []any{},
		},
	},
}

func TestResolveDefinitionsEndpoint(t *testing.T) {
	got, err := resolveDefinitionsEndpoint(Options{
		AppID:     "app-1",
		AccountID: "acct-1",
		BaseURL:   "https://api.example.com",
	})
	if err != nil {
		t.Fatal(err)
	}
	want := "https://api.example.com/client/v4/accounts/acct-1/flagship/apps/app-1/definitions"
	if got != want {
		t.Fatalf("got %q, want %q", got, want)
	}

	got, err = resolveDefinitionsEndpoint(Options{
		Endpoint: "https://api.example.com/v1/acct/apps/app/evaluate",
	})
	if err != nil {
		t.Fatal(err)
	}
	want = "https://api.example.com/v1/acct/apps/app/definitions"
	if got != want {
		t.Fatalf("got %q, want %q", got, want)
	}

	_, err = resolveDefinitionsEndpoint(Options{Endpoint: "https://api.example.com/v1/flags"})
	if err == nil || !strings.Contains(err.Error(), `/evaluate`) {
		t.Fatalf("err = %v", err)
	}
}

func TestNewProviderLocalValidation(t *testing.T) {
	t.Run("requires accountID", func(t *testing.T) {
		_, err := NewProvider(Options{
			Endpoint:        "https://example.com/evaluate",
			LocalEvaluation: true,
		})
		if err == nil || !strings.Contains(err.Error(), "accountID") {
			t.Fatalf("err = %v", err)
		}
	})

	t.Run("rejects cacheTTL", func(t *testing.T) {
		_, err := NewProvider(Options{
			AppID:           "a",
			AccountID:       "b",
			LocalEvaluation: true,
			CacheTTL:        time.Second,
		})
		if err == nil || !strings.Contains(err.Error(), "CacheTTL") {
			t.Fatalf("err = %v", err)
		}
	})

	t.Run("rejects non-positive refreshInterval", func(t *testing.T) {
		_, err := NewProvider(Options{
			AppID:           "a",
			AccountID:       "b",
			LocalEvaluation: true,
			RefreshInterval: -time.Second,
		})
		if err == nil || !strings.Contains(err.Error(), "refreshInterval") {
			t.Fatalf("err = %v", err)
		}
	})

	t.Run("rejects endpoint without /evaluate", func(t *testing.T) {
		_, err := NewProvider(Options{
			Endpoint:        "https://example.com/flags",
			AccountID:       "b",
			LocalEvaluation: true,
		})
		if err == nil || !strings.Contains(err.Error(), `/evaluate`) {
			t.Fatalf("err = %v", err)
		}
	})
}

func TestFetchDefinitions(t *testing.T) {
	t.Run("returns flags and etag on 200", func(t *testing.T) {
		var gotPath string
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			gotPath = r.URL.Path
			w.Header().Set("ETag", `"abc"`)
			_ = json.NewEncoder(w).Encode(map[string]any{"flags": map[string]any{"f1": map[string]any{"key": "f1"}}})
		}))
		defer server.Close()

		client, err := NewClient(Options{
			Endpoint:       server.URL + "/evaluate",
			DisableRetries: true,
		})
		if err != nil {
			t.Fatal(err)
		}
		result, err := client.fetchDefinitions(context.Background(), "")
		if err != nil {
			t.Fatal(err)
		}
		if result.ETag != `"abc"` || result.Flags["f1"] == nil {
			t.Fatalf("result = %#v", result)
		}
		if !strings.HasSuffix(gotPath, "/definitions") {
			t.Fatalf("path = %q", gotPath)
		}
	})

	t.Run("sends If-None-Match and returns not-modified on 304", func(t *testing.T) {
		var gotMatch string
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			gotMatch = r.Header.Get("If-None-Match")
			w.WriteHeader(http.StatusNotModified)
		}))
		defer server.Close()

		client, err := NewClient(Options{
			Endpoint:       server.URL + "/evaluate",
			DisableRetries: true,
		})
		if err != nil {
			t.Fatal(err)
		}
		result, err := client.fetchDefinitions(context.Background(), `"abc"`)
		if err != nil {
			t.Fatal(err)
		}
		if !result.NotModified || gotMatch != `"abc"` {
			t.Fatalf("result=%#v match=%q", result, gotMatch)
		}
	})

	t.Run("rejects malformed payload", func(t *testing.T) {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			_ = json.NewEncoder(w).Encode(map[string]any{"notFlags": true})
		}))
		defer server.Close()

		client, err := NewClient(Options{
			Endpoint:       server.URL + "/evaluate",
			DisableRetries: true,
		})
		if err != nil {
			t.Fatal(err)
		}
		_, err = client.fetchDefinitions(context.Background(), "")
		flagshipErr, ok := asFlagshipError(err)
		if !ok || flagshipErr.Code != ErrorCodeParse {
			t.Fatalf("err = %v", err)
		}
	})
}

type definitionsHandler struct {
	mu      sync.Mutex
	calls   int
	handler func(call int, w http.ResponseWriter, r *http.Request)
}

func (h *definitionsHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if !strings.Contains(r.URL.Path, "/definitions") {
		http.Error(w, "unexpected path "+r.URL.Path, http.StatusNotFound)
		return
	}
	h.mu.Lock()
	h.calls++
	call := h.calls
	handler := h.handler
	h.mu.Unlock()
	handler(call, w, r)
}

func (h *definitionsHandler) Calls() int {
	h.mu.Lock()
	defer h.mu.Unlock()
	return h.calls
}

func writeJSON(w http.ResponseWriter, status int, etag string, body any) {
	if etag != "" {
		w.Header().Set("ETag", etag)
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(body)
}

func newLocalProvider(t *testing.T, serverURL string, opts Options) *ServerProvider {
	t.Helper()
	opts.Endpoint = serverURL + "/evaluate"
	opts.AccountID = "acct-1"
	opts.LocalEvaluation = true
	opts.DisableRetries = true
	if opts.RefreshInterval == 0 {
		opts.RefreshInterval = time.Hour // keep background quiet unless a test opts in
	}
	provider, err := NewProvider(opts)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = provider.ShutdownWithContext(context.Background())
	})
	return provider
}

func TestLocalProviderInitialize(t *testing.T) {
	t.Run("loads definitions on success", func(t *testing.T) {
		handler := &definitionsHandler{handler: func(_ int, w http.ResponseWriter, _ *http.Request) {
			writeJSON(w, http.StatusOK, `"v1"`, localDefinitions)
		}}
		server := httptest.NewServer(handler)
		defer server.Close()

		provider := newLocalProvider(t, server.URL, Options{})
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}
		if handler.Calls() != 1 {
			t.Fatalf("calls = %d", handler.Calls())
		}

		d := provider.BooleanEvaluation(context.Background(), "bool_flag", false, openfeature.FlattenedContext{"plan": "pro"})
		if d.Value != true || d.Reason != openfeature.TargetingMatchReason || d.Variant != "on" {
			t.Fatalf("detail = %#v", d)
		}
	})

	t.Run("throws on failure", func(t *testing.T) {
		handler := &definitionsHandler{handler: func(_ int, w http.ResponseWriter, _ *http.Request) {
			http.Error(w, "boom", http.StatusInternalServerError)
		}}
		server := httptest.NewServer(handler)
		defer server.Close()

		provider := newLocalProvider(t, server.URL, Options{})
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err == nil {
			t.Fatal("expected init error")
		}
	})
}

func TestLocalProviderEvaluation(t *testing.T) {
	handler := &definitionsHandler{handler: func(_ int, w http.ResponseWriter, _ *http.Request) {
		writeJSON(w, http.StatusOK, `"v1"`, localDefinitions)
	}}
	server := httptest.NewServer(handler)
	defer server.Close()

	provider := newLocalProvider(t, server.URL, Options{})
	if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
		t.Fatal(err)
	}

	t.Run("FLAG_NOT_FOUND", func(t *testing.T) {
		d := provider.BooleanEvaluation(context.Background(), "nope", false, nil)
		requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.FlagNotFoundCode)
	})

	t.Run("PARSE_ERROR missing variation", func(t *testing.T) {
		d := provider.BooleanEvaluation(context.Background(), "bad_flag", false, nil)
		requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.ParseErrorCode)
	})

	t.Run("DISABLED default", func(t *testing.T) {
		d := provider.BooleanEvaluation(context.Background(), "disabled_flag", true, nil)
		if d.Value != true || d.Reason != openfeature.DisabledReason {
			t.Fatalf("detail = %#v", d)
		}
	})

	t.Run("TYPE_MISMATCH", func(t *testing.T) {
		d := provider.BooleanEvaluation(context.Background(), "string_flag", false, nil)
		requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.TypeMismatchCode)
	})

	t.Run("STATIC", func(t *testing.T) {
		d := provider.StringEvaluation(context.Background(), "string_flag", "x", nil)
		if d.Value != "hello" || d.Reason != openfeature.StaticReason || d.Variant != "a" {
			t.Fatalf("detail = %#v", d)
		}
	})

	t.Run("PROVIDER_NOT_READY before initialize", func(t *testing.T) {
		fresh := newLocalProvider(t, server.URL, Options{})
		d := fresh.BooleanEvaluation(context.Background(), "bool_flag", false, nil)
		requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.ProviderNotReadyCode)
	})

	t.Run("malformed flag does not poison others", func(t *testing.T) {
		malformed := map[string]any{
			"flags": map[string]any{
				"broken": map[string]any{
					"key":               "broken",
					"enabled":           true,
					"default_variation": "a",
					// missing variations intentionally
				},
				"ok": localDefinitions["flags"].(map[string]any)["string_flag"],
			},
		}
		h := &definitionsHandler{handler: func(_ int, w http.ResponseWriter, _ *http.Request) {
			writeJSON(w, http.StatusOK, `"v1"`, malformed)
		}}
		s := httptest.NewServer(h)
		defer s.Close()
		p := newLocalProvider(t, s.URL, Options{})
		if err := p.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}
		bad := p.BooleanEvaluation(context.Background(), "broken", false, nil)
		requireResolutionErrorCode(t, bad.ResolutionDetail(), openfeature.ParseErrorCode)
		ok := p.StringEvaluation(context.Background(), "ok", "x", nil)
		if ok.Value != "hello" {
			t.Fatalf("ok = %#v", ok)
		}
	})
}

func TestLocalProviderRefresh(t *testing.T) {
	t.Run("sends If-None-Match and keeps snapshot on 304", func(t *testing.T) {
		var matches []string
		var mu sync.Mutex
		handler := &definitionsHandler{handler: func(call int, w http.ResponseWriter, r *http.Request) {
			mu.Lock()
			matches = append(matches, r.Header.Get("If-None-Match"))
			mu.Unlock()
			if call == 1 {
				writeJSON(w, http.StatusOK, `"v1"`, localDefinitions)
				return
			}
			w.WriteHeader(http.StatusNotModified)
		}}
		server := httptest.NewServer(handler)
		defer server.Close()

		provider := newLocalProvider(t, server.URL, Options{RefreshInterval: 20 * time.Millisecond})
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}

		deadline := time.Now().Add(2 * time.Second)
		for handler.Calls() < 2 && time.Now().Before(deadline) {
			time.Sleep(10 * time.Millisecond)
		}
		if handler.Calls() < 2 {
			t.Fatalf("expected refresh, calls=%d", handler.Calls())
		}
		mu.Lock()
		defer mu.Unlock()
		if matches[0] != "" || matches[1] != `"v1"` {
			t.Fatalf("matches = %#v", matches)
		}

		d := provider.BooleanEvaluation(context.Background(), "bool_flag", false, openfeature.FlattenedContext{"plan": "pro"})
		if d.Value != true {
			t.Fatalf("detail = %#v", d)
		}
	})

	t.Run("swaps snapshot on 200 refresh", func(t *testing.T) {
		updated := map[string]any{
			"flags": map[string]any{
				"bool_flag": map[string]any{
					"key":               "bool_flag",
					"enabled":           true,
					"default_variation": "off",
					"variations":        map[string]any{"on": true, "off": false},
					"rules": []any{
						map[string]any{
							"conditions": []any{
								map[string]any{"attribute": "plan", "operator": "equals", "value": "enterprise"},
							},
							"serve_variation": "on",
						},
					},
				},
			},
		}
		handler := &definitionsHandler{handler: func(call int, w http.ResponseWriter, _ *http.Request) {
			if call == 1 {
				writeJSON(w, http.StatusOK, `"v1"`, localDefinitions)
				return
			}
			writeJSON(w, http.StatusOK, `"v2"`, updated)
		}}
		server := httptest.NewServer(handler)
		defer server.Close()

		provider := newLocalProvider(t, server.URL, Options{RefreshInterval: 20 * time.Millisecond})
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}

		deadline := time.Now().Add(2 * time.Second)
		for handler.Calls() < 2 && time.Now().Before(deadline) {
			time.Sleep(10 * time.Millisecond)
		}
		if handler.Calls() < 2 {
			t.Fatal("refresh did not run")
		}

		// Allow the snapshot swap to land.
		time.Sleep(20 * time.Millisecond)
		d := provider.BooleanEvaluation(context.Background(), "bool_flag", false, openfeature.FlattenedContext{"plan": "pro"})
		if d.Value != false || d.Reason != openfeature.DefaultReason {
			t.Fatalf("detail = %#v", d)
		}
	})

	t.Run("keeps old snapshot when refresh fails", func(t *testing.T) {
		handler := &definitionsHandler{handler: func(call int, w http.ResponseWriter, _ *http.Request) {
			if call == 1 {
				writeJSON(w, http.StatusOK, `"v1"`, localDefinitions)
				return
			}
			http.Error(w, "boom", http.StatusInternalServerError)
		}}
		server := httptest.NewServer(handler)
		defer server.Close()

		provider := newLocalProvider(t, server.URL, Options{RefreshInterval: 20 * time.Millisecond})
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}

		deadline := time.Now().Add(2 * time.Second)
		for handler.Calls() < 2 && time.Now().Before(deadline) {
			time.Sleep(10 * time.Millisecond)
		}
		d := provider.BooleanEvaluation(context.Background(), "bool_flag", false, openfeature.FlattenedContext{"plan": "pro"})
		if d.Value != true {
			t.Fatalf("detail = %#v", d)
		}
	})
}

func TestLocalProviderShutdown(t *testing.T) {
	t.Run("drops the snapshot", func(t *testing.T) {
		handler := &definitionsHandler{handler: func(_ int, w http.ResponseWriter, _ *http.Request) {
			writeJSON(w, http.StatusOK, `"v1"`, localDefinitions)
		}}
		server := httptest.NewServer(handler)
		defer server.Close()

		provider := newLocalProvider(t, server.URL, Options{})
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}
		if err := provider.ShutdownWithContext(context.Background()); err != nil {
			t.Fatal(err)
		}
		d := provider.BooleanEvaluation(context.Background(), "bool_flag", false, openfeature.FlattenedContext{"plan": "pro"})
		requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.ProviderNotReadyCode)
	})

	t.Run("can be re-initialized after close", func(t *testing.T) {
		handler := &definitionsHandler{handler: func(_ int, w http.ResponseWriter, _ *http.Request) {
			writeJSON(w, http.StatusOK, `"v1"`, localDefinitions)
		}}
		server := httptest.NewServer(handler)
		defer server.Close()

		provider := newLocalProvider(t, server.URL, Options{})
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}
		if err := provider.ShutdownWithContext(context.Background()); err != nil {
			t.Fatal(err)
		}
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}
		d := provider.BooleanEvaluation(context.Background(), "bool_flag", false, openfeature.FlattenedContext{"plan": "pro"})
		if d.Value != true {
			t.Fatalf("detail = %#v", d)
		}
	})

	t.Run("stops the refresh goroutine", func(t *testing.T) {
		handler := &definitionsHandler{handler: func(_ int, w http.ResponseWriter, _ *http.Request) {
			writeJSON(w, http.StatusOK, `"v1"`, localDefinitions)
		}}
		server := httptest.NewServer(handler)
		defer server.Close()

		before := runtime.NumGoroutine()
		provider := newLocalProvider(t, server.URL, Options{RefreshInterval: 10 * time.Millisecond})
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}
		if err := provider.ShutdownWithContext(context.Background()); err != nil {
			t.Fatal(err)
		}
		// Give the runtime a moment to reap the goroutine.
		deadline := time.Now().Add(2 * time.Second)
		for time.Now().Before(deadline) {
			if runtime.NumGoroutine() <= before+2 {
				return
			}
			time.Sleep(10 * time.Millisecond)
		}
		after := runtime.NumGoroutine()
		if after > before+5 {
			t.Fatalf("goroutines leaked: before=%d after=%d", before, after)
		}
	})

	t.Run("ignores in-flight refresh after close", func(t *testing.T) {
		release := make(chan struct{})
		var once atomic.Bool
		handler := &definitionsHandler{handler: func(call int, w http.ResponseWriter, _ *http.Request) {
			if call == 1 {
				writeJSON(w, http.StatusOK, `"v1"`, localDefinitions)
				return
			}
			if once.CompareAndSwap(false, true) {
				<-release
			}
			writeJSON(w, http.StatusOK, `"v2"`, map[string]any{
				"flags": map[string]any{
					"bool_flag": map[string]any{
						"key":               "bool_flag",
						"enabled":           true,
						"default_variation": "off",
						"variations":        map[string]any{"on": true, "off": false},
						"rules":             []any{},
					},
				},
			})
		}}
		server := httptest.NewServer(handler)
		defer server.Close()

		provider := newLocalProvider(t, server.URL, Options{RefreshInterval: 15 * time.Millisecond})
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}

		deadline := time.Now().Add(2 * time.Second)
		for handler.Calls() < 2 && time.Now().Before(deadline) {
			time.Sleep(5 * time.Millisecond)
		}
		if handler.Calls() < 2 {
			t.Fatal("refresh did not start")
		}

		if err := provider.ShutdownWithContext(context.Background()); err != nil {
			t.Fatal(err)
		}
		close(release)
		time.Sleep(30 * time.Millisecond)

		d := provider.BooleanEvaluation(context.Background(), "bool_flag", false, nil)
		requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.ProviderNotReadyCode)
	})

	t.Run("ignores stale in-flight refresh after close + re-initialize", func(t *testing.T) {
		staleDefinitions := map[string]any{
			"flags": map[string]any{
				"bool_flag": map[string]any{
					"key":               "bool_flag",
					"enabled":           true,
					"default_variation": "off",
					"variations":        map[string]any{"on": true, "off": false},
					"rules":             []any{},
				},
			},
		}
		freshDefinitions := map[string]any{
			"flags": map[string]any{
				"bool_flag": map[string]any{
					"key":               "bool_flag",
					"enabled":           true,
					"default_variation": "on",
					"variations":        map[string]any{"on": true, "off": false},
					"rules":             []any{},
				},
			},
		}

		releaseStale := make(chan struct{})
		handler := &definitionsHandler{handler: func(call int, w http.ResponseWriter, _ *http.Request) {
			switch call {
			case 1:
				writeJSON(w, http.StatusOK, `"v1"`, staleDefinitions)
			case 2:
				<-releaseStale
				writeJSON(w, http.StatusOK, `"stale"`, staleDefinitions)
			default:
				writeJSON(w, http.StatusOK, `"v2"`, freshDefinitions)
			}
		}}
		server := httptest.NewServer(handler)
		defer server.Close()

		provider := newLocalProvider(t, server.URL, Options{RefreshInterval: 15 * time.Millisecond})
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}

		deadline := time.Now().Add(2 * time.Second)
		for handler.Calls() < 2 && time.Now().Before(deadline) {
			time.Sleep(5 * time.Millisecond)
		}
		if handler.Calls() < 2 {
			t.Fatal("stale refresh did not start")
		}

		if err := provider.ShutdownWithContext(context.Background()); err != nil {
			t.Fatal(err)
		}
		if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
			t.Fatal(err)
		}

		before := provider.BooleanEvaluation(context.Background(), "bool_flag", false, nil)
		if before.Value != true || before.Reason != openfeature.StaticReason || before.Variant != "on" {
			t.Fatalf("before = %#v", before)
		}

		close(releaseStale)
		time.Sleep(50 * time.Millisecond)

		after := provider.BooleanEvaluation(context.Background(), "bool_flag", false, nil)
		if after.Value != true || after.Reason != openfeature.StaticReason || after.Variant != "on" {
			t.Fatalf("after = %#v (stale refresh overwrote snapshot)", after)
		}
	})
}
