package flagship

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

const metadataJSON = `{"owner":"team-payments","jira.id":"PROJ-123","experiment.active":false,"weight":2,"ratio":0.5}`

var wantMetadata = openfeature.FlagMetadata{
	"owner":             "team-payments",
	"jira.id":           "PROJ-123",
	"experiment.active": false,
	"weight":            int64(2),
	"ratio":             0.5,
}

func TestToFlagMetadata(t *testing.T) {
	t.Run("nil yields empty non-nil map", func(t *testing.T) {
		got := toFlagMetadata(nil)
		if got == nil || len(got) != 0 {
			t.Fatalf("got %#v", got)
		}
	})

	t.Run("converts primitives", func(t *testing.T) {
		var raw map[string]any
		decoder := json.NewDecoder(strings.NewReader(metadataJSON))
		decoder.UseNumber()
		if err := decoder.Decode(&raw); err != nil {
			t.Fatal(err)
		}
		if got := toFlagMetadata(raw); !reflect.DeepEqual(got, wantMetadata) {
			t.Fatalf("got %#v, want %#v", got, wantMetadata)
		}
	})

	t.Run("drops non-primitive and out-of-range entries", func(t *testing.T) {
		got := toFlagMetadata(map[string]any{
			"ok": "yes", "nested": map[string]any{"a": 1}, "list": []any{1}, "nil": nil, "big": json.Number("1e999"),
		})
		if !reflect.DeepEqual(got, openfeature.FlagMetadata{"ok": "yes"}) {
			t.Fatalf("got %#v", got)
		}
	})

	t.Run("returns a fresh map each call", func(t *testing.T) {
		raw := map[string]any{"a": "b"}
		first := toFlagMetadata(raw)
		first["a"] = "mutated"
		if toFlagMetadata(raw)["a"] != "b" {
			t.Fatal("mutating a result leaked into later results")
		}
	})
}

func metadataServer(t *testing.T, body string, calls *int) *httptest.Server {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		*calls++
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprint(w, body)
	}))
	t.Cleanup(server.Close)
	return server
}

func TestProviderHTTPFlagMetadata(t *testing.T) {
	var calls int

	t.Run("maps metadata", func(t *testing.T) {
		server := metadataServer(t, fmt.Sprintf(`{"flagKey":"k","value":true,"variant":"on","reason":"STATIC","metadata":%s}`, metadataJSON), &calls)
		d := newTestProvider(t, server.URL, Options{}).BooleanEvaluation(context.Background(), "k", false, nil)
		if !reflect.DeepEqual(d.FlagMetadata, wantMetadata) {
			t.Fatalf("FlagMetadata = %#v", d.FlagMetadata)
		}
	})

	t.Run("keeps metadata for disabled flags", func(t *testing.T) {
		server := metadataServer(t, fmt.Sprintf(`{"flagKey":"k","value":false,"reason":"DISABLED","metadata":%s}`, metadataJSON), &calls)
		d := newTestProvider(t, server.URL, Options{}).BooleanEvaluation(context.Background(), "k", true, nil)
		if d.Reason != openfeature.DisabledReason || !reflect.DeepEqual(d.FlagMetadata, wantMetadata) {
			t.Fatalf("detail = %#v", d)
		}
	})

	t.Run("absent or malformed metadata is empty", func(t *testing.T) {
		for _, metadata := range []string{``, `,"metadata":"oops"`, `,"metadata":null`, `,"metadata":[1]`} {
			server := metadataServer(t, `{"flagKey":"k","value":true,"reason":"STATIC"`+metadata+`}`, &calls)
			d := newTestProvider(t, server.URL, Options{}).BooleanEvaluation(context.Background(), "k", false, nil)
			if d.Reason == openfeature.ErrorReason || d.FlagMetadata == nil || len(d.FlagMetadata) != 0 {
				t.Fatalf("metadata %q: detail = %#v", metadata, d)
			}
		}
	})

	t.Run("cache hit preserves metadata", func(t *testing.T) {
		var cacheCalls int
		server := metadataServer(t, fmt.Sprintf(`{"flagKey":"k","value":true,"variant":"on","reason":"STATIC","metadata":%s}`, metadataJSON), &cacheCalls)
		provider := newTestProvider(t, server.URL, Options{CacheTTL: time.Minute})
		provider.BooleanEvaluation(context.Background(), "k", false, nil)
		hit := provider.BooleanEvaluation(context.Background(), "k", false, nil)
		if cacheCalls != 1 || hit.Reason != openfeature.CachedReason || !reflect.DeepEqual(hit.FlagMetadata, wantMetadata) {
			t.Fatalf("calls=%d detail=%#v", cacheCalls, hit)
		}
	})
}

func TestEvaluateFlagMetadata(t *testing.T) {
	flag, err := decodeEvalFlag(json.RawMessage(fmt.Sprintf(
		`{"key":"f","enabled":true,"default_variation":"on","variations":{"on":true},"rules":[],"metadata":%s}`, metadataJSON)))
	if err != nil {
		t.Fatal(err)
	}

	result, err := evaluateFlag(flag, map[string]any{}, "acct")
	if err != nil || !reflect.DeepEqual(toFlagMetadata(result.Metadata), wantMetadata) {
		t.Fatalf("result = %#v, err = %v", result, err)
	}

	flag.Enabled = false
	result, err = evaluateFlag(flag, map[string]any{}, "acct")
	if err != nil || result.Reason != ReasonDisabled || !reflect.DeepEqual(toFlagMetadata(result.Metadata), wantMetadata) {
		t.Fatalf("disabled result = %#v, err = %v", result, err)
	}
}

func TestProviderLocalFlagMetadata(t *testing.T) {
	flag := `{"key":"%s","enabled":true,"default_variation":"on","variations":{"on":true},"rules":[]%s}`
	body := fmt.Sprintf(`{"flags":{"f":%s,"g":%s}}`,
		fmt.Sprintf(flag, "f", `,"metadata":`+metadataJSON), fmt.Sprintf(flag, "g", ""))
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprint(w, body)
	}))
	defer server.Close()

	provider, err := NewProvider(Options{
		Endpoint:        server.URL + "/evaluate",
		AccountID:       "acct",
		LocalEvaluation: true,
		DisableRetries:  true,
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
		t.Fatal(err)
	}
	defer provider.Shutdown()

	if d := provider.BooleanEvaluation(context.Background(), "f", false, nil); !reflect.DeepEqual(d.FlagMetadata, wantMetadata) {
		t.Fatalf("f FlagMetadata = %#v", d.FlagMetadata)
	}
	if d := provider.BooleanEvaluation(context.Background(), "g", false, nil); d.FlagMetadata == nil || len(d.FlagMetadata) != 0 {
		t.Fatalf("g FlagMetadata = %#v", d.FlagMetadata)
	}
}
