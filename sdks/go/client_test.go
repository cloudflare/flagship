package flagship

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

func TestNewClientRequiresAppIDOrEndpoint(t *testing.T) {
	_, err := NewClient(Options{})
	if err == nil || !strings.Contains(err.Error(), `"appID" or "endpoint"`) {
		t.Fatalf("err = %v", err)
	}
}

func TestNewClientRejectsBothAppIDAndEndpoint(t *testing.T) {
	_, err := NewClient(Options{AppID: "a", AccountID: "b", Endpoint: "https://example.com/evaluate"})
	if err == nil || !strings.Contains(err.Error(), "not both") {
		t.Fatalf("err = %v", err)
	}
}

func TestNewClientRejectsAppIDWithoutAccountID(t *testing.T) {
	_, err := NewClient(Options{AppID: "a"})
	if err == nil || !strings.Contains(err.Error(), `"accountID"`) {
		t.Fatalf("err = %v", err)
	}
}

func TestNewClientRejectsInvalidEndpoint(t *testing.T) {
	_, err := NewClient(Options{Endpoint: "not-a-url"})
	if err == nil || !strings.Contains(err.Error(), "invalid endpoint URL") {
		t.Fatalf("err = %v", err)
	}
}

func TestNewClientBuildsDefaultEndpoint(t *testing.T) {
	c, err := NewClient(Options{AppID: "app-123", AccountID: "acct-456"})
	if err != nil {
		t.Fatal(err)
	}
	want := DefaultBaseURL + "/client/v4/accounts/acct-456/flagship/apps/app-123/evaluate"
	if c.Endpoint() != want {
		t.Fatalf("endpoint = %q, want %q", c.Endpoint(), want)
	}
}

func TestNewClientURLEncodesPathSegments(t *testing.T) {
	c, err := NewClient(Options{AppID: "my app", AccountID: "a/b"})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(c.Endpoint(), "accounts/a%2Fb/flagship/apps/my%20app/evaluate") {
		t.Fatalf("endpoint = %q", c.Endpoint())
	}
}

func TestNewClientStripsTrailingSlashesOnBaseURL(t *testing.T) {
	c, err := NewClient(Options{AppID: "a", AccountID: "b", BaseURL: "https://example.com///"})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(c.Endpoint(), "https://example.com/client/v4/") {
		t.Fatalf("endpoint = %q", c.Endpoint())
	}
}

func TestEvaluateSerializesContextIntoQuery(t *testing.T) {
	var gotQuery url.Values
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotQuery = r.URL.Query()
		writeEvaluationResponse(w, true, "on", "TARGETING_MATCH")
	}))
	defer server.Close()

	c := newTestClient(t, server.URL)
	_, err := c.Evaluate(
		context.Background(),
		"dark-mode",
		openfeature.NewEvaluationContext("u1", map[string]any{"plan": "premium"}),
	)
	if err != nil {
		t.Fatal(err)
	}

	if gotQuery.Get("flagKey") != "dark-mode" || gotQuery.Get("targetingKey") != "u1" || gotQuery.Get("plan") != "premium" {
		t.Fatalf("query = %v", gotQuery)
	}
}

func TestEvaluateReturnsResponse(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		writeEvaluationResponse(w, true, "on", "TARGETING_MATCH")
	}))
	defer server.Close()

	result, err := newTestClient(t, server.URL).EvaluateFlat(context.Background(), "k", nil)
	if err != nil {
		t.Fatal(err)
	}
	if result.FlagKey != "k" || result.Value != true || result.Variant != "on" || result.Reason != ReasonTargetingMatch {
		t.Fatalf("result = %#v", result)
	}
}

func TestEvaluate404ReturnsFlagNotFound(t *testing.T) {
	server := statusServer(http.StatusNotFound)
	defer server.Close()
	_, err := newTestClient(t, server.URL).EvaluateFlat(context.Background(), "k", nil)
	requireFlagshipErrorCode(t, err, ErrorCodeFlagNotFound)
}

func TestEvaluate400ReturnsBadRequest(t *testing.T) {
	server := statusServer(http.StatusBadRequest)
	defer server.Close()
	_, err := newTestClient(t, server.URL).EvaluateFlat(context.Background(), "k", nil)
	requireFlagshipErrorCode(t, err, ErrorCodeBadRequest)
}

