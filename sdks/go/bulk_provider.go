package flagship

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/rand"
	"net/http"
	"net/url"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/open-feature/go-sdk/openfeature"
)

var (
	_ openfeature.FeatureProvider          = (*ClientProvider)(nil)
	_ openfeature.ContextAwareStateHandler = (*ClientProvider)(nil)
	_ openfeature.EventHandler             = (*ClientProvider)(nil)
)

// ChangeDetection selects how a ClientProvider learns about configuration changes.
type ChangeDetection string

const (
	// ChangeDetectionSSE connects to event streams the server advertises and
	// refreshes when notified. Timer polling runs only while no stream is connected.
	ChangeDetectionSSE ChangeDetection = "sse"
	// ChangeDetectionPolling ignores event streams and refreshes every PollInterval.
	ChangeDetectionPolling ChangeDetection = "polling"
	// ChangeDetectionNone refreshes only on initialization and Identify.
	ChangeDetectionNone ChangeDetection = "none"
)

const (
	defaultClientTimeout    = 10 * time.Second
	defaultInactivityDelay  = 120 * time.Second
	hintCoalesceWindow      = 50 * time.Millisecond
	maxStreamReconnectDelay = 30 * time.Second
)

var hintRetryDelays = []time.Duration{time.Second, 2 * time.Second, 4 * time.Second, 8 * time.Second, 16 * time.Second}

// ClientProviderOptions configures a ClientProvider.
//
// Provide one of AppID plus AccountID, Endpoint (ending in /evaluate), or
// OFREPBaseURL (any OFREP server, up to but not including /ofrep).
type ClientProviderOptions struct {
	AppID        string
	AccountID    string
	Endpoint     string
	BaseURL      string
	OFREPBaseURL string

	AuthToken      string
	Headers        http.Header
	HeadersFactory HeaderFactory
	HTTPClient     *http.Client
	// Timeout limits each request. Defaults to 10 seconds.
	Timeout        time.Duration
	Retries        int
	DisableRetries bool
	RetryDelay     time.Duration

	// ChangeDetection defaults to ChangeDetectionSSE.
	ChangeDetection ChangeDetection
	// PollInterval enables timer polling when greater than 0. Defaults to 0.
	PollInterval time.Duration
	// InactivityDelay overrides the server's inactivityDelaySec for Pause.
	InactivityDelay time.Duration

	Logging bool
	Logger  Logger
	Hooks   []openfeature.Hook
}

// ClientProvider is an OpenFeature provider for one static evaluation context,
// such as a single user. It follows the OFREP static-context provider
// guideline: initialization evaluates every flag with one bulk request, and
// evaluations read only that cache.
//
// The OpenFeature Go SDK does not notify providers when the evaluation context
// changes. Call Identify with the new context and keep the OpenFeature context
// in sync. An evaluation whose non-empty context differs from the provider's
// context returns INVALID_CONTEXT instead of another context's cached values.
type ClientProvider struct {
	client          *FlagshipClient
	hooks           []openfeature.Hook
	changeDetection ChangeDetection
	pollInterval    time.Duration
	inactivity      time.Duration
	logging         bool
	logger          Logger
	events          chan openfeature.Event

	snapshot  atomic.Pointer[bulkSnapshot]
	refreshMu sync.Mutex

	mu           sync.Mutex
	evalCtx      openfeature.EvaluationContext
	epoch        uint64
	cancel       context.CancelFunc
	wg           sync.WaitGroup
	session      *streamSession
	pauseTimer   *time.Timer
	blockedUntil time.Time
	stale        bool
	fatal        bool
}

type bulkFlag struct {
	Key          string         `json:"key"`
	Value        any            `json:"value"`
	Variant      string         `json:"variant"`
	Reason       string         `json:"reason"`
	Metadata     map[string]any `json:"metadata"`
	ErrorCode    string         `json:"errorCode"`
	ErrorDetails string         `json:"errorDetails"`
}

type bulkEvaluation struct {
	flags   map[string]bulkFlag
	raw     map[string]string
	etag    string
	version string
	streams []eventStreamDescriptor
}

type bulkSnapshot struct {
	contextKey string
	bulkEvaluation
}

type refreshKind int

