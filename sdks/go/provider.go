package flagship

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"

	"github.com/open-feature/go-sdk/openfeature"
)

var (
	_ openfeature.FeatureProvider          = (*ServerProvider)(nil)
	_ openfeature.ContextAwareStateHandler = (*ServerProvider)(nil)
	_ openfeature.EventHandler             = (*ServerProvider)(nil)
)

// FlagshipServerProvider is an alias for ServerProvider kept for parity with
// the TypeScript and Python SDK names.
type FlagshipServerProvider = ServerProvider

// ServerProvider is the OpenFeature provider for Cloudflare Flagship.
type ServerProvider struct {
	client *FlagshipClient
	hooks  []openfeature.Hook

	logging bool
	logger  Logger

	mu     sync.RWMutex
	status openfeature.State
	events chan openfeature.Event
}

// NewProvider constructs a Flagship OpenFeature provider.
func NewProvider(options Options) (*ServerProvider, error) {
	client, err := NewClient(options)
	if err != nil {
		return nil, err
	}
	return &ServerProvider{
		client:  client,
		hooks:   append([]openfeature.Hook(nil), options.Hooks...),
		logging: options.Logging,
		logger:  resolveLogger(options.Logger),
		status:  openfeature.NotReadyState,
		events:  make(chan openfeature.Event, 5),
	}, nil
}

// NewServerProvider constructs a Flagship OpenFeature provider.
func NewServerProvider(options Options) (*ServerProvider, error) {
	return NewProvider(options)
}

// Metadata returns the provider metadata required by OpenFeature.
func (p *ServerProvider) Metadata() openfeature.Metadata {
	return openfeature.Metadata{Name: "Flagship Server Provider"}
}

// Hooks returns provider-level hooks.
func (p *ServerProvider) Hooks() []openfeature.Hook {
	return append([]openfeature.Hook(nil), p.hooks...)
}

// Init initializes the provider without a cancellation-aware context.
func (p *ServerProvider) Init(openfeature.EvaluationContext) error {
	return p.InitWithContext(context.Background(), openfeature.NewTargetlessEvaluationContext(nil))
}

// InitWithContext initializes the provider by probing the evaluation endpoint.
// A 404 means the endpoint is reachable and the health-check flag is absent,
// so it is treated as READY.
func (p *ServerProvider) InitWithContext(ctx context.Context, _ openfeature.EvaluationContext) error {
	_, err := p.client.EvaluateFlat(ctx, healthCheckFlag, openfeature.FlattenedContext{})
	if err != nil {
		if flagshipErr, ok := asFlagshipError(err); ok && flagshipErr.Code == ErrorCodeFlagNotFound {
			p.setStatus(openfeature.ReadyState)
			p.emit(openfeature.ProviderReady, openfeature.ProviderEventDetails{})
			return nil
		}

		p.setStatus(openfeature.ErrorState)
		p.emit(openfeature.ProviderError, openfeature.ProviderEventDetails{
			Message:   err.Error(),
			ErrorCode: providerErrorCode(err),
		})
		if p.logging {
			p.logger.ErrorContext(ctx, "Flagship initialization failed", "error", err)
		}
		return err
	}

	p.setStatus(openfeature.ReadyState)
	p.emit(openfeature.ProviderReady, openfeature.ProviderEventDetails{})
	return nil
}

// Shutdown resets provider status to NOT_READY.
func (p *ServerProvider) Shutdown() {
	p.setStatus(openfeature.NotReadyState)
}

// ShutdownWithContext resets provider status to NOT_READY.
func (p *ServerProvider) ShutdownWithContext(context.Context) error {
	p.Shutdown()
	return nil
}

// Status returns the provider status.
func (p *ServerProvider) Status() openfeature.State {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.status
}

// EventChannel returns provider lifecycle events.
func (p *ServerProvider) EventChannel() <-chan openfeature.Event {
	return p.events
}

