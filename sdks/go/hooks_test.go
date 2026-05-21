package flagship

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

func TestLoggingHookUsesCustomLogger(t *testing.T) {
	logger := &captureLogger{}
	hook := NewLoggingHook(logger)
	_, err := hook.Before(context.Background(), testHookContext("k"), openfeature.NewHookHints(nil))
	if err != nil {
		t.Fatal(err)
	}
	if logger.Len() != 1 || !containsLog(logger, "Evaluating") {
		t.Fatalf("logs = %q", logger.String())
	}
}

func TestLoggingHookAfterLogsValue(t *testing.T) {
	logger := &captureLogger{}
	hook := NewLoggingHook(logger)
	err := hook.After(context.Background(), testHookContext("k"), testEvaluationDetails(true), openfeature.NewHookHints(nil))
	if err != nil {
		t.Fatal(err)
	}
	if logger.Len() != 1 || !containsLog(logger, "evaluated") {
		t.Fatalf("logs = %q", logger.String())
	}
}

func TestLoggingHookErrorLogsMessage(t *testing.T) {
	logger := &captureLogger{}
	hook := NewLoggingHook(logger)
	hook.Error(context.Background(), testHookContext("k"), errors.New("boom"), openfeature.NewHookHints(nil))
	if logger.Len() != 1 || !containsLog(logger, "boom") {
		t.Fatalf("logs = %q", logger.String())
	}
}

func TestTelemetryHookBeforeAfterEmitsEvaluationEvent(t *testing.T) {
	var events []TelemetryEvent
	hook := NewTelemetryHook(func(event TelemetryEvent) {
		events = append(events, event)
	})
	ctx := testHookContext("k")
	hints := openfeature.NewHookHints(map[string]any{"trace_id": "abc"})

	_, err := hook.Before(context.Background(), ctx, hints)
	if err != nil {
		t.Fatal(err)
	}
	time.Sleep(time.Millisecond)
	err = hook.After(context.Background(), ctx, testEvaluationDetails(true), openfeature.NewHookHints(nil))
	if err != nil {
		t.Fatal(err)
	}

	if len(events) != 1 {
		t.Fatalf("events = %#v", events)
	}
	event := events[0]
	if event.Type != TelemetryEventEvaluation || event.FlagKey != "k" || event.Value != true || event.Duration == nil || *event.Duration < 0 {
		t.Fatalf("event = %#v", event)
	}
	if event.Hints == nil || event.Hints.Value("trace_id") != "abc" {
		t.Fatalf("hints = %#v", event.Hints)
	}
}

func TestTelemetryHookErrorEmitsErrorEvent(t *testing.T) {
	var events []TelemetryEvent
	hook := NewTelemetryHook(func(event TelemetryEvent) {
		events = append(events, event)
	})
	ctx := testHookContext("k")
	_, _ = hook.Before(context.Background(), ctx, openfeature.NewHookHints(nil))
	hook.Error(context.Background(), ctx, errors.New("boom"), openfeature.NewHookHints(nil))

	if len(events) != 1 || events[0].Type != TelemetryEventError || events[0].ErrorMessage != "boom" {
		t.Fatalf("events = %#v", events)
	}
}

func TestTelemetryHookAfterWithoutBeforeHasNoDuration(t *testing.T) {
	var events []TelemetryEvent
	hook := NewTelemetryHook(func(event TelemetryEvent) {
		events = append(events, event)
	})
	err := hook.After(context.Background(), testHookContext("k"), testEvaluationDetails(true), openfeature.NewHookHints(nil))
	if err != nil {
		t.Fatal(err)
	}
	if len(events) != 1 || events[0].Duration != nil {
		t.Fatalf("events = %#v", events)
	}
}

func TestTelemetryHookConcurrentEvaluationsTrackedIndependently(t *testing.T) {
	var events []TelemetryEvent
	hook := NewTelemetryHook(func(event TelemetryEvent) {
		events = append(events, event)
	})
	a := testHookContext("flagA")
	b := testHookContext("flagB")

	_, _ = hook.Before(context.Background(), a, openfeature.NewHookHints(map[string]any{"id": "A"}))
	_, _ = hook.Before(context.Background(), b, openfeature.NewHookHints(map[string]any{"id": "B"}))
	_ = hook.After(context.Background(), a, testEvaluationDetails(1), openfeature.NewHookHints(nil))
	_ = hook.After(context.Background(), b, testEvaluationDetails(2), openfeature.NewHookHints(nil))

	if len(events) != 2 {
		t.Fatalf("events = %#v", events)
	}
	byKey := map[string]TelemetryEvent{}
	for _, event := range events {
		byKey[event.FlagKey] = event
	}
	if byKey["flagA"].Hints.Value("id") != "A" || byKey["flagB"].Hints.Value("id") != "B" {
		t.Fatalf("events = %#v", events)
	}
}

func TestTelemetryHookErrorCodePropagated(t *testing.T) {
	var events []TelemetryEvent
	hook := NewTelemetryHook(func(event TelemetryEvent) {
		events = append(events, event)
	})
	ctx := testHookContext("k")
	_, _ = hook.Before(context.Background(), ctx, openfeature.NewHookHints(nil))
	_ = hook.After(
		context.Background(),
		ctx,
		testEvaluationDetails(false, openfeature.TypeMismatchCode),
		openfeature.NewHookHints(nil),
	)
	if events[0].ErrorCode != openfeature.TypeMismatchCode {
		t.Fatalf("events = %#v", events)
	}
}

func testHookContext(flagKey string) openfeature.HookContext {
	return openfeature.NewHookContext(
		flagKey,
		openfeature.Boolean,
		false,
		openfeature.NewClientMetadata("test-client"),
		openfeature.Metadata{Name: "test-provider"},
		openfeature.NewEvaluationContext("u", map[string]any{"plan": "free"}),
	)
}

func testEvaluationDetails(value any, errorCodes ...openfeature.ErrorCode) openfeature.InterfaceEvaluationDetails {
	var errorCode openfeature.ErrorCode
	if len(errorCodes) > 0 {
		errorCode = errorCodes[0]
	}
	return openfeature.InterfaceEvaluationDetails{
		Value: value,
		EvaluationDetails: openfeature.EvaluationDetails{
			FlagKey:  "k",
			FlagType: openfeature.Boolean,
			ResolutionDetail: openfeature.ResolutionDetail{
				Reason:    openfeature.TargetingMatchReason,
				Variant:   "on",
				ErrorCode: errorCode,
			},
		},
	}
}

func containsLog(logger *captureLogger, want string) bool {
	return strings.Contains(logger.String(), want)
}