const (
	refreshDiscover refreshKind = iota
	refreshHint
	refreshRevalidate
)

type bulkRequest struct {
	ifNoneMatch            string
	flagConfigEtag         string
	flagConfigLastModified string
	requestEventStream     bool
}

type rateLimitError struct{ retryAfter time.Duration }

func (e *rateLimitError) Error() string { return "bulk evaluation was rate limited (HTTP 429)" }

// NewClientProvider constructs an OFREP static-context provider.
func NewClientProvider(options ClientProviderOptions) (*ClientProvider, error) {
	changeDetection := options.ChangeDetection
	if changeDetection == "" {
		changeDetection = ChangeDetectionSSE
	}
	if changeDetection != ChangeDetectionSSE && changeDetection != ChangeDetectionPolling && changeDetection != ChangeDetectionNone {
		return nil, fmt.Errorf(`Flagship: ChangeDetection must be "sse", "polling", or "none"`)
	}
	if options.PollInterval < 0 {
		return nil, fmt.Errorf("Flagship: PollInterval must not be negative")
	}
	endpoint := options.Endpoint
	if options.OFREPBaseURL != "" {
		if options.AppID != "" || endpoint != "" {
			return nil, fmt.Errorf(`Flagship: provide only one of "OFREPBaseURL", "AppID", or "Endpoint"`)
		}
		base, err := url.Parse(options.OFREPBaseURL)
		if err != nil || (base.Scheme != "https" && base.Scheme != "http") || base.Host == "" {
			return nil, fmt.Errorf("Flagship: OFREPBaseURL must be an absolute http or https URL: %s", options.OFREPBaseURL)
		}
		base.Path = strings.TrimRight(base.Path, "/") + "/evaluate"
		base.RawQuery, base.Fragment = "", ""
		endpoint = base.String()
	}
	timeout := options.Timeout
	if timeout == 0 {
		timeout = defaultClientTimeout
	}
	client, err := NewClient(Options{
		AppID: options.AppID, AccountID: options.AccountID, Endpoint: endpoint, BaseURL: options.BaseURL,
		AuthToken: options.AuthToken, Headers: options.Headers, HeadersFactory: options.HeadersFactory,
		HTTPClient: options.HTTPClient, Timeout: timeout, Retries: options.Retries,
		DisableRetries: options.DisableRetries, RetryDelay: options.RetryDelay,
	})
	if err != nil {
		return nil, err
	}
	if _, err := client.bulkEndpoint(); err != nil {
		return nil, fmt.Errorf(`Flagship: the static-context provider requires an endpoint ending in "/evaluate"`)
	}
	return &ClientProvider{
		client:          client,
		hooks:           append([]openfeature.Hook(nil), options.Hooks...),
		changeDetection: changeDetection,
		pollInterval:    options.PollInterval,
		inactivity:      options.InactivityDelay,
		logging:         options.Logging,
		logger:          resolveLogger(options.Logger),
		events:          make(chan openfeature.Event, 16),
	}, nil
}

// Metadata returns the provider metadata required by OpenFeature.
func (p *ClientProvider) Metadata() openfeature.Metadata {
	return openfeature.Metadata{Name: "Flagship Client Provider"}
}

// Hooks returns provider-level hooks.
func (p *ClientProvider) Hooks() []openfeature.Hook {
	return append([]openfeature.Hook(nil), p.hooks...)
}

// EventChannel returns the provider's event channel.
func (p *ClientProvider) EventChannel() <-chan openfeature.Event { return p.events }

// Init initializes the provider without a cancellation-aware context.
func (p *ClientProvider) Init(evalCtx openfeature.EvaluationContext) error {
	return p.InitWithContext(context.Background(), evalCtx)
}

// InitWithContext evaluates every flag for evalCtx and starts change detection.
// A 401 or 403 returns a PROVIDER_FATAL initialization error.
func (p *ClientProvider) InitWithContext(ctx context.Context, evalCtx openfeature.EvaluationContext) error {
	return p.load(ctx, evalCtx, true)
}

// Identify re-evaluates every flag for a new context and reconnects change
// detection. The request is unconditional, so the previous context's ETag never
// validates the new context's results. On failure the cache is cleared.
func (p *ClientProvider) Identify(ctx context.Context, evalCtx openfeature.EvaluationContext) error {
	return p.load(ctx, evalCtx, false)
}

