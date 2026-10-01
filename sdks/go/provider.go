package flagship

import (
	"context"
	"encoding/json"
	"fmt"
	"strconv"
	"sync"
	"sync/atomic"
	"time"

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
	client          *FlagshipClient
	hooks           []openfeature.Hook
	cache           *responseCache
	accountID       string
	localEvaluation bool
	refreshInterval time.Duration
	logging         bool
	logger          Logger

	// Local-evaluation lifecycle state.
	//
	// snapshot is an immutable pointer swapped atomically on refresh.
	// epoch is bumped on every Init and Shutdown so in-flight refreshes from a
	// previous session cannot apply their result after close or re-init. A
	// boolean "closed" flag is not enough (see overview §7).
	snapshot atomic.Pointer[definitionsSnapshot]
	etag     atomic.Value // string
	epoch    atomic.Uint64

	refreshMu     sync.Mutex
	refreshCancel context.CancelFunc
	refreshWG     sync.WaitGroup
}

const (
	flagTypeBoolean = "boolean"
	flagTypeString  = "string"
	flagTypeFloat   = "float"
	flagTypeInt     = "integer"
	flagTypeObject  = "object"
)

// NewProvider constructs a Flagship OpenFeature provider.
func NewProvider(options Options) (*ServerProvider, error) {
	localEvaluation := options.LocalEvaluation
	refreshInterval := options.RefreshInterval

	if localEvaluation {
		if refreshInterval == 0 {
			refreshInterval = defaultRefreshInterval
		}
		if refreshInterval <= 0 {
			return nil, fmt.Errorf(`Flagship: "refreshInterval" must be a duration greater than 0`)
		}
		if options.CacheTTL > 0 {
			return nil, fmt.Errorf("Flagship: LocalEvaluation is incompatible with CacheTTL — local evaluation has nothing to cache")
		}
		if options.AccountID == "" {
			return nil, fmt.Errorf(`Flagship: "accountID" is required when LocalEvaluation is enabled`)
		}
		// Fail fast if the definitions URL cannot be derived.
		if _, err := resolveDefinitionsEndpoint(options); err != nil {
			return nil, err
		}
	}

	client, err := NewClient(options)
	if err != nil {
		return nil, err
	}

	var cache *responseCache
	if options.CacheTTL > 0 {
		cache = newResponseCache(options.CacheTTL, options.CacheMaxSize)
	}

	provider := &ServerProvider{
		client:          client,
		hooks:           append([]openfeature.Hook(nil), options.Hooks...),
		cache:           cache,
		accountID:       options.AccountID,
		localEvaluation: localEvaluation,
		refreshInterval: refreshInterval,
		logging:         options.Logging,
		logger:          resolveLogger(options.Logger),
	}
	provider.etag.Store("")
	return provider, nil
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

// InitWithContext initializes the provider.
//
// In local evaluation mode, it blocks until the first definitions fetch
// succeeds, then starts a background refresh goroutine. Failure puts the
// OpenFeature provider in ERROR. In HTTP mode this is a no-op: connectivity
// and authentication errors are reported by individual flag evaluations.
func (p *ServerProvider) InitWithContext(ctx context.Context, _ openfeature.EvaluationContext) error {
	if !p.localEvaluation {
		return nil
	}

	// Stop any prior refresh session before starting a new one. Wait for the
	// previous loop to exit so WaitGroup accounting stays balanced, then bump
	// the epoch so in-flight fetches from the previous session cannot apply.
	p.stopRefreshLocked()
	p.refreshWG.Wait()

	epoch := p.epoch.Add(1)
	p.snapshot.Store(nil)
	p.etag.Store("")

	if err := p.fetchAndApplySnapshot(ctx, true, epoch); err != nil {
		return err
	}

	// Only start the refresh loop if this initialize was not superseded by
	// Shutdown/re-Init while the blocking fetch ran.
	if p.epoch.Load() != epoch {
		return nil
	}

	refreshCtx, cancel := context.WithCancel(context.Background())
	p.refreshMu.Lock()
	p.refreshCancel = cancel
	p.refreshWG.Add(1)
	p.refreshMu.Unlock()

	go p.refreshLoop(refreshCtx, epoch)
	return nil
}

// Shutdown releases provider resources.
func (p *ServerProvider) Shutdown() {
	_ = p.ShutdownWithContext(context.Background())
}

// ShutdownWithContext releases provider resources, cancelling the local
// evaluation refresh goroutine and waiting for it to exit (respecting ctx).
func (p *ServerProvider) ShutdownWithContext(ctx context.Context) error {
	if p.localEvaluation {
		p.stopRefreshLocked()
		// Bump the epoch so in-flight fetches observe a stale generation and
		// refuse to write.
		p.epoch.Add(1)
		p.snapshot.Store(nil)
		p.etag.Store("")

		done := make(chan struct{})
		go func() {
			p.refreshWG.Wait()
			close(done)
		}()
		select {
		case <-done:
		case <-ctx.Done():
			// Best-effort: the goroutine is cancelled and will exit; we just
			// couldn't wait out the caller's deadline.
		}
	}
	if p.cache != nil {
		p.cache.clear()
	}
	return nil
}

func (p *ServerProvider) stopRefreshLocked() {
	p.refreshMu.Lock()
	cancel := p.refreshCancel
	p.refreshCancel = nil
	p.refreshMu.Unlock()
	if cancel != nil {
		cancel()
	}
}

func (p *ServerProvider) refreshLoop(ctx context.Context, epoch uint64) {
	defer p.refreshWG.Done()

	ticker := time.NewTicker(p.refreshInterval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			if p.epoch.Load() != epoch {
				return
			}
			// Refresh failures keep the last good snapshot (logged when enabled).
			_ = p.fetchAndApplySnapshot(ctx, false, epoch)
		}
	}
}

