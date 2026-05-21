package flagship

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/open-feature/go-sdk/openfeature"
)

func TestProviderInitializeSucceedsOn200(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "DEFAULT")
	defer server.Close()

	if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
		t.Fatal(err)
	}
	if provider.Status() != openfeature.ReadyState {
		t.Fatalf("status = %s", provider.Status())
	}
}

func TestProviderInitializeTreats404AsReady(t *testing.T) {
	server := statusServer(http.StatusNotFound)
	defer server.Close()
	provider := newTestProvider(t, server.URL, Options{DisableRetries: true})

	if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err != nil {
		t.Fatal(err)
	}
	if provider.Status() != openfeature.ReadyState {
		t.Fatalf("status = %s", provider.Status())
	}
}

func TestProviderInitializeFailureSetsErrorStatus(t *testing.T) {
	server := statusServer(http.StatusInternalServerError)
	defer server.Close()
	provider := newTestProvider(t, server.URL, Options{DisableRetries: true})

	if err := provider.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil)); err == nil {
		t.Fatal("expected init error")
	}
	if provider.Status() != openfeature.ErrorState {
		t.Fatalf("status = %s", provider.Status())
	}
}

func TestProviderShutdownResetsStatus(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "DEFAULT")
	defer server.Close()
	provider.setStatus(openfeature.ReadyState)
	provider.Shutdown()
	if provider.Status() != openfeature.NotReadyState {
		t.Fatalf("status = %s", provider.Status())
	}
}

func TestProviderMetadataName(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "DEFAULT")
	defer server.Close()
	if provider.Metadata().Name != "Flagship Server Provider" {
		t.Fatalf("metadata = %#v", provider.Metadata())
	}
}

func TestProviderHooksDefaultEmpty(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "DEFAULT")
	defer server.Close()
	if len(provider.Hooks()) != 0 {
		t.Fatalf("hooks = %#v", provider.Hooks())
	}
}

func TestProviderSuccessfulResolution(t *testing.T) {
	tests := []struct {
		name   string
		value  any
		assert func(*ServerProvider)
	}{
		{
			name:  "boolean",
			value: true,
			assert: func(p *ServerProvider) {
				d := p.BooleanEvaluation(context.Background(), "k", false, nil)
				if d.Value != true || d.ResolutionDetail().ErrorCode != "" || d.Variant != "on" || d.Reason != openfeature.TargetingMatchReason {
					t.Fatalf("detail = %#v", d)
				}
			},
		},
		{
			name:  "string",
			value: "v2",
			assert: func(p *ServerProvider) {
				d := p.StringEvaluation(context.Background(), "k", "default", nil)
				if d.Value != "v2" || d.ResolutionDetail().ErrorCode != "" {
					t.Fatalf("detail = %#v", d)
				}
			},
		},
		{
			name:  "int",
			value: 42,
			assert: func(p *ServerProvider) {
				d := p.IntEvaluation(context.Background(), "k", 0, nil)
				if d.Value != 42 || d.ResolutionDetail().ErrorCode != "" {
					t.Fatalf("detail = %#v", d)
				}
			},
		},
		{
			name:  "float",
			value: 1.5,
			assert: func(p *ServerProvider) {
				d := p.FloatEvaluation(context.Background(), "k", 0, nil)
				if d.Value != 1.5 || d.ResolutionDetail().ErrorCode != "" {
					t.Fatalf("detail = %#v", d)
				}
			},
		},
		{
			name:  "object",
			value: map[string]any{"theme": "dark"},
			assert: func(p *ServerProvider) {
				d := p.ObjectEvaluation(context.Background(), "k", map[string]any{}, nil)
				got, ok := d.Value.(map[string]any)
				if !ok || got["theme"] != "dark" || d.ResolutionDetail().ErrorCode != "" {
					t.Fatalf("detail = %#v", d)
				}
			},
		},
		{
			name:  "array",
			value: []any{1, 2, 3},
			assert: func(p *ServerProvider) {
				d := p.ObjectEvaluation(context.Background(), "k", []any{}, nil)
				got, ok := d.Value.([]any)
				if !ok || len(got) != 3 || d.ResolutionDetail().ErrorCode != "" {
					t.Fatalf("detail = %#v", d)
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			provider, server := newProviderWithResponse(t, http.StatusOK, tt.value, "on", "TARGETING_MATCH")
			defer server.Close()
			tt.assert(provider)
		})
	}
}

func TestProviderBooleanRejectsString(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, "not a bool", "on", "DEFAULT")
	defer server.Close()
	d := provider.BooleanEvaluation(context.Background(), "k", false, nil)
	requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.TypeMismatchCode)
}