func TestEvaluate500ReturnsGeneral(t *testing.T) {
	server := statusServer(http.StatusInternalServerError)
	defer server.Close()
	_, err := newTestClient(t, server.URL).EvaluateFlat(context.Background(), "k", nil)
	requireFlagshipErrorCode(t, err, ErrorCodeGeneral)
}

func TestEvaluateInvalidJSONReturnsParseError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("not json{"))
	}))
	defer server.Close()
	_, err := newTestClient(t, server.URL).EvaluateFlat(context.Background(), "k", nil)
	requireFlagshipErrorCode(t, err, ErrorCodeParse)
}

func TestEvaluateMissingFieldsReturnsParseError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte(`{"flagKey":"k"}`))
	}))
	defer server.Close()
	_, err := newTestClient(t, server.URL).EvaluateFlat(context.Background(), "k", nil)
	requireFlagshipErrorCode(t, err, ErrorCodeParse)
}

func TestEvaluateNetworkErrorReturnsNetworkError(t *testing.T) {
	client, err := NewClient(Options{
		Endpoint:       "https://api.example.com/evaluate",
		HTTPClient:     &http.Client{Transport: roundTripFunc(func(*http.Request) (*http.Response, error) { return nil, errors.New("boom") })},
		DisableRetries: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.EvaluateFlat(context.Background(), "k", nil)
	requireFlagshipErrorCode(t, err, ErrorCodeNetwork)
}

func TestEvaluateTimeoutReturnsTimeoutError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(50 * time.Millisecond)
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	client, err := NewClient(Options{Endpoint: server.URL, Timeout: time.Millisecond, DisableRetries: true})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.EvaluateFlat(context.Background(), "k", nil)
	requireFlagshipErrorCode(t, err, ErrorCodeTimeout)
}

func TestEvaluateInvalidContextReturnsBeforeFetch(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	_, err := newTestClient(t, server.URL).EvaluateFlat(context.Background(), "k", openfeature.FlattenedContext{"obj": map[string]any{"x": 1}})
	requireFlagshipErrorCode(t, err, ErrorCodeInvalidContext)
	if calls.Load() != 0 {
		t.Fatalf("server calls = %d, want 0", calls.Load())
	}
}

func TestAuthTokenSetsBearerHeader(t *testing.T) {
	var authorization string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		authorization = r.Header.Get("Authorization")
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	client, err := NewClient(Options{Endpoint: server.URL, AuthToken: "secret", DisableRetries: true})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.EvaluateFlat(context.Background(), "k", nil)
	if err != nil {
		t.Fatal(err)
	}
	if authorization != "Bearer secret" {
		t.Fatalf("authorization = %q", authorization)
	}
}

func TestHeadersFactoryIsInvokedPerRequest(t *testing.T) {
	var calls atomic.Int32
	var customHeader string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		customHeader = r.Header.Get("X-Custom")
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	client, err := NewClient(Options{
		Endpoint: server.URL,
		HeadersFactory: func(context.Context) (http.Header, error) {
			calls.Add(1)
			return http.Header{"X-Custom": []string{"value"}}, nil
		},
		DisableRetries: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	_, _ = client.EvaluateFlat(context.Background(), "k", nil)
	_, _ = client.EvaluateFlat(context.Background(), "k", nil)

	if calls.Load() != 2 || customHeader != "value" {
		t.Fatalf("calls = %d, custom = %q", calls.Load(), customHeader)
	}
}

func TestHeadersFactoryMergedWithAuthToken(t *testing.T) {
	var got http.Header
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		got = r.Header.Clone()
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	client, err := NewClient(Options{
		Endpoint:       server.URL,
		AuthToken:      "secret",
		HeadersFactory: func(context.Context) (http.Header, error) { return http.Header{"X-Custom": []string{"value"}}, nil },
		DisableRetries: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.EvaluateFlat(context.Background(), "k", nil)
	if err != nil {
		t.Fatal(err)
	}
	if got.Get("Authorization") != "Bearer secret" || got.Get("X-Custom") != "value" {
		t.Fatalf("headers = %#v", got)
	}
}

func TestHeadersFactoryOverridesAuthToken(t *testing.T) {
	var authorization string
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		authorization = r.Header.Get("Authorization")
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	client, err := NewClient(Options{
		Endpoint:  server.URL,
		AuthToken: "secret",
		HeadersFactory: func(context.Context) (http.Header, error) {
			return http.Header{"Authorization": []string{"Basic abc"}}, nil
		},
		DisableRetries: true,
	})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.EvaluateFlat(context.Background(), "k", nil)
	if err != nil {
		t.Fatal(err)
	}
	if authorization != "Basic abc" {
		t.Fatalf("authorization = %q", authorization)
	}
}

func TestRetriesOnTransientError(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if calls.Add(1) == 1 {
			http.Error(w, "server error", http.StatusInternalServerError)
			return
		}
		writeEvaluationResponse(w, true, "on", "DEFAULT")
	}))
	defer server.Close()

	client, err := NewClient(Options{Endpoint: server.URL, Retries: 1, RetryDelay: 0})
	if err != nil {
		t.Fatal(err)
	}
	result, err := client.EvaluateFlat(context.Background(), "k", nil)
	if err != nil {
		t.Fatal(err)
	}
	if result.Value != true || calls.Load() != 2 {
		t.Fatalf("result = %#v calls = %d", result, calls.Load())
	}
}

func Test404IsNotRetried(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		http.NotFound(w, r)
	}))
	defer server.Close()

	client, err := NewClient(Options{Endpoint: server.URL, Retries: 2, RetryDelay: 0})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.EvaluateFlat(context.Background(), "k", nil)
	requireFlagshipErrorCode(t, err, ErrorCodeFlagNotFound)
	if calls.Load() != 1 {
		t.Fatalf("calls = %d, want 1", calls.Load())
	}
}

