package flagship

import (
	"context"
	"encoding/json"
	"fmt"

	"github.com/open-feature/go-sdk/openfeature"
)

var (
	_ openfeature.FeatureProvider          = (*ServerProvider)(nil)
	_ openfeature.ContextAwareStateHandler = (*ServerProvider)(nil)
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

// InitWithContext does not probe the evaluation endpoint. HTTP connectivity
// and authentication errors are reported by individual flag evaluations.
func (p *ServerProvider) InitWithContext(context.Context, openfeature.EvaluationContext) error {
	return nil
}

// Shutdown releases provider resources.
func (p *ServerProvider) Shutdown() {
}

// ShutdownWithContext releases provider resources.
func (p *ServerProvider) ShutdownWithContext(context.Context) error {
	p.Shutdown()
	return nil
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
	number, ok := value.(json.Number)
	if !ok {
		return 0, fmt.Errorf("expected number, got %s", typeName(value))
	}

	result, err := number.Float64()
	if err != nil {
		return 0, fmt.Errorf("expected number, got %s", number.String())
	}
	return result, nil
}

func toInt64(value any) (int64, error) {
	number, ok := value.(json.Number)
	if !ok {
		return 0, fmt.Errorf("expected integer, got %s", typeName(value))
	}

	result, err := number.Int64()
	if err != nil {
		return 0, fmt.Errorf("expected integer, got number")
	}
	return result, nil
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