func (p *ServerProvider) fetchAndApplySnapshot(ctx context.Context, initial bool, epoch uint64) error {
	currentETag, _ := p.etag.Load().(string)
	result, err := p.client.fetchDefinitions(ctx, currentETag)
	// Stale session (closed or superseded by a newer initialize).
	if p.epoch.Load() != epoch {
		return nil
	}
	if err != nil {
		if initial {
			return err
		}
		if p.logging {
			p.logger.WarnContext(ctx, "Flagship definitions refresh failed; keeping last good snapshot", "error", err)
		}
		return nil
	}
	if result.NotModified {
		if p.logging {
			p.logger.DebugContext(ctx, "Flagship definitions not modified (304)")
		}
		return nil
	}

	// Only validate top-level shape at parse time; individual malformed flags
	// are left in the snapshot and surface PARSE_ERROR at evaluation time.
	snap := newDefinitionsSnapshot(result.ETag, result.Flags)
	if p.epoch.Load() != epoch {
		return nil
	}
	p.snapshot.Store(snap)
	p.etag.Store(result.ETag)
	if result.ETag == "" && p.logging {
		p.logger.DebugContext(ctx, "Flagship definitions response had no ETag; conditional refresh (304) is unavailable")
	}
	if p.logging {
		p.logger.DebugContext(ctx, "Flagship definitions snapshot updated", "flags", len(result.Flags))
	}
	return nil
}