func TestProviderIntegerRejectsFloat(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, 7.5, "on", "DEFAULT")
	defer server.Close()
	d := provider.IntEvaluation(context.Background(), "k", 0, nil)
	requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.TypeMismatchCode)
}

func TestProviderIntegerRejectsBool(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "DEFAULT")
	defer server.Close()
	d := provider.IntEvaluation(context.Background(), "k", 0, nil)
	requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.TypeMismatchCode)
}

func TestProviderFloatAcceptsInt(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, 7, "on", "DEFAULT")
	defer server.Close()
	d := provider.FloatEvaluation(context.Background(), "k", 0, nil)
	if d.Value != 7 {
		t.Fatalf("value = %v", d.Value)
	}
}

func TestProviderFloatRejectsBool(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "DEFAULT")
	defer server.Close()
	d := provider.FloatEvaluation(context.Background(), "k", 0, nil)
	requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.TypeMismatchCode)
}

func TestProviderObjectRejectsString(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, "string", "on", "DEFAULT")
	defer server.Close()
	d := provider.ObjectEvaluation(context.Background(), "k", map[string]any{}, nil)
	requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.TypeMismatchCode)
}

func TestProviderDisabledReturnsDefaultWithoutError(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "DISABLED")
	defer server.Close()
	d := provider.BooleanEvaluation(context.Background(), "k", false, nil)
	if d.Value != false || d.ResolutionDetail().ErrorCode != "" || d.Reason != openfeature.DisabledReason {
		t.Fatalf("detail = %#v", d)
	}
}

func TestProvider404MapsToFlagNotFound(t *testing.T) {
	server := statusServer(http.StatusNotFound)
	defer server.Close()
	provider := newTestProvider(t, server.URL, Options{DisableRetries: true})
	d := provider.BooleanEvaluation(context.Background(), "k", false, nil)
	requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.FlagNotFoundCode)
}

func TestProvider500MapsToGeneral(t *testing.T) {
	server := statusServer(http.StatusInternalServerError)
	defer server.Close()
	provider := newTestProvider(t, server.URL, Options{DisableRetries: true})
	d := provider.BooleanEvaluation(context.Background(), "k", false, nil)
	requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.GeneralCode)
}

func TestProviderParseError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("not json{"))
	}))
	defer server.Close()
	provider := newTestProvider(t, server.URL, Options{DisableRetries: true})
	d := provider.BooleanEvaluation(context.Background(), "k", false, nil)
	requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.ParseErrorCode)
}

func TestProviderInvalidContext(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "DEFAULT")
	defer server.Close()
	d := provider.BooleanEvaluation(context.Background(), "k", false, openfeature.FlattenedContext{"obj": map[string]any{"x": 1}})
	requireResolutionErrorCode(t, d.ResolutionDetail(), openfeature.InvalidContextCode)
}

func TestProviderLoggingDisabledByDefault(t *testing.T) {
	logger := &captureLogger{}
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "DEFAULT", Options{Logger: logger})
	defer server.Close()
	_ = provider.BooleanEvaluation(context.Background(), "k", false, nil)
	if logger.Len() != 0 {
		t.Fatalf("log records = %d", logger.Len())
	}
}

func TestProviderLoggingEnabledEmitsRecords(t *testing.T) {
	logger := &captureLogger{}
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "DEFAULT", Options{Logger: logger, Logging: true})
	defer server.Close()
	_ = provider.BooleanEvaluation(context.Background(), "k", false, nil)
	if logger.Len() == 0 || !strings.Contains(logger.String(), "k") {
		t.Fatalf("log records = %q", logger.String())
	}
}