// Shutdown stops background work and clears the cache.
func (p *ClientProvider) Shutdown() { _ = p.ShutdownWithContext(context.Background()) }

// ShutdownWithContext stops background work and clears the cache.
func (p *ClientProvider) ShutdownWithContext(context.Context) error {
	p.stopBackground()
	p.snapshot.Store(nil)
	p.mu.Lock()
	p.stale, p.fatal = false, false
	p.mu.Unlock()
	return nil
}

// Pause tells the provider the host became inactive, such as a backgrounded
// app. Event streams close after the effective inactivity delay:
// InactivityDelay, else the server's inactivityDelaySec, else 120 seconds.
func (p *ClientProvider) Pause() {
	p.mu.Lock()
	defer p.mu.Unlock()
	session := p.session
	if session == nil || p.pauseTimer != nil {
		return
	}
	delay := p.inactivity
	if delay <= 0 {
		delay = session.inactivityDelay()
	}
	if delay <= 0 {
		delay = defaultInactivityDelay
	}
	p.pauseTimer = time.AfterFunc(delay, session.suspend)
}

// Resume tells the provider the host became active again. A stream closed for
// inactivity reconnects with an unconditional refresh. Without a stream, the
// cache is revalidated with If-None-Match unless change detection is disabled.
func (p *ClientProvider) Resume(ctx context.Context) {
	p.mu.Lock()
	if p.pauseTimer != nil {
		p.pauseTimer.Stop()
		p.pauseTimer = nil
	}
	session, fatal, epoch := p.session, p.fatal, p.epoch
	p.mu.Unlock()
	if session != nil {
		session.resume()
		return
	}
	if p.changeDetection != ChangeDetectionNone && p.snapshot.Load() != nil && !fatal {
		p.refresh(ctx, epoch, refreshRevalidate, configurationHint{}, false)
	}
}

