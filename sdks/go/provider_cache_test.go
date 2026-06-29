package flagship

import (
	"context"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

func TestProviderCacheDisabledByDefault(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		writeEvaluationResponse(w, true, "on", "TARGETING_MATCH")
	}))
	defer server.Close()

	provider := newTestProvider(t, server.URL, Options{})
	_ = provider.BooleanEvaluation(context.Background(), "k", false, nil)
	_ = provider.BooleanEvaluation(context.Background(), "k", false, nil)

	if calls.Load() != 2 {
		t.Fatalf("calls = %d, want 2", calls.Load())
	}
}

func TestProviderCacheHitServesWithoutRequestAndMarksCached(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		writeEvaluationResponse(w, true, "on", "TARGETING_MATCH")
	}))
	defer server.Close()

	provider := newTestProvider(t, server.URL, Options{CacheTTL: time.Minute})
	first := provider.BooleanEvaluation(context.Background(), "k", false, nil)
	second := provider.BooleanEvaluation(context.Background(), "k", false, nil)

	if first.Value != true || first.Reason != openfeature.TargetingMatchReason {
		t.Fatalf("first = %#v", first)
	}
	if second.Value != true || second.Reason != openfeature.CachedReason || second.Variant != "on" {
		t.Fatalf("second = %#v", second)
	}
	if calls.Load() != 1 {
		t.Fatalf("calls = %d, want 1", calls.Load())
	}
}

func TestProviderCacheKeyIncludesContext(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		writeEvaluationResponse(w, true, "on", "TARGETING_MATCH")
	}))
	defer server.Close()

	provider := newTestProvider(t, server.URL, Options{CacheTTL: time.Minute})
	free := openfeature.FlattenedContext{"plan": "free"}
	enterprise := openfeature.FlattenedContext{"plan": "enterprise"}

	_ = provider.BooleanEvaluation(context.Background(), "k", false, free)
	_ = provider.BooleanEvaluation(context.Background(), "k", false, enterprise)
	third := provider.BooleanEvaluation(context.Background(), "k", false, free)

	if third.Reason != openfeature.CachedReason {
		t.Fatalf("third reason = %s, want CACHED", third.Reason)
	}
	if calls.Load() != 2 {
		t.Fatalf("calls = %d, want 2", calls.Load())
	}
}

func TestProviderCacheKeyIncludesType(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		writeEvaluationResponse(w, 7, "seven", "DEFAULT")
	}))
	defer server.Close()

	provider := newTestProvider(t, server.URL, Options{CacheTTL: time.Minute})
	_ = provider.IntEvaluation(context.Background(), "k", 0, nil)
	_ = provider.FloatEvaluation(context.Background(), "k", 0, nil)
	third := provider.IntEvaluation(context.Background(), "k", 0, nil)

	if third.Reason != openfeature.CachedReason {
		t.Fatalf("third reason = %s, want CACHED", third.Reason)
	}
	if calls.Load() != 2 {
		t.Fatalf("calls = %d, want 2", calls.Load())
	}
}

func TestProviderCacheExpiresAfterTTL(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	provider := newTestProvider(t, server.URL, Options{CacheTTL: 25 * time.Millisecond})
	_ = provider.BooleanEvaluation(context.Background(), "k", false, nil)
	second := provider.BooleanEvaluation(context.Background(), "k", false, nil)
	time.Sleep(50 * time.Millisecond)
	third := provider.BooleanEvaluation(context.Background(), "k", false, nil)

	if second.Reason != openfeature.CachedReason {
		t.Fatalf("second reason = %s, want CACHED", second.Reason)
	}
	if third.Reason == openfeature.CachedReason {
		t.Fatalf("third reason = %s, want non-cached", third.Reason)
	}
	if calls.Load() != 2 {
		t.Fatalf("calls = %d, want 2", calls.Load())
	}
}

func TestProviderCacheEvictsLeastRecentlyUsed(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	provider := newTestProvider(t, server.URL, Options{CacheTTL: time.Minute, CacheMaxSize: 1})
	_ = provider.BooleanEvaluation(context.Background(), "a", false, nil)
	_ = provider.BooleanEvaluation(context.Background(), "b", false, nil)
	third := provider.BooleanEvaluation(context.Background(), "b", false, nil)
	fourth := provider.BooleanEvaluation(context.Background(), "a", false, nil)

	if third.Reason != openfeature.CachedReason {
		t.Fatalf("third reason = %s, want CACHED", third.Reason)
	}
	if fourth.Reason == openfeature.CachedReason {
		t.Fatalf("fourth reason = %s, want non-cached", fourth.Reason)
	}
	if calls.Load() != 3 {
		t.Fatalf("calls = %d, want 3", calls.Load())
	}
}

func TestProviderCacheDoesNotStoreDisabledResults(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		writeEvaluationResponse(w, true, "on", "DISABLED")
	}))
	defer server.Close()

	provider := newTestProvider(t, server.URL, Options{CacheTTL: time.Minute})
	first := provider.BooleanEvaluation(context.Background(), "k", false, nil)
	second := provider.BooleanEvaluation(context.Background(), "k", false, nil)

	if first.Reason != openfeature.DisabledReason || second.Reason != openfeature.DisabledReason {
		t.Fatalf("first = %#v second = %#v", first, second)
	}
	if calls.Load() != 2 {
		t.Fatalf("calls = %d, want 2", calls.Load())
	}
}

func TestProviderCacheDoesNotStoreErrors(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if calls.Add(1) == 1 {
			http.Error(w, "temporary", http.StatusInternalServerError)
			return
		}
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	provider := newTestProvider(t, server.URL, Options{CacheTTL: time.Minute})
	first := provider.BooleanEvaluation(context.Background(), "k", false, nil)
	second := provider.BooleanEvaluation(context.Background(), "k", false, nil)

	requireResolutionErrorCode(t, first.ResolutionDetail(), openfeature.GeneralCode)
	if second.Value != true || second.ResolutionDetail().ErrorCode != "" {
		t.Fatalf("second = %#v", second)
	}
	if calls.Load() != 2 {
		t.Fatalf("calls = %d, want 2", calls.Load())
	}
}

func TestProviderCacheDoesNotStoreTypeMismatches(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if calls.Add(1) == 1 {
			writeEvaluationResponse(w, "not a bool", "bad", "DEFAULT")
			return
		}
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	provider := newTestProvider(t, server.URL, Options{CacheTTL: time.Minute})
	first := provider.BooleanEvaluation(context.Background(), "k", false, nil)
	second := provider.BooleanEvaluation(context.Background(), "k", false, nil)

	requireResolutionErrorCode(t, first.ResolutionDetail(), openfeature.TypeMismatchCode)
	if second.Value != true || second.ResolutionDetail().ErrorCode != "" {
		t.Fatalf("second = %#v", second)
	}
	if calls.Load() != 2 {
		t.Fatalf("calls = %d, want 2", calls.Load())
	}
}

func TestProviderShutdownClearsCache(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	provider := newTestProvider(t, server.URL, Options{CacheTTL: time.Minute})
	_ = provider.BooleanEvaluation(context.Background(), "k", false, nil)
	provider.Shutdown()
	second := provider.BooleanEvaluation(context.Background(), "k", false, nil)

	if second.Reason == openfeature.CachedReason {
		t.Fatalf("second reason = %s, want non-cached", second.Reason)
	}
	if calls.Load() != 2 {
		t.Fatalf("calls = %d, want 2", calls.Load())
	}
}
