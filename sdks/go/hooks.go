package flagship

import (
	"context"
	"sync"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

var (
	_ openfeature.Hook = (*LoggingHook)(nil)
	_ openfeature.Hook = (*TelemetryHook)(nil)
)

// LoggingHook logs OpenFeature evaluation lifecycle events.
type LoggingHook struct {
	logger Logger
}

// NewLoggingHook creates a hook that logs to logger. When logger is nil,
// slog.Default() is used.
func NewLoggingHook(logger Logger) *LoggingHook {
	return &LoggingHook{logger: resolveLogger(logger)}
}

// Before logs the start of an evaluation.
func (h *LoggingHook) Before(ctx context.Context, hookContext openfeature.HookContext, _ openfeature.HookHints) (*openfeature.EvaluationContext, error) {
	h.logger.InfoContext(
		ctx,
		"Evaluating Flagship flag",
		"flag", hookContext.FlagKey(),
		"defaultValue", hookContext.DefaultValue(),
		"context", hookContext.EvaluationContext().Attributes(),
	)
	return nil, nil
}

// After logs a completed evaluation.
func (h *LoggingHook) After(ctx context.Context, hookContext openfeature.HookContext, details openfeature.InterfaceEvaluationDetails, _ openfeature.HookHints) error {
	h.logger.InfoContext(
		ctx,
		"Flagship flag evaluated",
		"flag", hookContext.FlagKey(),
		"value", details.Value,
		"reason", details.Reason,
		"variant", details.Variant,
		"errorCode", details.ErrorCode,
	)
	return nil
}

// Error logs a failed evaluation.
func (h *LoggingHook) Error(ctx context.Context, hookContext openfeature.HookContext, err error, _ openfeature.HookHints) {
	h.logger.ErrorContext(ctx, "Error evaluating Flagship flag", "flag", hookContext.FlagKey(), "error", err)
}

// Finally is a no-op.
func (h *LoggingHook) Finally(context.Context, openfeature.HookContext, openfeature.InterfaceEvaluationDetails, openfeature.HookHints) {
}

// TelemetryEventType identifies the kind of telemetry event.
type TelemetryEventType string

const (
	TelemetryEventEvaluation TelemetryEventType = "evaluation"
	TelemetryEventError      TelemetryEventType = "error"
)

// TelemetryEvent describes one OpenFeature evaluation lifecycle result.
type TelemetryEvent struct {
	Type         TelemetryEventType
	FlagKey      string
	Timestamp    time.Time
	Duration     *time.Duration
	Value        any
	Reason       openfeature.Reason
	Variant      string
	ErrorCode    openfeature.ErrorCode
	ErrorMessage string
	Context      openfeature.EvaluationContext
	Hints        *openfeature.HookHints
}

// TelemetryHook emits a TelemetryEvent for every flag evaluation.
type TelemetryHook struct {
	onEvent func(TelemetryEvent)

	mu      sync.Mutex
	records map[telemetryKey][]telemetryRecord
}

type telemetryKey struct {
	flagKey  string
	flagType openfeature.Type
}

type telemetryRecord struct {
	start time.Time
	hints openfeature.HookHints
}

// NewTelemetryHook creates a telemetry hook. onEvent is called synchronously
// from hook stages.
func NewTelemetryHook(onEvent func(TelemetryEvent)) *TelemetryHook {
	if onEvent == nil {
		onEvent = func(TelemetryEvent) {}
	}
	return &TelemetryHook{
		onEvent: onEvent,
		records: map[telemetryKey][]telemetryRecord{},
	}
}

// Before starts duration tracking for an evaluation.
func (h *TelemetryHook) Before(_ context.Context, hookContext openfeature.HookContext, hookHints openfeature.HookHints) (*openfeature.EvaluationContext, error) {
	key := newTelemetryKey(hookContext)
	h.mu.Lock()
	defer h.mu.Unlock()
	h.records[key] = append(h.records[key], telemetryRecord{start: time.Now(), hints: hookHints})
	return nil, nil
}

// After emits an evaluation event.
func (h *TelemetryHook) After(_ context.Context, hookContext openfeature.HookContext, details openfeature.InterfaceEvaluationDetails, _ openfeature.HookHints) error {
	record, ok := h.popRecord(hookContext)

	var duration *time.Duration
	var hints *openfeature.HookHints
	if ok {
		elapsed := time.Since(record.start)
		duration = &elapsed
		hints = &record.hints
	}

	h.onEvent(TelemetryEvent{
		Type:      TelemetryEventEvaluation,
		FlagKey:   hookContext.FlagKey(),
		Timestamp: time.Now(),
		Duration:  duration,
		Value:     details.Value,
		Reason:    details.Reason,
		Variant:   details.Variant,
		ErrorCode: details.ErrorCode,
		Context:   hookContext.EvaluationContext(),
		Hints:     hints,
	})
	return nil
}

// Error emits an error event.
func (h *TelemetryHook) Error(_ context.Context, hookContext openfeature.HookContext, err error, _ openfeature.HookHints) {
	record, ok := h.popRecord(hookContext)

	var duration *time.Duration
	var hints *openfeature.HookHints
	if ok {
		elapsed := time.Since(record.start)
		duration = &elapsed
		hints = &record.hints
	}

	errorMessage := ""
	if err != nil {
		errorMessage = err.Error()
	}

	h.onEvent(TelemetryEvent{
		Type:         TelemetryEventError,
		FlagKey:      hookContext.FlagKey(),
		Timestamp:    time.Now(),
		Duration:     duration,
		ErrorMessage: errorMessage,
		Context:      hookContext.EvaluationContext(),
		Hints:        hints,
	})
}

// Finally is a no-op. Duration records are removed by After and Error.
func (h *TelemetryHook) Finally(_ context.Context, hookContext openfeature.HookContext, _ openfeature.InterfaceEvaluationDetails, _ openfeature.HookHints) {
	_ = hookContext
}

func (h *TelemetryHook) popRecord(hookContext openfeature.HookContext) (telemetryRecord, bool) {
	key := newTelemetryKey(hookContext)
	h.mu.Lock()
	defer h.mu.Unlock()

	records := h.records[key]
	if len(records) == 0 {
		return telemetryRecord{}, false
	}
	record := records[0]
	if len(records) == 1 {
		delete(h.records, key)
	} else {
		h.records[key] = records[1:]
	}
	return record, true
}

func newTelemetryKey(hookContext openfeature.HookContext) telemetryKey {
	return telemetryKey{flagKey: hookContext.FlagKey(), flagType: hookContext.FlagType()}
}