// BooleanEvaluation evaluates a boolean flag from the cache.
func (p *ClientProvider) BooleanEvaluation(_ context.Context, flag string, defaultValue bool, flatCtx openfeature.FlattenedContext) openfeature.BoolResolutionDetail {
	value, detail := resolveCached(p, flag, defaultValue, flatCtx, toBool)
	return openfeature.BoolResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// StringEvaluation evaluates a string flag from the cache.
func (p *ClientProvider) StringEvaluation(_ context.Context, flag string, defaultValue string, flatCtx openfeature.FlattenedContext) openfeature.StringResolutionDetail {
	value, detail := resolveCached(p, flag, defaultValue, flatCtx, toString)
	return openfeature.StringResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// FloatEvaluation evaluates a float flag from the cache.
func (p *ClientProvider) FloatEvaluation(_ context.Context, flag string, defaultValue float64, flatCtx openfeature.FlattenedContext) openfeature.FloatResolutionDetail {
	value, detail := resolveCached(p, flag, defaultValue, flatCtx, toFloat64)
	return openfeature.FloatResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// IntEvaluation evaluates an integer flag from the cache.
func (p *ClientProvider) IntEvaluation(_ context.Context, flag string, defaultValue int64, flatCtx openfeature.FlattenedContext) openfeature.IntResolutionDetail {
	value, detail := resolveCached(p, flag, defaultValue, flatCtx, toInt64)
	return openfeature.IntResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

// ObjectEvaluation evaluates an object flag from the cache.
func (p *ClientProvider) ObjectEvaluation(_ context.Context, flag string, defaultValue any, flatCtx openfeature.FlattenedContext) openfeature.InterfaceResolutionDetail {
	value, detail := resolveCached(p, flag, defaultValue, flatCtx, toObject)
	return openfeature.InterfaceResolutionDetail{Value: value, ProviderResolutionDetail: detail}
}

func resolveCached[T any](p *ClientProvider, flag string, defaultValue T, flatCtx openfeature.FlattenedContext, convert func(any) (T, error)) (T, openfeature.ProviderResolutionDetail) {
	failed := func(err openfeature.ResolutionError) (T, openfeature.ProviderResolutionDetail) {
		return defaultValue, openfeature.ProviderResolutionDetail{Reason: openfeature.ErrorReason, ResolutionError: err, FlagMetadata: openfeature.FlagMetadata{}}
	}
	snap := p.snapshot.Load()
	if snap == nil {
		return failed(openfeature.NewProviderNotReadyResolutionError("Flagship flags have not been evaluated for a context yet"))
	}
	if len(flatCtx) > 0 {
		if key, err := flatContextKey(flatCtx); err != nil || key != snap.contextKey {
			return failed(openfeature.NewInvalidContextResolutionError("the evaluation context differs from the provider's static context; call Identify with it"))
		}
	}
	entry, ok := snap.flags[flag]
	if !ok {
		return failed(openfeature.NewFlagNotFoundResolutionError(fmt.Sprintf("Flag %q was not returned by the bulk evaluation", flag)))
	}
	if entry.ErrorCode != "" {
		message := entry.ErrorDetails
		if message == "" {
			message = fmt.Sprintf("Flag %q evaluation failed", flag)
		}
		return failed(bulkResolutionError(entry.ErrorCode, message))
	}
	if EvaluationReason(entry.Reason) == ReasonDisabled {
		return defaultValue, openfeature.ProviderResolutionDetail{Reason: openfeature.DisabledReason, FlagMetadata: toFlagMetadata(entry.Metadata)}
	}
	value, err := convert(entry.Value)
	if err != nil {
		return failed(openfeature.NewTypeMismatchResolutionError(err.Error()))
	}
	reason := entry.Reason
	if reason == "" {
		reason = string(ReasonDefault)
	}
	return value, openfeature.ProviderResolutionDetail{Reason: mapReason(EvaluationReason(reason)), Variant: entry.Variant, FlagMetadata: toFlagMetadata(entry.Metadata)}
}

func bulkResolutionError(code, message string) openfeature.ResolutionError {
	switch code {
	case "FLAG_NOT_FOUND":
		return openfeature.NewFlagNotFoundResolutionError(message)
	case "PARSE_ERROR":
		return openfeature.NewParseErrorResolutionError(message)
	case "TYPE_MISMATCH":
		return openfeature.NewTypeMismatchResolutionError(message)
	case "TARGETING_KEY_MISSING":
		return openfeature.NewTargetingKeyMissingResolutionError(message)
	case "INVALID_CONTEXT":
		return openfeature.NewInvalidContextResolutionError(message)
	default:
		return openfeature.NewGeneralResolutionError(message)
	}
}

func (p *ClientProvider) load(ctx context.Context, evalCtx openfeature.EvaluationContext, initial bool) error {
	p.stopBackground()
	previous := p.snapshot.Swap(nil)
	p.mu.Lock()
	p.epoch++
	epoch := p.epoch
	p.evalCtx = evalCtx
	p.stale, p.fatal = false, false
	p.mu.Unlock()

	result, err := p.fetch(ctx, refreshDiscover, configurationHint{})
	if err == nil && result == nil {
		err = newError(ErrorCodeParse, "unexpected 304 for an unconditional bulk evaluation", http.StatusNotModified, nil)
	}
	if err != nil {
		if p.logging {
			p.logger.WarnContext(ctx, "Flagship bulk evaluation failed", "initial", initial, "error", err)
		}
		if isAuthError(err) {
			return &openfeature.ProviderInitError{ErrorCode: openfeature.ProviderFatalCode, Message: err.Error()}
		}
		return err
	}
	snap, err := newBulkSnapshot(evalCtx, result)
	if err != nil {
		return err
	}

	p.mu.Lock()
	if epoch != p.epoch {
		p.mu.Unlock()
		return nil
	}
	p.snapshot.Store(snap)
	background, cancel := context.WithCancel(context.Background())
	p.cancel = cancel
	if p.changeDetection == ChangeDetectionSSE && len(streamURLs(result.streams, p.bulkURL())) > 0 {
		p.startSessionLocked(background, epoch, result.streams)
	}
	if p.changeDetection != ChangeDetectionNone && p.pollInterval > 0 {
		p.wg.Add(1)
		go p.pollLoop(background, epoch)
	}
	p.mu.Unlock()
	if !initial {
		p.emitChanged(previous, snap, false)
	}
	return nil
}

func (p *ClientProvider) startSessionLocked(ctx context.Context, epoch uint64, descriptors []eventStreamDescriptor) {
	session := &streamSession{provider: p, epoch: epoch, descriptors: descriptors, wake: make(chan struct{}, 1)}
	session.delay.Store(int64(streamInactivityDelay(descriptors)))
	p.session = session
	p.wg.Add(1)
	go func() {
		defer p.wg.Done()
		session.run(ctx)
	}()
}

func (p *ClientProvider) stopBackground() {
	p.mu.Lock()
	p.epoch++
	if p.cancel != nil {
		p.cancel()
		p.cancel = nil
	}
	if p.pauseTimer != nil {
		p.pauseTimer.Stop()
		p.pauseTimer = nil
	}
	p.session = nil
	p.mu.Unlock()
	p.wg.Wait()
}

func (p *ClientProvider) pollLoop(ctx context.Context, epoch uint64) {
	defer p.wg.Done()
	ticker := time.NewTicker(p.pollInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
		p.mu.Lock()
		session, fatal := p.session, p.fatal
		p.mu.Unlock()
		if fatal {
			return
		}
		if p.changeDetection == ChangeDetectionSSE && session != nil && session.connected.Load() {
			continue
		}
		p.refresh(ctx, epoch, refreshRevalidate, configurationHint{}, false)
	}
}

// fetch issues one bulk request: refreshDiscover is unconditional and requests
// event streams, refreshHint forwards event metadata, and refreshRevalidate
// sends the stored ETag. A nil result means 304 Not Modified.
func (p *ClientProvider) fetch(ctx context.Context, kind refreshKind, hint configurationHint) (*bulkEvaluation, error) {
	p.mu.Lock()
	blockedUntil, evalCtx, session := p.blockedUntil, p.evalCtx, p.session
	p.mu.Unlock()
	if wait := time.Until(blockedUntil); wait > 0 {
		return nil, &rateLimitError{retryAfter: wait}
	}
	var etag string
	if snap := p.snapshot.Load(); snap != nil {
		etag = snap.etag
	}
	request := bulkRequest{}
	switch kind {
	case refreshDiscover:
		request.requestEventStream = p.changeDetection == ChangeDetectionSSE
	case refreshHint:
		request = bulkRequest{ifNoneMatch: etag, flagConfigEtag: hint.ETag, flagConfigLastModified: hint.lastModified()}
	case refreshRevalidate:
		request = bulkRequest{ifNoneMatch: etag, requestEventStream: p.changeDetection == ChangeDetectionSSE && (session == nil || !session.connected.Load())}
	}
	result, err := p.client.fetchBulk(ctx, flattenContext(evalCtx), request)
	var limited *rateLimitError
	if errors.As(err, &limited) && limited.retryAfter > 0 {
		p.mu.Lock()
		p.blockedUntil = time.Now().Add(limited.retryAfter)
		p.mu.Unlock()
	}
	return result, err
}

// refresh applies one refresh and reports whether it succeeded.
func (p *ClientProvider) refresh(ctx context.Context, epoch uint64, kind refreshKind, hint configurationHint, notification bool) (*bulkEvaluation, bool) {
	p.refreshMu.Lock()
	defer p.refreshMu.Unlock()
	if !p.current(epoch) {
		return nil, false
	}
	result, err := p.fetch(ctx, kind, hint)
	if err != nil {
		if !p.current(epoch) || ctx.Err() != nil {
			return nil, false
		}
		if isAuthError(err) {
			p.fail(err.Error())
		} else {
			p.markStale(err.Error())
		}
		return nil, false
	}
	p.mu.Lock()
	if epoch != p.epoch {
		p.mu.Unlock()
		return nil, false
	}
	evalCtx := p.evalCtx
	p.mu.Unlock()
	previous := p.snapshot.Load()
	current := previous
	if result != nil {
		snap, err := newBulkSnapshot(evalCtx, result)
		if err != nil {
			p.markStale(err.Error())
			return nil, false
		}
		p.snapshot.Store(snap)
		current = snap
	}
	p.recover()
	if result != nil {
		p.emitChanged(previous, current, notification)
		p.mu.Lock()
		if p.changeDetection == ChangeDetectionSSE && p.session == nil && p.cancel != nil && epoch == p.epoch &&
			len(streamURLs(result.streams, p.bulkURL())) > 0 {
			background, cancel := context.WithCancel(context.Background())
			previousCancel := p.cancel
			p.cancel = func() { previousCancel(); cancel() }
			p.startSessionLocked(background, epoch, result.streams)
		}
		p.mu.Unlock()
	}
	return result, true
}

func (p *ClientProvider) current(epoch uint64) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return epoch == p.epoch && !p.fatal
}

func (p *ClientProvider) emitChanged(previous, current *bulkSnapshot, force bool) {
	if current == nil {
		return
	}
	var old map[string]string
	if previous != nil {
		old = previous.raw
	}
	keys := map[string]struct{}{}
	for key := range old {
		keys[key] = struct{}{}
	}
	for key := range current.raw {
		keys[key] = struct{}{}
	}
	var changed []string
	for key := range keys {
		if old[key] != current.raw[key] {
			changed = append(changed, key)
		}
	}
	sort.Strings(changed)
	versionChanged := previous != nil && current.version != "" && current.version != previous.version
	if force || len(changed) > 0 || versionChanged {
		p.emit(openfeature.ProviderConfigChange, openfeature.ProviderEventDetails{FlagChanges: changed})
	}
}

func (p *ClientProvider) markStale(message string) {
	if p.logging {
		p.logger.WarnContext(context.Background(), "Flagship refresh failed; keeping cached flags", "error", message)
	}
	p.mu.Lock()
	notify := !p.stale && !p.fatal && p.snapshot.Load() != nil
	p.stale = p.stale || notify
	p.mu.Unlock()
	if notify {
		p.emit(openfeature.ProviderStale, openfeature.ProviderEventDetails{Message: message})
	}
}

func (p *ClientProvider) recover() {
	p.mu.Lock()
	notify := p.stale
	p.stale = false
	p.mu.Unlock()
	if notify {
		p.emit(openfeature.ProviderReady, openfeature.ProviderEventDetails{})
	}
}

// fail stops background refreshes after the server rejects the credentials.
func (p *ClientProvider) fail(message string) {
	p.mu.Lock()
	p.fatal = true
	if p.cancel != nil {
		p.cancel()
	}
	p.session = nil
	p.mu.Unlock()
	p.emit(openfeature.ProviderError, openfeature.ProviderEventDetails{Message: message, ErrorCode: openfeature.ProviderFatalCode})
}

func (p *ClientProvider) emit(eventType openfeature.EventType, details openfeature.ProviderEventDetails) {
	select {
	case p.events <- openfeature.Event{ProviderName: p.Metadata().Name, EventType: eventType, ProviderEventDetails: details}:
	default:
	}
}

func (p *ClientProvider) bulkURL() string {
	endpoint, _ := p.client.bulkEndpoint()
	return endpoint
}

// streamSession holds every advertised event stream open and turns hints into
// refreshes. Stream URLs carry single-use tickets, so a dropped connection is
// replaced by rediscovering fresh descriptors, never by reopening an old URL.
type streamSession struct {
	provider    *ClientProvider
	epoch       uint64
	descriptors []eventStreamDescriptor
	connected   atomic.Bool
	suspended   atomic.Bool
	delay       atomic.Int64
	wake        chan struct{}
	mu          sync.Mutex
	stopListen  context.CancelFunc
}

func (s *streamSession) inactivityDelay() time.Duration { return time.Duration(s.delay.Load()) }

func (s *streamSession) suspend() {
	s.suspended.Store(true)
	s.mu.Lock()
	if s.stopListen != nil {
		s.stopListen()
	}
	s.mu.Unlock()
}

func (s *streamSession) resume() {
	if s.suspended.CompareAndSwap(true, false) {
		select {
		case s.wake <- struct{}{}:
		default:
		}
	}
}

func (s *streamSession) run(ctx context.Context) {
	p := s.provider
	delay := 500 * time.Millisecond
	descriptors := s.descriptors
	for ctx.Err() == nil && p.current(s.epoch) {
		if s.suspended.Load() {
			select {
			case <-ctx.Done():
				return
			case <-s.wake:
			}
			continue
		}
		if descriptors == nil {
			// The rediscovery refetch doubles as the catch-up refetch OFREP requires after reconnecting.
			if result, ok := p.refresh(ctx, s.epoch, refreshDiscover, configurationHint{}, false); ok && result != nil {
				descriptors = result.streams
			}
		}
		urls := streamURLs(descriptors, p.bulkURL())
		if delay := streamInactivityDelay(descriptors); delay > 0 {
			s.delay.Store(int64(delay))
		}
		descriptors = nil
		if len(urls) > 0 && s.listen(ctx, urls) {
			delay = 500 * time.Millisecond
		}
		if ctx.Err() != nil || s.suspended.Load() {
			continue
		}
		p.markStale("configuration stream interrupted")
		wait := delay/2 + time.Duration(rand.Int63n(int64(delay)+1))
		if sleepWithContext(ctx, wait) != nil {
			return
		}
		delay = min(maxStreamReconnectDelay, delay*2)
	}
}

// listen holds every stream until one ends and reports whether all connected.
func (s *streamSession) listen(parent context.Context, urls []string) bool {
	ctx, cancel := context.WithCancel(parent)
	s.mu.Lock()
	s.stopListen = cancel
	s.mu.Unlock()
	defer cancel()

	hints := make(chan configurationHint, 16)
	closed := make(chan struct{}, len(urls))
	var open atomic.Int32
	var readers sync.WaitGroup
	for _, address := range urls {
		readers.Add(1)
		go func() {
			defer readers.Done()
			defer func() { closed <- struct{}{} }()
			response, stop, err := s.provider.client.openStream(ctx, address)
			if err != nil {
				return
			}
			defer stop()
			defer response.Body.Close()
			if int(open.Add(1)) == len(urls) {
				s.connected.Store(true)
			}
			_ = readConfigurationEvents(response.Body, func(hint configurationHint) error {
				select {
				case hints <- hint:
					return nil
				case <-ctx.Done():
					return ctx.Err()
				}
			})
		}()
	}
	defer func() {
		cancel()
		readers.Wait()
		s.connected.Store(false)
	}()

	for {
		select {
		case <-ctx.Done():
			return int(open.Load()) == len(urls)
		case <-closed:
			return int(open.Load()) == len(urls)
		case hint := <-hints:
			// Coalesce bursts, including one change announced on several streams.
			timer := time.NewTimer(hintCoalesceWindow)
			done := false
			for !done {
				select {
				case next := <-hints:
					hint = next
				case <-timer.C:
					done = true
				}
			}
			s.refreshFor(ctx, hint)
		}
	}
}

// refreshFor refreshes until the observed version matches the hinted one.
func (s *streamSession) refreshFor(ctx context.Context, hint configurationHint) {
	p := s.provider
	for attempt := 0; attempt <= len(hintRetryDelays); attempt++ {
		if attempt > 0 && sleepWithContext(ctx, hintRetryDelays[attempt-1]) != nil {
			return
		}
		if _, ok := p.refresh(ctx, s.epoch, refreshHint, hint, attempt == 0); !ok {
			continue
		}
		if snap := p.snapshot.Load(); hint.ETag == "" || (snap != nil && snap.version == hint.ETag) {
			return
		}
	}
}

func (c *FlagshipClient) fetchBulk(ctx context.Context, flatCtx openfeature.FlattenedContext, request bulkRequest) (*bulkEvaluation, error) {
	endpoint, err := c.bulkEndpoint()
	if err != nil {
		return nil, err
	}
	normalized, err := normalizeContext(flatCtx)
	if err != nil {
		return nil, err
	}
	body, err := json.Marshal(struct {
		Context map[string]any `json:"context"`
	}{Context: normalized.values})
	if err != nil {
		return nil, newError(ErrorCodeInvalidContext, fmt.Sprintf("failed to serialize evaluation context: %v", err), 0, err)
	}
	u, _ := url.Parse(endpoint)
	query := u.Query()
	if request.flagConfigEtag != "" {
		query.Set("flagConfigEtag", request.flagConfigEtag)
	}
	if request.flagConfigLastModified != "" {
		query.Set("flagConfigLastModified", request.flagConfigLastModified)
	}
	u.RawQuery = query.Encode()
	built := evaluationRequest{method: http.MethodPost, url: u.String(), body: body}
	if request.requestEventStream {
		built.extraHeaders = http.Header{"Flagship-Event-Stream": []string{"sse"}}
	}
	return fetchWithRetry(ctx, c, built, request.ifNoneMatch, parseBulkEvaluation)
}

func parseBulkEvaluation(resp *http.Response) (*bulkEvaluation, error) {
	switch {
	case resp.StatusCode == http.StatusNotModified:
		return nil, nil
	case resp.StatusCode == http.StatusTooManyRequests:
		return nil, &rateLimitError{retryAfter: parseRetryAfter(resp.Header.Get("Retry-After"), time.Now())}
	case resp.StatusCode == http.StatusUnauthorized || resp.StatusCode == http.StatusForbidden:
		return nil, newError(ErrorCodeBadRequest, fmt.Sprintf("bulk evaluation returned HTTP %d", resp.StatusCode), resp.StatusCode, nil)
	case resp.StatusCode == http.StatusNotFound:
		return nil, newError(ErrorCodeFlagNotFound, "bulk evaluation returned HTTP 404", resp.StatusCode, nil)
	case resp.StatusCode == http.StatusBadRequest:
		return nil, newError(ErrorCodeBadRequest, "bulk evaluation returned HTTP 400", resp.StatusCode, nil)
	case resp.StatusCode != http.StatusOK:
		return nil, newError(ErrorCodeNetwork, fmt.Sprintf("bulk evaluation returned HTTP %d", resp.StatusCode), resp.StatusCode, nil)
	}
	var envelope struct {
		Flags    []json.RawMessage `json:"flags"`
		Metadata struct {
			Version string `json:"version"`
		} `json:"metadata"`
		EventStreams []eventStreamDescriptor `json:"eventStreams"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 16<<20)).Decode(&envelope); err != nil || envelope.Flags == nil {
		return nil, newError(ErrorCodeParse, "invalid bulk evaluation response", resp.StatusCode, err)
	}
	result := &bulkEvaluation{
		flags:   make(map[string]bulkFlag, len(envelope.Flags)),
		raw:     make(map[string]string, len(envelope.Flags)),
		etag:    resp.Header.Get("ETag"),
		version: envelope.Metadata.Version,
		streams: envelope.EventStreams,
	}
	for _, raw := range envelope.Flags {
		decoder := json.NewDecoder(strings.NewReader(string(raw)))
		decoder.UseNumber()
		var entry bulkFlag
		if decoder.Decode(&entry) != nil || entry.Key == "" {
			continue
		}
		result.flags[entry.Key] = entry
		result.raw[entry.Key] = string(raw)
	}
	return result, nil
}

// parseRetryAfter reads a Retry-After delay in seconds or an HTTP date.
func parseRetryAfter(value string, now time.Time) time.Duration {
	value = strings.TrimSpace(value)
	if seconds, err := strconv.ParseInt(value, 10, 64); err == nil && seconds >= 0 {
		return time.Duration(seconds) * time.Second
	}
	if date, err := http.ParseTime(value); err == nil {
		return max(0, date.Sub(now))
	}
	return 0
}

func isAuthError(err error) bool {
	flagshipErr, ok := asFlagshipError(err)
	return ok && (flagshipErr.StatusCode == http.StatusUnauthorized || flagshipErr.StatusCode == http.StatusForbidden)
}

func newBulkSnapshot(evalCtx openfeature.EvaluationContext, result *bulkEvaluation) (*bulkSnapshot, error) {
	key, err := flatContextKey(flattenContext(evalCtx))
	if err != nil {
		return nil, err
	}
	return &bulkSnapshot{contextKey: key, bulkEvaluation: *result}, nil
}

func flattenContext(evalCtx openfeature.EvaluationContext) openfeature.FlattenedContext {
	flat := openfeature.FlattenedContext{}
	for key, value := range evalCtx.Attributes() {
		flat[key] = value
	}
	if targetingKey := evalCtx.TargetingKey(); targetingKey != "" {
		flat[openfeature.TargetingKey] = targetingKey
	}
	return flat
}

func flatContextKey(flatCtx openfeature.FlattenedContext) (string, error) {
	normalized, err := normalizeContext(flatCtx)
	if err != nil {
		return "", err
	}
	encoded, err := json.Marshal(normalized.values)
	return string(encoded), err
}