// BooleanEvaluation evaluates a boolean flag.
func (p *ServerProvider) BooleanEvaluation(ctx context.Context, flag string, defaultValue bool, flatCtx openfeature.FlattenedContext) openfeature.BoolResolutionDetail {
	value, detail := resolveTyped(ctx, p, flag, defaultValue, flatCtx, flagTypeBoolean, toBool)
	return openfeature.BoolResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// StringEvaluation evaluates a string flag.
func (p *ServerProvider) StringEvaluation(ctx context.Context, flag string, defaultValue string, flatCtx openfeature.FlattenedContext) openfeature.StringResolutionDetail {
	value, detail := resolveTyped(ctx, p, flag, defaultValue, flatCtx, flagTypeString, toString)
	return openfeature.StringResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// FloatEvaluation evaluates a float flag.
func (p *ServerProvider) FloatEvaluation(ctx context.Context, flag string, defaultValue float64, flatCtx openfeature.FlattenedContext) openfeature.FloatResolutionDetail {
	value, detail := resolveTyped(ctx, p, flag, defaultValue, flatCtx, flagTypeFloat, toFloat64)
	return openfeature.FloatResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// IntEvaluation evaluates an integer flag.
func (p *ServerProvider) IntEvaluation(ctx context.Context, flag string, defaultValue int64, flatCtx openfeature.FlattenedContext) openfeature.IntResolutionDetail {
	value, detail := resolveTyped(ctx, p, flag, defaultValue, flatCtx, flagTypeInt, toInt64)
	return openfeature.IntResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// ObjectEvaluation evaluates an object flag.
func (p *ServerProvider) ObjectEvaluation(ctx context.Context, flag string, defaultValue any, flatCtx openfeature.FlattenedContext) openfeature.InterfaceResolutionDetail {
	value, detail := resolveTyped(ctx, p, flag, defaultValue, flatCtx, flagTypeObject, toObject)
	return openfeature.InterfaceResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

func resolveTyped[T any](
	ctx context.Context,
	p *ServerProvider,
	flag string,
	defaultValue T,
	flatCtx openfeature.FlattenedContext,
	expectedType string,
	convert func(any) (T, error),
) (T, openfeature.ProviderResolutionDetail) {
	if p.logging {
		p.logger.DebugContext(ctx, "Evaluating Flagship flag", "flag", flag)
	}

	var cacheKey string
	if p.cache != nil {
		key, err := buildCacheKey(flag, expectedType, flatCtx)
		if err == nil {
			if cached, ok := p.cache.get(key); ok {
				value, err := convert(cached.Value)
				if err == nil {
					return value, openfeature.ProviderResolutionDetail{
						Reason:       openfeature.CachedReason,
						Variant:      cached.Variant,
						FlagMetadata: openfeature.FlagMetadata{},
					}
				}
			}
			cacheKey = key
		}
	}

	var result EvaluationResponse
	var err error
	if p.localEvaluation {
		result, err = p.evaluateLocal(ctx, flag, flatCtx)
	} else {
		result, err = p.client.EvaluateFlat(ctx, flag, flatCtx)
	}
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
	if p.cache != nil && cacheKey != "" {
		p.cache.set(cacheKey, result)
	}

	return value, openfeature.ProviderResolutionDetail{
		Reason:       mapReason(result.Reason),
		Variant:      result.Variant,
		FlagMetadata: openfeature.FlagMetadata{},
	}
}

func (p *ServerProvider) evaluateLocal(ctx context.Context, flagKey string, flatCtx openfeature.FlattenedContext) (EvaluationResponse, error) {
	snap := p.snapshot.Load()
	if snap == nil {
		return EvaluationResponse{}, newError(
			ErrorCodeProviderNotReady,
			"Flagship local evaluation is not ready — definitions have not been loaded",
			0,
			nil,
		)
	}

	flagDef, found, err := snap.get(flagKey)
	if !found {
		return EvaluationResponse{}, newError(
			ErrorCodeFlagNotFound,
			fmt.Sprintf(`Flag "%s" was not found in local definitions`, flagKey),
			0,
			nil,
		)
	}
	if err != nil {
		return EvaluationResponse{}, newError(
			ErrorCodeParse,
			fmt.Sprintf(`Flag "%s" definition is malformed: %v`, flagKey, err),
			0,
			err,
		)
	}

	contextValues, err := normalizeLocalContext(flatCtx)
	if err != nil {
		return EvaluationResponse{}, err
	}

	result, err := evaluateFlag(flagDef, contextValues, p.accountID)
	if err != nil {
		if _, ok := err.(*flagConfigError); ok {
			return EvaluationResponse{}, newError(ErrorCodeParse, err.Error(), 0, err)
		}
		return EvaluationResponse{}, newError(
			ErrorCodeParse,
			fmt.Sprintf(`Flag "%s" definition is malformed: %v`, flagKey, err),
			0,
			err,
		)
	}

	_ = ctx // local evaluation is pure; ctx is reserved for future cancellation hooks
	return EvaluationResponse{
		FlagKey: flagKey,
		Value:   result.Value,
		Variant: result.Variant,
		Reason:  result.Reason,
	}, nil
}

// normalizeLocalContext converts FlattenedContext into the JSON value model the
// engine expects. time.Time becomes RFC3339Nano UTC, ints/floats become
// json.Number, and maps/slices are accepted when JSON-compatible. HTTP mode's
// primitive-only restriction is intentionally not changed here.
func normalizeLocalContext(flatCtx openfeature.FlattenedContext) (map[string]any, error) {
	normalized, err := normalizeContext(flatCtx)
	if err != nil {
		return nil, err
	}
	return toJSONValueModel(normalized.values)
}

func toJSONValueModel(values map[string]any) (map[string]any, error) {
	result := make(map[string]any, len(values))
	for key, value := range values {
		converted, err := toJSONValue(value, key)
		if err != nil {
			return nil, err
		}
		result[key] = converted
	}
	return result, nil
}

func toJSONValue(value any, path string) (any, error) {
	if value == nil {
		return nil, nil
	}
	switch v := value.(type) {
	case string, bool:
		return v, nil
	case json.Number:
		return v, nil
	case float32:
		return json.Number(strconvFormatFloat(float64(v))), nil
	case float64:
		return json.Number(strconvFormatFloat(v)), nil
	case int:
		return json.Number(fmt.Sprintf("%d", v)), nil
	case int8:
		return json.Number(fmt.Sprintf("%d", v)), nil
	case int16:
		return json.Number(fmt.Sprintf("%d", v)), nil
	case int32:
		return json.Number(fmt.Sprintf("%d", v)), nil
	case int64:
		return json.Number(fmt.Sprintf("%d", v)), nil
	case uint:
		return json.Number(fmt.Sprintf("%d", v)), nil
	case uint8:
		return json.Number(fmt.Sprintf("%d", v)), nil
	case uint16:
		return json.Number(fmt.Sprintf("%d", v)), nil
	case uint32:
		return json.Number(fmt.Sprintf("%d", v)), nil
	case uint64:
		return json.Number(fmt.Sprintf("%d", v)), nil
	case map[string]any:
		return toJSONValueModel(v)
	case []any:
		out := make([]any, len(v))
		for i, item := range v {
			converted, err := toJSONValue(item, fmt.Sprintf("%s[%d]", path, i))
			if err != nil {
				return nil, err
			}
			out[i] = converted
		}
		return out, nil
	default:
		return nil, invalidContextError(path, value)
	}
}

func strconvFormatFloat(f float64) string {
	return strconv.FormatFloat(f, 'f', -1, 64)
}

func buildCacheKey(flagKey string, expectedType string, flatCtx openfeature.FlattenedContext) (string, error) {
	normalized, err := normalizeContext(flatCtx)
	if err != nil {
		return "", err
	}
	contextJSON, err := json.Marshal(normalized.values)
	if err != nil {
		return "", err
	}
	encoded, err := json.Marshal([]string{flagKey, expectedType, string(contextJSON)})
	if err != nil {
		return "", err
	}
	return string(encoded), nil
}

func mapReason(reason EvaluationReason) openfeature.Reason {
	switch reason {
	case ReasonStatic:
		return openfeature.StaticReason
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
		case ErrorCodeProviderNotReady:
			return openfeature.NewProviderNotReadyResolutionError(flagshipErr.Error())
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