// BooleanEvaluation evaluates a boolean flag.
func (p *ServerProvider) BooleanEvaluation(ctx context.Context, flag string, defaultValue bool, flatCtx openfeature.FlattenedContext) openfeature.BoolResolutionDetail {
	value, detail := resolveTyped(ctx, p, flag, defaultValue, flatCtx, toBool)
	return openfeature.BoolResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// StringEvaluation evaluates a string flag.
func (p *ServerProvider) StringEvaluation(ctx context.Context, flag string, defaultValue string, flatCtx openfeature.FlattenedContext) openfeature.StringResolutionDetail {
	value, detail := resolveTyped(ctx, p, flag, defaultValue, flatCtx, toString)
	return openfeature.StringResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// FloatEvaluation evaluates a float flag.
func (p *ServerProvider) FloatEvaluation(ctx context.Context, flag string, defaultValue float64, flatCtx openfeature.FlattenedContext) openfeature.FloatResolutionDetail {
	value, detail := resolveTyped(ctx, p, flag, defaultValue, flatCtx, toFloat64)
	return openfeature.FloatResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// IntEvaluation evaluates an integer flag.
func (p *ServerProvider) IntEvaluation(ctx context.Context, flag string, defaultValue int64, flatCtx openfeature.FlattenedContext) openfeature.IntResolutionDetail {
	value, detail := resolveTyped(ctx, p, flag, defaultValue, flatCtx, toInt64)
	return openfeature.IntResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// ObjectEvaluation evaluates an object flag.
func (p *ServerProvider) ObjectEvaluation(ctx context.Context, flag string, defaultValue any, flatCtx openfeature.FlattenedContext) openfeature.InterfaceResolutionDetail {
	value, detail := resolveTyped(ctx, p, flag, defaultValue, flatCtx, toObject)
	return openfeature.InterfaceResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

func resolveTyped[T any](
	ctx context.Context,
	p *ServerProvider,
	flag string,
	defaultValue T,
	flatCtx openfeature.FlattenedContext,
	convert func(any) (T, error),
) (T, openfeature.ProviderResolutionDetail) {
	if p.logging {
		p.logger.DebugContext(ctx, "Evaluating Flagship flag", "flag", flag)
	}

	result, err := p.client.EvaluateFlat(ctx, flag, flatCtx)
	if err != nil {
		if p.logging {
			p.logger.ErrorContext(ctx, "Flagship flag evaluation failed", "flag", flag, "error", err)
		}
		return defaultValue, openfeature.ProviderResolutionDetail{
			Reason:          openfeature.ErrorReason,
			ResolutionError: resolutionError(err),
			FlagMetadata:    openfeature.FlagMetadata{},
		}
	}

	if result.Reason == ReasonDisabled {
		return defaultValue, openfeature.ProviderResolutionDetail{
			Reason:       openfeature.DisabledReason,
			FlagMetadata: openfeature.FlagMetadata{},
		}
	}

	value, err := convert(result.Value)
	if err != nil {
		if p.logging {
			p.logger.WarnContext(ctx, "Flagship flag type mismatch", "flag", flag, "error", err)
		}
		return defaultValue, openfeature.ProviderResolutionDetail{
			Reason:          openfeature.ErrorReason,
			ResolutionError: openfeature.NewTypeMismatchResolutionError(err.Error()),
			FlagMetadata:    openfeature.FlagMetadata{},
		}
	}

	if p.logging {
		p.logger.DebugContext(ctx, "Flagship flag resolved", "flag", flag, "value", value, "reason", result.Reason, "variant", result.Variant)
	}

	return value, openfeature.ProviderResolutionDetail{
		Reason:       mapReason(result.Reason),
		Variant:      result.Variant,
		FlagMetadata: openfeature.FlagMetadata{},
	}
}

func (p *ServerProvider) setStatus(status openfeature.State) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.status = status
}

func (p *ServerProvider) emit(eventType openfeature.EventType, details openfeature.ProviderEventDetails) {
	event := openfeature.Event{
		ProviderName:         p.Metadata().Name,
		EventType:            eventType,
		ProviderEventDetails: details,
	}
	select {
	case p.events <- event:
	default:
	}
}

func mapReason(reason EvaluationReason) openfeature.Reason {
	switch reason {
	case ReasonTargetingMatch:
		return openfeature.TargetingMatchReason
	case ReasonSplit:
		return openfeature.SplitReason
	case ReasonDisabled:
		return openfeature.DisabledReason
	case ReasonDefault:
		return openfeature.DefaultReason
	default:
		return openfeature.Reason(reason)
	}
}

func providerErrorCode(err error) openfeature.ErrorCode {
	if flagshipErr, ok := asFlagshipError(err); ok {
		switch flagshipErr.Code {
		case ErrorCodeFlagNotFound:
			return openfeature.FlagNotFoundCode
		case ErrorCodeInvalidContext:
			return openfeature.InvalidContextCode
		case ErrorCodeParse:
			return openfeature.ParseErrorCode
		default:
			return openfeature.GeneralCode
		}
	}
	return openfeature.GeneralCode
}

func resolutionError(err error) openfeature.ResolutionError {
	if flagshipErr, ok := asFlagshipError(err); ok {
		switch flagshipErr.Code {
		case ErrorCodeFlagNotFound:
			return openfeature.NewFlagNotFoundResolutionError(flagshipErr.Error())
		case ErrorCodeInvalidContext:
			return openfeature.NewInvalidContextResolutionError(flagshipErr.Error())
		case ErrorCodeParse:
			return openfeature.NewParseErrorResolutionError(flagshipErr.Error(), err)
		default:
			return openfeature.NewGeneralResolutionError(flagshipErr.Error(), err)
		}
	}
	return openfeature.NewGeneralResolutionError(err.Error(), err)
}

func toBool(value any) (bool, error) {
	v, ok := value.(bool)
	if !ok {
		return false, fmt.Errorf("expected boolean, got %s", typeName(value))
	}
	return v, nil
}

func toString(value any) (string, error) {
	v, ok := value.(string)
	if !ok {
		return "", fmt.Errorf("expected string, got %s", typeName(value))
	}
	return v, nil
}

func toFloat64(value any) (float64, error) {
	switch v := value.(type) {
	case json.Number:
		f, err := v.Float64()
		if err != nil {
			return 0, fmt.Errorf("expected number, got %s", v.String())
		}
		return f, nil
	case float64:
		return v, nil
	case float32:
		return float64(v), nil
	case int:
		return float64(v), nil
	case int8:
		return float64(v), nil
	case int16:
		return float64(v), nil
	case int32:
		return float64(v), nil
	case int64:
		return float64(v), nil
	case uint:
		return float64(v), nil
	case uint8:
		return float64(v), nil
	case uint16:
		return float64(v), nil
	case uint32:
		return float64(v), nil
	case uint64:
		return float64(v), nil
	default:
		return 0, fmt.Errorf("expected number, got %s", typeName(value))
	}
}

func toInt64(value any) (int64, error) {
	switch v := value.(type) {
	case json.Number:
		i, err := v.Int64()
		if err != nil {
			return 0, fmt.Errorf("expected integer, got number")
		}
		return i, nil
	case int:
		return int64(v), nil
	case int8:
		return int64(v), nil
	case int16:
		return int64(v), nil
	case int32:
		return int64(v), nil
	case int64:
		return v, nil
	default:
		return 0, fmt.Errorf("expected integer, got %s", typeName(value))
	}
}

func toObject(value any) (any, error) {
	switch value.(type) {
	case nil, map[string]any, []any:
		return value, nil
	default:
		return nil, fmt.Errorf("expected object, got %s", typeName(value))
	}
}

func typeName(value any) string {
	switch value.(type) {
	case nil:
		return "null"
	case bool:
		return "boolean"
	case string:
		return "string"
	case json.Number, float32, float64, int, int8, int16, int32, int64, uint, uint8, uint16, uint32, uint64:
		return "number"
	case map[string]any, []any:
		return "object"
	default:
		return fmt.Sprintf("%T", value)
	}
}