func Test400IsNotRetried(t *testing.T) {
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		http.Error(w, "bad request", http.StatusBadRequest)
	}))
	defer server.Close()

	client, err := NewClient(Options{Endpoint: server.URL, Retries: 2, RetryDelay: 0})
	if err != nil {
		t.Fatal(err)
	}
	_, err = client.EvaluateFlat(context.Background(), "k", nil)
	requireFlagshipErrorCode(t, err, ErrorCodeBadRequest)
	if calls.Load() != 1 {
		t.Fatalf("calls = %d, want 1", calls.Load())
	}
}

func TestRetriesCappedAtMax(t *testing.T) {
	c, err := NewClient(Options{AppID: "a", AccountID: "b", Retries: 999})
	if err != nil {
		t.Fatal(err)
	}
	if c.Retries() != 10 {
		t.Fatalf("retries = %d, want 10", c.Retries())
	}
}

func TestRetryDelayCappedAtMax(t *testing.T) {
	c, err := NewClient(Options{AppID: "a", AccountID: "b", RetryDelay: 999 * time.Second})
	if err != nil {
		t.Fatal(err)
	}
	if c.RetryDelay() != 30*time.Second {
		t.Fatalf("retry delay = %s", c.RetryDelay())
	}
}

func newTestClient(t *testing.T, endpoint string) *FlagshipClient {
	t.Helper()
	c, err := NewClient(Options{Endpoint: endpoint, DisableRetries: true})
	if err != nil {
		t.Fatal(err)
	}
	return c
}

func statusServer(status int) *httptest.Server {
	return httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		http.Error(w, http.StatusText(status), status)
	}))
}

func writeEvaluationResponse(w http.ResponseWriter, value any, variant string, reason string) {
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write([]byte(`{"flagKey":"k","value":` + jsonLiteral(value) + `,"variant":"` + variant + `","reason":"` + reason + `"}`))
}

func jsonLiteral(value any) string {
	b, err := json.Marshal(value)
	if err != nil {
		panic(err)
	}
	return string(b)
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) {
	return f(r)
}

func requireFlagshipErrorCode(t *testing.T, err error, code ErrorCode) {
	t.Helper()
	var flagshipErr *Error
	if !errors.As(err, &flagshipErr) {
		t.Fatalf("err = %T %v, want *Error", err, err)
	}
	if flagshipErr.Code != code {
		t.Fatalf("error code = %s, want %s (err: %v)", flagshipErr.Code, code, err)
	}
}