func TestOpenFeatureIntegration(t *testing.T) {
	provider, server := newProviderWithResponse(t, http.StatusOK, true, "on", "TARGETING_MATCH")
	defer server.Close()
	defer openfeature.Shutdown()

	domain := "flagship-go-integration"
	if err := openfeature.SetNamedProviderAndWait(domain, provider); err != nil {
		t.Fatal(err)
	}

	client := openfeature.NewClient(domain)
	details, err := client.BooleanValueDetails(
		context.Background(),
		"dark-mode",
		false,
		openfeature.NewEvaluationContext("u1", nil),
	)
	if err != nil {
		t.Fatal(err)
	}
	if details.Value != true || details.Variant != "on" || details.ErrorCode != "" {
		t.Fatalf("details = %#v", details)
	}
}

func TestOpenFeatureFlagNotFoundReturnsDefaultViaAPI(t *testing.T) {
	server := statusServer(http.StatusNotFound)
	defer server.Close()
	provider := newTestProvider(t, server.URL, Options{DisableRetries: true})
	defer openfeature.Shutdown()

	domain := "flagship-go-missing"
	if err := openfeature.SetNamedProviderAndWait(domain, provider); err != nil {
		t.Fatal(err)
	}

	client := openfeature.NewClient(domain)
	details, err := client.BooleanValueDetails(context.Background(), "missing", false, openfeature.NewTargetlessEvaluationContext(nil))
	if err == nil {
		t.Fatal("expected OpenFeature error")
	}
	if details.Value != false || details.ErrorCode != openfeature.FlagNotFoundCode {
		t.Fatalf("details = %#v err = %v", details, err)
	}
}

func newProviderWithResponse(t *testing.T, status int, value any, variant string, reason string, overrides ...Options) (*ServerProvider, *httptest.Server) {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(status)
		if status >= 200 && status < 300 {
			writeEvaluationResponse(w, value, variant, reason)
		}
	}))
	return newTestProvider(t, server.URL, mergeOptions(overrides...)), server
}

func newTestProvider(t *testing.T, endpoint string, options Options) *ServerProvider {
	t.Helper()
	options.Endpoint = endpoint
	if !options.Logging {
		options.DisableRetries = true
	}
	provider, err := NewProvider(options)
	if err != nil {
		t.Fatal(err)
	}
	return provider
}

func mergeOptions(options ...Options) Options {
	var merged Options
	for _, option := range options {
		if option.Endpoint != "" {
			merged.Endpoint = option.Endpoint
		}
		if option.Logger != nil {
			merged.Logger = option.Logger
		}
		if option.Logging {
			merged.Logging = true
		}
		if option.DisableRetries {
			merged.DisableRetries = true
		}
	}
	return merged
}

func requireResolutionErrorCode(t *testing.T, detail openfeature.ResolutionDetail, code openfeature.ErrorCode) {
	t.Helper()
	if detail.ErrorCode != code {
		t.Fatalf("error code = %s, want %s (detail: %#v)", detail.ErrorCode, code, detail)
	}
}

type captureLogger struct {
	mu      sync.Mutex
	records []string
}

func (l *captureLogger) DebugContext(_ context.Context, msg string, args ...any) {
	l.append("debug", msg, args...)
}

func (l *captureLogger) InfoContext(_ context.Context, msg string, args ...any) {
	l.append("info", msg, args...)
}

func (l *captureLogger) WarnContext(_ context.Context, msg string, args ...any) {
	l.append("warn", msg, args...)
}

func (l *captureLogger) ErrorContext(_ context.Context, msg string, args ...any) {
	l.append("error", msg, args...)
}

func (l *captureLogger) append(level string, msg string, args ...any) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.records = append(l.records, level+":"+msg+":"+fmt.Sprint(args...))
}

func (l *captureLogger) Len() int {
	l.mu.Lock()
	defer l.mu.Unlock()
	return len(l.records)
}

func (l *captureLogger) String() string {
	l.mu.Lock()
	defer l.mu.Unlock()
	return strings.Join(l.records, "\n")
}
