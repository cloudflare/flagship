//! OFREP static-context provider: one bulk evaluation per context, cached in memory.
//!
//! [`FlagshipClientProvider`] follows the OFREP static-context provider
//! guideline. Initialization evaluates every flag for one context with a
//! single bulk request, and resolutions read only that cache.
//!
//! `open-feature` 0.3 has no provider events and never tells a provider that
//! the evaluation context changed. Call [`FlagshipClientProvider::identify`]
//! when the context changes, and use [`FlagshipClientProvider::subscribe`] for
//! configuration-change, stale, recovery, and fatal-error notifications.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use open_feature::provider::{
    FeatureProvider, ProviderMetadata, ProviderStatus, ResolutionDetails,
};
use open_feature::{
    EvaluationContext, EvaluationError, EvaluationErrorCode, EvaluationResult, Hook, HookWrapper,
    StructValue, async_trait,
};
use reqwest::header::{ETAG, HeaderMap, HeaderName, HeaderValue, IF_NONE_MATCH};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue};
use tokio::sync::{Notify, broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::client::{FlagshipClient, RawRequest, RawResponse};
use crate::config::FlagshipClientOptions;
use crate::context::normalize_context;
use crate::endpoint::resolve_bulk_endpoint;
use crate::error::{ConfigError, ErrorCode, FlagshipError};
use crate::provider::{
    flag_not_found, json_struct, map_flagship_error, map_reason, provider_not_ready,
    resolution_details,
};
use crate::types::{EvaluationReason, FlagshipEvaluationResponse};

const PROVIDER_NAME: &str = "Flagship Client Provider";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_INACTIVITY: Duration = Duration::from_secs(120);
const COALESCE_WINDOW: Duration = Duration::from_millis(50);
const MIN_RECONNECT_DELAY: Duration = Duration::from_millis(500);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);
const HINT_RETRY_DELAYS: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
];
const BULK_BODY_LIMIT: usize = 16 * 1024 * 1024;
const EVENT_LIMIT: usize = 64 * 1024;
const METADATA_LIMIT: usize = 256;
const EVENT_CHANNEL_CAPACITY: usize = 16;
const EVENT_STREAM_HEADER: HeaderName = HeaderName::from_static("flagship-event-stream");

/// How a [`FlagshipClientProvider`] learns about configuration changes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum ChangeDetection {
    /// Connect to event streams the server advertises and refresh when
    /// notified. Timer polling runs only while no stream is connected.
    #[default]
    Sse,
    /// Ignore event streams and refresh every poll interval.
    Polling,
    /// Refresh only on initialization and [`FlagshipClientProvider::identify`].
    None,
}

/// A provider notification. `open-feature` 0.3 has no provider events, so
/// subscribe with [`FlagshipClientProvider::subscribe`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientProviderEvent {
    /// The cached flags were refreshed after a configuration change.
    /// `flags_changed` lists keys whose evaluation changed for this context.
    ConfigurationChanged {
        /// Keys whose cached evaluation changed.
        flags_changed: Vec<String>,
    },
    /// A refresh succeeded after the provider became stale.
    Ready,
    /// A refresh failed; the provider keeps serving its cached flags.
    Stale {
        /// A summary of the failure.
        message: String,
    },
    /// The server rejected the credentials; background refreshes stopped.
    Fatal {
        /// A summary of the failure.
        message: String,
    },
}

/// Builder for [`FlagshipClientProvider`].
pub struct FlagshipClientProviderBuilder {
    options: FlagshipClientOptions,
    change_detection: ChangeDetection,
    poll_interval: Duration,
    inactivity_delay: Option<Duration>,
    hooks: Vec<HookWrapper>,
}

impl FlagshipClientProviderBuilder {
    /// Sets change detection, which defaults to [`ChangeDetection::Sse`].
    #[must_use]
    pub const fn change_detection(mut self, change_detection: ChangeDetection) -> Self {
        self.change_detection = change_detection;

        self
    }

    /// Enables timer polling at `interval`. Defaults to zero, which disables it.
    #[must_use]
    pub const fn poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;

        self
    }

    /// Overrides the server's `inactivityDelaySec` used by
    /// [`FlagshipClientProvider::pause`]. The fallback is 120 seconds.
    #[must_use]
    pub const fn inactivity_delay(mut self, delay: Duration) -> Self {
        self.inactivity_delay = Some(delay);

        self
    }

    /// Adds a provider hook.
    #[must_use]
    pub fn hook(mut self, hook: impl Hook) -> Self {
        self.hooks.push(HookWrapper::new(hook));

        self
    }

    /// Validates the configuration and constructs the provider.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the evaluation endpoint does not end in
    /// `/evaluate`, so no OFREP bulk endpoint can be derived.
    pub fn build(self) -> Result<FlagshipClientProvider, ConfigError> {
        let bulk_url = resolve_bulk_endpoint(self.options.evaluation_url()).ok_or_else(|| {
            ConfigError::new("the static-context provider requires an endpoint ending in /evaluate")
        })?;
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let inner = Inner {
            client: FlagshipClient::new(self.options.with_default_timeout(DEFAULT_TIMEOUT)),
            bulk_url,
            change_detection: self.change_detection,
            poll_interval: self.poll_interval,
            inactivity_override: self.inactivity_delay.filter(|delay| !delay.is_zero()),
            snapshot: RwLock::new(None),
            state: Mutex::new(State::default()),
            refresh: tokio::sync::Mutex::new(()),
            events,
        };

        Ok(FlagshipClientProvider {
            inner: Arc::new(inner),
            metadata: ProviderMetadata::new(PROVIDER_NAME),
            hooks: self.hooks,
        })
    }
}

impl fmt::Debug for FlagshipClientProviderBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlagshipClientProviderBuilder")
            .field("change_detection", &self.change_detection)
            .field("poll_interval", &self.poll_interval)
            .field("inactivity_delay", &self.inactivity_delay)
            .field("hook_count", &self.hooks.len())
            .finish_non_exhaustive()
    }
}

/// `OpenFeature` provider for one static evaluation context, such as a single user.
///
/// Call [`Self::initialize_provider`] with the context before registering the
/// provider so errors are returned to you, and [`Self::shutdown_provider`]
/// when you are done: `open-feature` 0.3 calls neither lifecycle hook for you.
/// Background tasks require a Tokio runtime.
///
/// A resolution whose non-empty context differs from the provider's context
/// returns `INVALID_CONTEXT` instead of serving another context's cached
/// values. Call [`Self::identify`] when the context changes.
#[derive(Clone)]
pub struct FlagshipClientProvider {
    inner: Arc<Inner>,
    metadata: ProviderMetadata,
    hooks: Vec<HookWrapper>,
}

impl FlagshipClientProvider {
    /// Starts a provider builder.
    #[must_use]
    pub fn builder(options: FlagshipClientOptions) -> FlagshipClientProviderBuilder {
        FlagshipClientProviderBuilder {
            options,
            change_detection: ChangeDetection::default(),
            poll_interval: Duration::ZERO,
            inactivity_delay: None,
            hooks: Vec::new(),
        }
    }

    /// Evaluates every flag for `context` and starts change detection.
    ///
    /// # Errors
    ///
    /// Returns the request failure. A 401 or 403 leaves the provider in
    /// [`ProviderStatus::Error`] and reports a [`ClientProviderEvent::Fatal`].
    pub async fn initialize_provider(
        &self,
        context: &EvaluationContext,
    ) -> Result<(), FlagshipError> {
        self.inner.load(context.clone(), true).await
    }

    /// Re-evaluates every flag for a new context and reconnects change detection.
    ///
    /// The request is unconditional, so the previous context's `ETag` never
    /// validates the new context's results. On failure the cache is cleared.
    ///
    /// # Errors
    ///
    /// Returns the request failure.
    pub async fn identify(&self, context: &EvaluationContext) -> Result<(), FlagshipError> {
        self.inner.load(context.clone(), false).await
    }

    /// Stops background tasks and clears the cache.
    pub fn shutdown_provider(&self) {
        self.inner.stop_background();
        *write(&self.inner.snapshot) = None;
        lock(&self.inner.state).status = Status::NotReady;
    }

    /// Subscribes to provider notifications.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ClientProviderEvent> {
        self.inner.events.subscribe()
    }

    /// Tells the provider the host became inactive, such as a backgrounded app.
    ///
    /// Event streams close after the effective inactivity delay: the builder's
    /// override, else the server's `inactivityDelaySec`, else 120 seconds.
    ///
    /// # Panics
    ///
    /// Panics outside a Tokio runtime.
    pub fn pause(&self) {
        let mut state = lock(&self.inner.state);
        let Some(control) = state.session.clone() else {
            return;
        };
        if state.pause.is_some() {
            return;
        }
        let delay = self
            .inner
            .inactivity_override
            .or_else(|| *lock(&control.inactivity))
            .unwrap_or(DEFAULT_INACTIVITY);
        state.pause = Some(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            control.suspend();
        }));
    }

    /// Tells the provider the host became active again.
    ///
    /// Streams closed for inactivity reconnect with an unconditional refresh.
    /// Without a stream, the cache is revalidated with `If-None-Match` unless
    /// change detection is disabled.
    pub async fn resume(&self) {
        let (session, epoch, status) = {
            let mut state = lock(&self.inner.state);
            if let Some(pause) = state.pause.take() {
                pause.abort();
            }
            (state.session.clone(), state.epoch, state.status)
        };
        if let Some(control) = session {
            control.resume();
        } else if self.inner.change_detection != ChangeDetection::None
            && matches!(status, Status::Ready | Status::Stale)
        {
            self.inner
                .refresh(epoch, RefreshKind::Revalidate, None, false)
                .await;
        }
    }

    /// Returns the provider's current lifecycle status.
    #[must_use]
    pub fn status(&self) -> ProviderStatus {
        match lock(&self.inner.state).status {
            Status::NotReady => ProviderStatus::NotReady,
            Status::Ready => ProviderStatus::Ready,
            Status::Stale => ProviderStatus::STALE,
            Status::Error | Status::Fatal => ProviderStatus::Error,
        }
    }

    fn resolve<T>(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
        expected_name: &'static str,
        convert: impl FnOnce(&JsonValue) -> Result<T, ()>,
    ) -> EvaluationResult<ResolutionDetails<T>> {
        let snapshot = read(&self.inner.snapshot)
            .clone()
            .ok_or_else(provider_not_ready)?;
        if has_values(context) {
            let key = context_key(context).map_err(map_flagship_error)?;
            if key != snapshot.context_key {
                return Err(EvaluationError {
                    code: EvaluationErrorCode::InvalidContext,
                    message: Some(
                        "INVALID_CONTEXT: the evaluation context differs from the provider's \
                         static context; call identify with it"
                            .to_owned(),
                    ),
                });
            }
        }
        let flag = snapshot.flags.get(flag_key).ok_or_else(flag_not_found)?;
        if let Some(code) = &flag.error_code {
            return Err(bulk_error(code));
        }
        let reason = flag.reason.clone().unwrap_or_else(|| "DEFAULT".to_owned());
        if reason == "DISABLED" && flag.value.is_null() {
            return Err(EvaluationError {
                code: EvaluationErrorCode::General("DISABLED".to_owned()),
                message: Some("DISABLED: the flag is disabled, so the default applies".to_owned()),
            });
        }
        let response = FlagshipEvaluationResponse::new(flag_key, flag.value.clone())
            .with_variant(flag.variant.clone().unwrap_or_default())
            .with_reason(EvaluationReason::from(reason))
            .with_metadata(flag.metadata.clone());

        resolution_details(
            &response,
            map_reason(&response.reason),
            expected_name,
            convert,
        )
    }
}

impl fmt::Debug for FlagshipClientProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlagshipClientProvider")
            .field("status", &self.status())
            .field("change_detection", &self.inner.change_detection)
            .field("poll_interval", &self.inner.poll_interval)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl FeatureProvider for FlagshipClientProvider {
    async fn initialize(&mut self, context: &EvaluationContext) {
        if lock(&self.inner.state).status == Status::NotReady {
            let _ = self.initialize_provider(context).await;
        }
    }

    fn status(&self) -> ProviderStatus {
        Self::status(self)
    }

    fn metadata(&self) -> &ProviderMetadata {
        &self.metadata
    }

    fn hooks(&self) -> &[HookWrapper] {
        &self.hooks
    }

    async fn resolve_bool_value(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<bool>> {
        self.resolve(flag_key, context, "boolean", |value| {
            value.as_bool().ok_or(())
        })
    }

    async fn resolve_int_value(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<i64>> {
        self.resolve(flag_key, context, "integer", |value| {
            value.as_i64().ok_or(())
        })
    }

    async fn resolve_float_value(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<f64>> {
        self.resolve(flag_key, context, "finite number", |value| {
            value.as_f64().filter(|value| value.is_finite()).ok_or(())
        })
    }

    async fn resolve_string_value(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<String>> {
        self.resolve(flag_key, context, "string", |value| {
            value.as_str().map(str::to_owned).ok_or(())
        })
    }

    async fn resolve_struct_value(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<StructValue>> {
        self.resolve(
            flag_key,
            context,
            "OpenFeature-compatible object",
            json_struct,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Status {
    #[default]
    NotReady,
    Ready,
    Stale,
    Error,
    Fatal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RefreshKind {
    /// Unconditional, requesting event streams: initialization, context
    /// changes, reconnects, and resuming after inactivity.
    Discover,
    /// Triggered by an event: forwards its metadata as query parameters.
    Hint,
    /// Polling or foreground: revalidates with the stored response `ETag`.
    Revalidate,
}

#[derive(Default)]
struct State {
    context: EvaluationContext,
    epoch: u64,
    status: Status,
    tasks: Vec<JoinHandle<()>>,
    session: Option<Arc<SessionControl>>,
    pause: Option<JoinHandle<()>>,
    blocked_until: Option<Instant>,
}

struct Inner {
    client: FlagshipClient,
    bulk_url: Url,
    change_detection: ChangeDetection,
    poll_interval: Duration,
    inactivity_override: Option<Duration>,
    snapshot: RwLock<Option<Arc<Snapshot>>>,
    state: Mutex<State>,
    refresh: tokio::sync::Mutex<()>,
    events: broadcast::Sender<ClientProviderEvent>,
}

struct Snapshot {
    context_key: String,
    flags: HashMap<String, BulkFlag>,
    raw: HashMap<String, String>,
    etag: Option<HeaderValue>,
    version: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BulkFlag {
    #[serde(default)]
    value: JsonValue,
    variant: Option<String>,
    reason: Option<String>,
    #[serde(default)]
    metadata: Map<String, JsonValue>,
    error_code: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventStream {
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    endpoint: Option<StreamEndpoint>,
    inactivity_delay_sec: Option<u64>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamEndpoint {
    request_uri: String,
    origin: Option<String>,
}

struct BulkEvaluation {
    snapshot: Snapshot,
    streams: Vec<EventStream>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Hint {
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(Serialize)]
struct BulkBody<'a> {
    context: &'a std::collections::BTreeMap<String, JsonValue>,
}

impl Inner {
    async fn load(
        self: &Arc<Self>,
        context: EvaluationContext,
        initial: bool,
    ) -> Result<(), FlagshipError> {
        self.stop_background();
        let previous = write(&self.snapshot).take();
        let epoch = {
            let mut state = lock(&self.state);
            state.epoch += 1;
            state.context = context.clone();
            state.status = Status::NotReady;
            state.epoch
        };
        let result = match self.fetch(RefreshKind::Discover, None).await {
            Ok(Some(result)) => result,
            Ok(None) => Err(FlagshipError::new(
                ErrorCode::ParseError,
                "unexpected 304 for an unconditional bulk evaluation",
            ))?,
            Err(error) => {
                let fatal = is_auth_error(&error);
                let mut state = lock(&self.state);
                if state.epoch == epoch {
                    state.status = if fatal { Status::Fatal } else { Status::Error };
                }
                drop(state);
                if fatal {
                    self.emit(ClientProviderEvent::Fatal {
                        message: error.to_string(),
                    });
                }
                return Err(error);
            }
        };
        let current = Arc::new(result.snapshot);
        {
            let mut state = lock(&self.state);
            if state.epoch != epoch {
                return Ok(());
            }
            *write(&self.snapshot) = Some(Arc::clone(&current));
            state.status = Status::Ready;
            if self.change_detection == ChangeDetection::Sse
                && !stream_urls(&result.streams, &self.bulk_url).is_empty()
            {
                self.start_session(&mut state, epoch, result.streams);
            }
            if self.change_detection != ChangeDetection::None && !self.poll_interval.is_zero() {
                let weak = Arc::downgrade(self);
                state
                    .tasks
                    .push(tokio::spawn(poll_loop(weak, epoch, self.poll_interval)));
            }
        }
        if !initial {
            self.emit_changed(previous.as_deref(), &current, false);
        }

        Ok(())
    }

    fn start_session(self: &Arc<Self>, state: &mut State, epoch: u64, streams: Vec<EventStream>) {
        let control = Arc::new(SessionControl::new(inactivity_delay(&streams)));
        state.session = Some(Arc::clone(&control));
        let weak = Arc::downgrade(self);
        state.tasks.push(tokio::spawn(run_session(
            weak,
            epoch,
            control,
            Some(streams),
        )));
    }

    fn stop_background(&self) {
        let mut state = lock(&self.state);
        state.epoch += 1;
        for task in state.tasks.drain(..) {
            task.abort();
        }
        if let Some(pause) = state.pause.take() {
            pause.abort();
        }
        state.session = None;
    }

    fn current(&self, epoch: u64) -> bool {
        let state = lock(&self.state);
        state.epoch == epoch && state.status != Status::Fatal
    }

    async fn fetch(
        &self,
        kind: RefreshKind,
        hint: Option<&Hint>,
    ) -> Result<Option<BulkEvaluation>, FlagshipError> {
        let (context, connected) = {
            let state = lock(&self.state);
            if let Some(until) = state.blocked_until
                && let Some(wait) = until.checked_duration_since(Instant::now())
            {
                return Err(FlagshipError::new(
                    ErrorCode::General,
                    "bulk evaluation is rate limited until the Retry-After time",
                )
                .with_status(429)
                .with_retryable(false)
                .with_retry_after(Some(wait)));
            }
            let connected = state
                .session
                .as_ref()
                .is_some_and(|control| control.connected.load(Ordering::Acquire));
            (state.context.clone(), connected)
        };
        let normalized = normalize_context(&context)?;
        let body = serde_json::to_vec(&BulkBody {
            context: normalized.values(),
        })
        .map_err(|source| {
            FlagshipError::new(ErrorCode::InvalidContext, "context could not be serialized")
                .with_source(source)
        })?;
        let etag = read(&self.snapshot)
            .as_ref()
            .and_then(|snapshot| snapshot.etag.clone());
        let sse = self.change_detection == ChangeDetection::Sse;
        let mut url = self.bulk_url.clone();
        let mut headers = HeaderMap::new();
        let request_stream = match kind {
            RefreshKind::Discover => sse,
            RefreshKind::Revalidate => sse && !connected,
            RefreshKind::Hint => false,
        };
        if request_stream {
            headers.insert(EVENT_STREAM_HEADER, HeaderValue::from_static("sse"));
        }
        if kind != RefreshKind::Discover
            && let Some(etag) = etag
        {
            headers.insert(IF_NONE_MATCH, etag);
        }
        if let Some(hint) = hint {
            let mut query = url.query_pairs_mut();
            if let Some(etag) = &hint.etag {
                query.append_pair("flagConfigEtag", etag);
            }
            if let Some(last_modified) = &hint.last_modified {
                query.append_pair("flagConfigLastModified", last_modified);
            }
        }
        let request = RawRequest::new(Method::POST, url, BULK_BODY_LIMIT, headers)
            .with_body(body)
            .respecting_retry_after();
        match self.client.send_raw(&request).await {
            Ok(response) if response.status == StatusCode::NOT_MODIFIED => Ok(None),
            Ok(response) => parse_bulk(&context, &response).map(Some),
            Err(error) => {
                if let Some(wait) = error.retry_after() {
                    lock(&self.state).blocked_until = Some(Instant::now() + wait);
                }
                Err(error)
            }
        }
    }

    /// Applies one refresh. Returns the response's event streams (empty for a
    /// `304`), or `None` when the refresh failed or was superseded.
    async fn refresh(
        self: &Arc<Self>,
        epoch: u64,
        kind: RefreshKind,
        hint: Option<&Hint>,
        notification: bool,
    ) -> Option<Vec<EventStream>> {
        let _serial = self.refresh.lock().await;
        if !self.current(epoch) {
            return None;
        }
        let result = match self.fetch(kind, hint).await {
            Ok(result) => result,
            Err(error) => {
                if self.current(epoch) {
                    if is_auth_error(&error) {
                        self.fail(error.to_string());
                    } else {
                        self.mark_stale(error.to_string());
                    }
                }
                return None;
            }
        };
        if !self.current(epoch) {
            return None;
        }
        self.recover();
        let Some(result) = result else {
            return Some(Vec::new());
        };
        let current = Arc::new(result.snapshot);
        let previous = write(&self.snapshot).replace(Arc::clone(&current));
        self.emit_changed(previous.as_deref(), &current, notification);
        if self.change_detection == ChangeDetection::Sse {
            let mut state = lock(&self.state);
            if state.epoch == epoch
                && state.session.is_none()
                && !stream_urls(&result.streams, &self.bulk_url).is_empty()
            {
                self.start_session(&mut state, epoch, result.streams.clone());
            }
        }

        Some(result.streams)
    }

    /// Refreshes until the observed version matches the hinted one.
    async fn refresh_for(self: &Arc<Self>, epoch: u64, hint: &Hint) {
        for attempt in 0..=HINT_RETRY_DELAYS.len() {
            if let Some(delay) = attempt.checked_sub(1).map(|index| HINT_RETRY_DELAYS[index]) {
                tokio::time::sleep(delay).await;
            }
            if self
                .refresh(epoch, RefreshKind::Hint, Some(hint), attempt == 0)
                .await
                .is_none()
            {
                continue;
            }
            let version = read(&self.snapshot)
                .as_ref()
                .and_then(|snapshot| snapshot.version.clone());
            if hint.etag.is_none() || version == hint.etag {
                return;
            }
        }
    }

    fn emit_changed(&self, previous: Option<&Snapshot>, current: &Snapshot, force: bool) {
        let empty = HashMap::new();
        let old = previous.map_or(&empty, |snapshot| &snapshot.raw);
        let mut changed: Vec<String> = old
            .keys()
            .chain(current.raw.keys())
            .filter(|key| old.get(*key) != current.raw.get(*key))
            .cloned()
            .collect();
        changed.sort();
        changed.dedup();
        let version_changed = previous.is_some_and(|previous| {
            current.version.is_some() && current.version != previous.version
        });
        if force || version_changed || !changed.is_empty() {
            self.emit(ClientProviderEvent::ConfigurationChanged {
                flags_changed: changed,
            });
        }
    }

    fn mark_stale(&self, message: String) {
        let mut state = lock(&self.state);
        if state.status != Status::Ready {
            return;
        }
        state.status = Status::Stale;
        drop(state);
        self.emit(ClientProviderEvent::Stale { message });
    }

    fn recover(&self) {
        let mut state = lock(&self.state);
        if state.status != Status::Stale {
            return;
        }
        state.status = Status::Ready;
        drop(state);
        self.emit(ClientProviderEvent::Ready);
    }

    /// Stops background refreshes after the server rejects the credentials.
    fn fail(&self, message: String) {
        let mut state = lock(&self.state);
        state.status = Status::Fatal;
        state.session = None;
        let tasks = std::mem::take(&mut state.tasks);
        drop(state);
        self.emit(ClientProviderEvent::Fatal { message });
        // Aborting may cancel the calling task at its next await.
        for task in tasks {
            task.abort();
        }
    }

    fn emit(&self, event: ClientProviderEvent) {
        let _ = self.events.send(event);
    }
}

async fn poll_loop(weak: Weak<Inner>, epoch: u64, interval: Duration) {
    loop {
        tokio::time::sleep(interval).await;
        let Some(inner) = weak.upgrade() else {
            return;
        };
        if !inner.current(epoch) {
            return;
        }
        let connected = lock(&inner.state)
            .session
            .as_ref()
            .is_some_and(|control| control.connected.load(Ordering::Acquire));
        if !(inner.change_detection == ChangeDetection::Sse && connected) {
            inner
                .refresh(epoch, RefreshKind::Revalidate, None, false)
                .await;
        }
    }
}

/// Holds every advertised event stream open. Stream URLs carry single-use
/// tickets, so a dropped connection is replaced by rediscovering fresh
/// descriptors, never by reopening an old URL.
struct SessionControl {
    connected: AtomicBool,
    suspended: AtomicBool,
    wake: Notify,
    inactivity: Mutex<Option<Duration>>,
    stop: Mutex<Option<mpsc::Sender<StreamItem>>>,
}

impl SessionControl {
    fn new(inactivity: Option<Duration>) -> Self {
        Self {
            connected: AtomicBool::new(false),
            suspended: AtomicBool::new(false),
            wake: Notify::new(),
            inactivity: Mutex::new(inactivity),
            stop: Mutex::new(None),
        }
    }

    fn suspend(&self) {
        self.suspended.store(true, Ordering::Release);
        if let Some(sender) = lock(&self.stop).as_ref() {
            let _ = sender.try_send(StreamItem::Stop);
        }
    }

    fn resume(&self) {
        if self.suspended.swap(false, Ordering::AcqRel) {
            self.wake.notify_one();
        }
    }
}

enum StreamItem {
    Opened,
    Hint(Hint),
    Closed,
    Stop,
}

/// Aborts reader tasks when a listening round ends or its task is aborted.
struct Readers(Vec<JoinHandle<()>>);

impl Drop for Readers {
    fn drop(&mut self) {
        for reader in &self.0 {
            reader.abort();
        }
    }
}

async fn run_session(
    weak: Weak<Inner>,
    epoch: u64,
    control: Arc<SessionControl>,
    mut descriptors: Option<Vec<EventStream>>,
) {
    let mut delay = MIN_RECONNECT_DELAY;
    loop {
        if control.suspended.load(Ordering::Acquire) {
            control.wake.notified().await;
            continue;
        }
        let Some(inner) = weak.upgrade() else {
            return;
        };
        if !inner.current(epoch) {
            return;
        }
        let streams = match descriptors.take() {
            Some(streams) => streams,
            // This unconditional refetch doubles as the catch-up refetch OFREP
            // requires after reconnecting.
            None => inner
                .refresh(epoch, RefreshKind::Discover, None, false)
                .await
                .unwrap_or_default(),
        };
        let urls = stream_urls(&streams, &inner.bulk_url);
        if let Some(delay) = inactivity_delay(&streams) {
            *lock(&control.inactivity) = Some(delay);
        }
        let client = inner.client.clone();
        drop(inner);
        if !urls.is_empty() && listen(&weak, epoch, &control, &client, urls).await {
            delay = MIN_RECONNECT_DELAY;
        }
        if control.suspended.load(Ordering::Acquire) {
            continue;
        }
        let Some(inner) = weak.upgrade() else {
            return;
        };
        if !inner.current(epoch) {
            return;
        }
        inner.mark_stale("configuration stream interrupted".to_owned());
        drop(inner);
        tokio::time::sleep(jittered(delay)).await;
        delay = (delay * 2).min(MAX_RECONNECT_DELAY);
    }
}

/// Holds every stream until one ends; returns whether all of them connected.
async fn listen(
    weak: &Weak<Inner>,
    epoch: u64,
    control: &SessionControl,
    client: &FlagshipClient,
    urls: Vec<Url>,
) -> bool {
    let total = urls.len();
    let (sender, mut receiver) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    *lock(&control.stop) = Some(sender.clone());
    let readers = Readers(
        urls.into_iter()
            .map(|url| tokio::spawn(read_stream(client.clone(), url, sender.clone())))
            .collect(),
    );
    drop(sender);
    let mut opened = 0;
    let mut all_connected = false;
    let mut on_open = |opened: &mut usize| {
        *opened += 1;
        if *opened == total {
            control.connected.store(true, Ordering::Release);
            all_connected = true;
        }
    };
    loop {
        let Some(item) = receiver.recv().await else {
            break;
        };
        let mut hint = match item {
            StreamItem::Opened => {
                on_open(&mut opened);
                continue;
            }
            StreamItem::Closed | StreamItem::Stop => break,
            StreamItem::Hint(hint) => hint,
        };
        // Coalesce bursts, including one change announced on several streams.
        let deadline = Instant::now() + COALESCE_WINDOW;
        let mut closed = false;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match tokio::time::timeout(remaining, receiver.recv()).await {
                Ok(Some(StreamItem::Hint(next))) => hint = next,
                Ok(Some(StreamItem::Opened)) => on_open(&mut opened),
                Ok(Some(StreamItem::Closed | StreamItem::Stop) | None) => {
                    closed = true;
                    break;
                }
                Err(_) => break,
            }
        }
        let Some(inner) = weak.upgrade() else {
            break;
        };
        inner.refresh_for(epoch, &hint).await;
        drop(inner);
        if closed {
            break;
        }
    }
    control.connected.store(false, Ordering::Release);
    *lock(&control.stop) = None;
    drop(readers);

    all_connected
}

async fn read_stream(client: FlagshipClient, url: Url, sender: mpsc::Sender<StreamItem>) {
    if let Ok(mut response) = client.open_stream(url).await {
        let _ = sender.send(StreamItem::Opened).await;
        let mut parser = SseParser::default();
        while let Ok(Some(chunk)) = response.chunk().await {
            let Ok(hints) = parser.push(&chunk) else {
                break;
            };
            for hint in hints {
                if sender.send(StreamItem::Hint(hint)).await.is_err() {
                    return;
                }
            }
        }
    }
    let _ = sender.send(StreamItem::Closed).await;
}

/// Incremental server-sent events parser yielding OFREP `refetchEvaluation` hints.
#[derive(Default)]
struct SseParser {
    buffer: Vec<u8>,
    data: String,
    event: String,
}

impl SseParser {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Hint>, ()> {
        self.buffer.extend_from_slice(chunk);
        let mut hints = Vec::new();
        while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                hints.extend(self.dispatch());
            } else if let Some(data) = line.strip_prefix("data:") {
                if self.data.len() + data.len() > EVENT_LIMIT {
                    return Err(());
                }
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
            } else if let Some(event) = line.strip_prefix("event:") {
                event.trim().clone_into(&mut self.event);
            }
        }
        if self.buffer.len() > EVENT_LIMIT {
            return Err(());
        }

        Ok(hints)
    }

    fn dispatch(&mut self) -> Option<Hint> {
        let data = std::mem::take(&mut self.data);
        let event = std::mem::take(&mut self.event);
        if data.is_empty() || !(event.is_empty() || event == "message") {
            return None;
        }
        let JsonValue::Object(message) = serde_json::from_str::<JsonValue>(&data).ok()? else {
            return None;
        };
        if message.get("type").and_then(JsonValue::as_str) != Some("refetchEvaluation") {
            return None;
        }
        let etag = message
            .get("etag")
            .and_then(JsonValue::as_str)
            .filter(|etag| !etag.is_empty() && etag.len() <= METADATA_LIMIT)
            .map(str::to_owned);

        Some(Hint {
            etag,
            last_modified: message.get("lastModified").and_then(last_modified),
        })
    }
}

/// Validates `lastModified`: Unix seconds or an RFC 3339 date-time, per OFREP.
fn last_modified(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::Number(number) => number.as_u64().map(|seconds| seconds.to_string()),
        JsonValue::String(date) if date.len() <= METADATA_LIMIT && is_rfc3339(date) => {
            Some(date.clone())
        }
        _ => None,
    }
}

fn is_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        bytes
            .get(range)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    };
    bytes.len() >= 20
        && digits(0..4)
        && bytes[4] == b'-'
        && digits(5..7)
        && bytes[7] == b'-'
        && digits(8..10)
        && matches!(bytes[10], b'T' | b't' | b' ')
        && digits(11..13)
        && bytes[13] == b':'
        && digits(14..16)
        && bytes[16] == b':'
        && digits(17..19)
        && (value.ends_with(['Z', 'z'])
            || value.len() >= 25 && {
                let offset = &bytes[bytes.len() - 6..];
                matches!(offset[0], b'+' | b'-') && offset[3] == b':'
            })
}

/// Resolves every usable SSE descriptor, ignoring unknown types and URLs that
/// are neither HTTPS nor loopback HTTP.
fn stream_urls(streams: &[EventStream], base: &Url) -> Vec<Url> {
    let mut urls: Vec<Url> = Vec::new();
    for stream in streams.iter().filter(|stream| stream.kind == "sse") {
        let resolved = match (&stream.url, &stream.endpoint) {
            (Some(url), _) => Url::parse(url).ok(),
            (None, Some(endpoint)) => {
                let origin = match &endpoint.origin {
                    Some(origin) => Url::parse(origin).ok(),
                    None => Some(base.clone()),
                };
                origin.and_then(|origin| origin.join(&endpoint.request_uri).ok())
            }
            (None, None) => None,
        };
        let Some(url) = resolved else {
            continue;
        };
        let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1"));
        let safe = (url.scheme() == "https" || url.scheme() == "http" && loopback)
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none();
        if safe && !urls.contains(&url) {
            urls.push(url);
        }
    }

    urls
}

fn inactivity_delay(streams: &[EventStream]) -> Option<Duration> {
    streams
        .iter()
        .find_map(|stream| stream.inactivity_delay_sec.filter(|seconds| *seconds >= 1))
        .map(Duration::from_secs)
}

fn parse_bulk(
    context: &EvaluationContext,
    response: &RawResponse,
) -> Result<BulkEvaluation, FlagshipError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Envelope {
        flags: Vec<JsonValue>,
        #[serde(default)]
        metadata: Map<String, JsonValue>,
        #[serde(default)]
        event_streams: Vec<JsonValue>,
    }
    let invalid = || {
        FlagshipError::new(ErrorCode::ParseError, "invalid bulk evaluation response")
            .with_status(response.status.as_u16())
    };
    let envelope: Envelope = serde_json::from_slice(&response.body).map_err(|_| invalid())?;
    let mut flags = HashMap::with_capacity(envelope.flags.len());
    let mut raw = HashMap::with_capacity(envelope.flags.len());
    for item in envelope.flags {
        let Some(key) = item
            .get("key")
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        let serialized = item.to_string();
        if let Ok(flag) = serde_json::from_value::<BulkFlag>(item) {
            flags.insert(key.clone(), flag);
            raw.insert(key, serialized);
        }
    }
    let streams = envelope
        .event_streams
        .into_iter()
        .filter_map(|stream| serde_json::from_value(stream).ok())
        .collect();
    let snapshot = Snapshot {
        context_key: context_key(context)?,
        flags,
        raw,
        etag: response
            .headers
            .get(ETAG)
            .filter(|value| !value.is_empty())
            .cloned(),
        version: envelope
            .metadata
            .get("version")
            .and_then(JsonValue::as_str)
            .filter(|version| !version.is_empty())
            .map(str::to_owned),
    };

    Ok(BulkEvaluation { snapshot, streams })
}

fn bulk_error(code: &str) -> EvaluationError {
    let code = match code {
        "FLAG_NOT_FOUND" => EvaluationErrorCode::FlagNotFound,
        "PARSE_ERROR" => EvaluationErrorCode::ParseError,
        "TYPE_MISMATCH" => EvaluationErrorCode::TypeMismatch,
        "TARGETING_KEY_MISSING" => EvaluationErrorCode::TargetingKeyMissing,
        "INVALID_CONTEXT" => EvaluationErrorCode::InvalidContext,
        _ => EvaluationErrorCode::General("GENERAL".to_owned()),
    };
    // Server-provided details are not forwarded, so they never reach OpenFeature logs.
    let message = format!("{code}: the bulk evaluation reported an error for this flag");

    EvaluationError {
        code,
        message: Some(message),
    }
}

fn is_auth_error(error: &FlagshipError) -> bool {
    matches!(error.status(), Some(401 | 403))
}

fn has_values(context: &EvaluationContext) -> bool {
    context
        .targeting_key
        .as_deref()
        .is_some_and(|key| !key.is_empty())
        || !context.custom_fields.is_empty()
}

fn context_key(context: &EvaluationContext) -> Result<String, FlagshipError> {
    let normalized = normalize_context(context)?;
    serde_json::to_string(normalized.values()).map_err(|source| {
        FlagshipError::new(ErrorCode::InvalidContext, "context could not be serialized")
            .with_source(source)
    })
}

/// A delay between half and one-and-a-half times `delay`.
fn jittered(delay: Duration) -> Duration {
    let entropy = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos() % 1_000);

    delay / 2 + delay * entropy / 1_000
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T: Clone>(lock: &RwLock<T>) -> T {
    lock.read().unwrap_or_else(PoisonError::into_inner).clone()
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(all(
    test,
    not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    ))
))]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::client::parse_retry_after;
    use crate::config::TimeoutPolicy;

    #[derive(Clone, Debug)]
    struct Request {
        path: String,
        headers: HashMap<String, String>,
        body: String,
    }

    impl Request {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.get(name).map(String::as_str)
        }

        fn query(&self, name: &str) -> Option<String> {
            Url::parse(&format!("http://test{}", self.path))
                .ok()?
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        }
    }

    enum Reply {
        Status(u16, Vec<(&'static str, String)>, String),
        Stream(String, bool),
    }

    struct Server {
        base: String,
        requests: Arc<Mutex<Vec<Request>>>,
        release: Arc<AtomicBool>,
    }

    impl Server {
        fn start(handler: impl Fn(&Request, usize) -> Reply + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
            let base = format!("http://{}", listener.local_addr().expect("address"));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let release = Arc::new(AtomicBool::new(false));
            let handler = Arc::new(handler);
            let bulk_calls = Arc::new(AtomicUsize::new(0));
            let (recorded, released) = (Arc::clone(&requests), Arc::clone(&release));
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { return };
                    let (handler, recorded, released, bulk_calls) = (
                        Arc::clone(&handler),
                        Arc::clone(&recorded),
                        Arc::clone(&released),
                        Arc::clone(&bulk_calls),
                    );
                    std::thread::spawn(move || {
                        let Some(request) = read_request(&mut stream) else {
                            return;
                        };
                        let call = if request.path.starts_with("/v1/acct/apps/app/ofrep/") {
                            lock(&recorded).push(request.clone());
                            bulk_calls.fetch_add(1, Ordering::SeqCst) + 1
                        } else {
                            0
                        };
                        match handler(&request, call) {
                            Reply::Status(status, headers, body) => {
                                let mut head = format!(
                                    "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
                                    body.len()
                                );
                                for (name, value) in headers {
                                    let _ = std::fmt::Write::write_fmt(
                                        &mut head,
                                        format_args!("{name}: {value}\r\n"),
                                    );
                                }
                                let _ = write!(stream, "{head}\r\n{body}");
                            }
                            Reply::Stream(body, hold) => {
                                let _ = write!(
                                    stream,
                                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
                                );
                                let _ = stream.flush();
                                while hold && !released.load(Ordering::SeqCst) {
                                    std::thread::sleep(Duration::from_millis(10));
                                }
                            }
                        }
                    });
                }
            });

            Self {
                base,
                requests,
                release,
            }
        }

        fn endpoint(&self) -> String {
            format!("{}/v1/acct/apps/app/evaluate", self.base)
        }

        fn calls(&self) -> Vec<Request> {
            lock(&self.requests).clone()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.release.store(true, Ordering::SeqCst);
        }
    }

    fn read_request(stream: &mut std::net::TcpStream) -> Option<Request> {
        let mut data = Vec::new();
        let mut buffer = [0_u8; 4096];
        let head_end = loop {
            if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                break end;
            }
            let count = stream.read(&mut buffer).ok()?;
            if count == 0 {
                return None;
            }
            data.extend_from_slice(&buffer[..count]);
        };
        let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
        let mut lines = head.split("\r\n");
        let path = lines.next()?.split(' ').nth(1)?.to_owned();
        let headers: HashMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let length: usize = headers
            .get("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let mut body = data[head_end + 4..].to_vec();
        while body.len() < length {
            let count = stream.read(&mut buffer).ok()?;
            if count == 0 {
                break;
            }
            body.extend_from_slice(&buffer[..count]);
        }

        Some(Request {
            path,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }

    fn bulk(etag: &str, version: &str, flags: &str, streams: Option<String>) -> Reply {
        let streams = streams.map_or_else(String::new, |streams| {
            format!(",\"eventStreams\":{streams}")
        });
        Reply::Status(
            200,
            vec![("ETag", etag.to_owned())],
            format!("{{\"flags\":{flags},\"metadata\":{{\"version\":\"{version}\"}}{streams}}}"),
        )
    }

    fn status(code: u16) -> Reply {
        Reply::Status(code, Vec::new(), String::new())
    }

    fn provider(
        server: &Server,
        configure: impl FnOnce(FlagshipClientProviderBuilder) -> FlagshipClientProviderBuilder,
    ) -> FlagshipClientProvider {
        let options = FlagshipClientOptions::builder()
            .endpoint(server.endpoint())
            .auth_token("token")
            .retries(0)
            .build()
            .expect("options");
        configure(FlagshipClientProvider::builder(options))
            .build()
            .expect("provider")
    }

    async fn wait_until(mut condition: impl FnMut() -> bool) {
        for _ in 0..300 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition not met");
    }

    async fn next_event(
        events: &mut broadcast::Receiver<ClientProviderEvent>,
        matches: impl Fn(&ClientProviderEvent) -> bool,
    ) -> ClientProviderEvent {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let event = events.recv().await.expect("event");
                if matches(&event) {
                    return event;
                }
            }
        })
        .await
        .expect("event before timeout")
    }

    fn user(key: &str) -> EvaluationContext {
        EvaluationContext::default().with_targeting_key(key)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn caches_one_bulk_evaluation_and_resolves_without_requests() {
        let flags = r#"[{"key":"bool","value":true,"variant":"on","reason":"TARGETING_MATCH"},
            {"key":"num","value":3,"reason":"SPLIT"},{"key":"off","reason":"DISABLED"},
            {"key":"broken","errorCode":"PARSE_ERROR","errorDetails":"secret server text"}]"#;
        let server = Server::start(move |_, _| bulk("\"e1\"", "v1", flags, None));
        let provider = provider(&server, |builder| {
            builder.change_detection(ChangeDetection::None)
        });
        provider
            .initialize_provider(&user("a"))
            .await
            .expect("initialize");

        let details = provider
            .resolve_bool_value("bool", &user("a"))
            .await
            .expect("bool");
        assert!(details.value);
        assert_eq!(details.variant.as_deref(), Some("on"));
        assert_eq!(
            provider
                .resolve_int_value("num", &EvaluationContext::default())
                .await
                .expect("num")
                .value,
            3
        );
        assert!(
            provider
                .resolve_bool_value("off", &user("a"))
                .await
                .is_err()
        );
        let broken = provider
            .resolve_bool_value("broken", &user("a"))
            .await
            .expect_err("broken");
        assert_eq!(broken.code, EvaluationErrorCode::ParseError);
        assert!(!broken.message.unwrap_or_default().contains("secret"));
        assert_eq!(
            provider
                .resolve_string_value("bool", &user("a"))
                .await
                .expect_err("type")
                .code,
            EvaluationErrorCode::TypeMismatch
        );
        assert_eq!(
            provider
                .resolve_bool_value("missing", &user("a"))
                .await
                .expect_err("missing")
                .code,
            EvaluationErrorCode::FlagNotFound
        );
        assert_eq!(
            provider
                .resolve_bool_value("bool", &user("b"))
                .await
                .expect_err("context")
                .code,
            EvaluationErrorCode::InvalidContext
        );
        let calls = server.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].header("authorization"), Some("Bearer token"));
        assert_eq!(calls[0].body, r#"{"context":{"targetingKey":"a"}}"#);
        provider.shutdown_provider();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn supports_generic_ofrep_base_urls_and_a_ten_second_default_timeout() {
        let options = FlagshipClientOptions::builder()
            .ofrep_base_url("https://flags.example.com/api/")
            .build()
            .expect("options");
        assert_eq!(
            resolve_bulk_endpoint(options.evaluation_url())
                .expect("bulk")
                .as_str(),
            "https://flags.example.com/api/ofrep/v1/evaluate/flags"
        );
        assert_eq!(
            options
                .clone()
                .with_default_timeout(DEFAULT_TIMEOUT)
                .timeout(),
            TimeoutPolicy::Duration(DEFAULT_TIMEOUT)
        );
        let explicit = FlagshipClientOptions::builder()
            .endpoint("https://flags.example.com/evaluate")
            .timeout(Duration::from_secs(2))
            .build()
            .expect("options");
        assert_eq!(
            explicit.with_default_timeout(DEFAULT_TIMEOUT).timeout(),
            TimeoutPolicy::Duration(Duration::from_secs(2))
        );
        assert!(
            FlagshipClientOptions::builder()
                .ofrep_base_url("https://a.example.com")
                .app_id("app")
                .account_id("acct")
                .build()
                .is_err()
        );
        assert!(
            FlagshipClientOptions::builder()
                .ofrep_base_url("ftp://a.example.com")
                .build()
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn identify_is_unconditional_and_clears_the_cache_on_failure() {
        let server = Server::start(|request, call| match call {
            1 => bulk("\"e1\"", "v1", r#"[{"key":"f","value":false}]"#, None),
            2 => {
                assert!(request.header("if-none-match").is_none());
                bulk("\"e2\"", "v1", r#"[{"key":"f","value":true}]"#, None)
            }
            _ => status(500),
        });
        let provider = provider(&server, |builder| {
            builder.change_detection(ChangeDetection::None)
        });
        let mut events = provider.subscribe();
        provider
            .initialize_provider(&user("a"))
            .await
            .expect("initialize");
        provider.identify(&user("b")).await.expect("identify");

        let event = next_event(&mut events, |_| true).await;
        assert_eq!(
            event,
            ClientProviderEvent::ConfigurationChanged {
                flags_changed: vec!["f".to_owned()]
            }
        );
        assert!(
            provider
                .resolve_bool_value("f", &user("b"))
                .await
                .expect("value")
                .value
        );
        assert!(provider.identify(&user("c")).await.is_err());
        assert_eq!(
            provider
                .resolve_bool_value("f", &user("c"))
                .await
                .expect_err("cleared")
                .code,
            EvaluationErrorCode::ProviderNotReady
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn polling_is_opt_in_and_revalidates_with_the_response_etag() {
        let server = Server::start(|_, call| match call {
            1 => bulk("\"e1\"", "v1", r#"[{"key":"f","value":true}]"#, None),
            2 => status(304),
            _ => bulk("\"e2\"", "v2", r#"[{"key":"f","value":false}]"#, None),
        });
        let provider = provider(&server, |builder| {
            builder
                .change_detection(ChangeDetection::Polling)
                .poll_interval(Duration::from_millis(50))
        });
        let mut events = provider.subscribe();
        provider
            .initialize_provider(&EvaluationContext::default())
            .await
            .expect("initialize");
        let event = next_event(&mut events, |_| true).await;
        assert_eq!(
            event,
            ClientProviderEvent::ConfigurationChanged {
                flags_changed: vec!["f".to_owned()]
            }
        );
        assert_eq!(server.calls()[1].header("if-none-match"), Some("\"e1\""));
        assert!(
            !provider
                .resolve_bool_value("f", &EvaluationContext::default())
                .await
                .expect("value")
                .value
        );
        provider.shutdown_provider();

        let idle = Server::start(|_, _| bulk("\"e\"", "v", "[]", None));
        let quiet = self::provider(&idle, |builder| builder);
        quiet
            .initialize_provider(&EvaluationContext::default())
            .await
            .expect("initialize");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(idle.calls().len(), 1);
        quiet.shutdown_provider();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn auth_failures_are_fatal_and_stop_polling() {
        let server = Server::start(|_, call| {
            if call == 1 {
                bulk("\"e\"", "v", "[]", None)
            } else {
                status(401)
            }
        });
        let provider = provider(&server, |builder| {
            builder.poll_interval(Duration::from_millis(50))
        });
        let mut events = provider.subscribe();
        provider
            .initialize_provider(&EvaluationContext::default())
            .await
            .expect("initialize");
        next_event(&mut events, |event| {
            matches!(event, ClientProviderEvent::Fatal { .. })
        })
        .await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(server.calls().len(), 2);
        assert_eq!(provider.status(), ProviderStatus::Error);

        let rejected = Server::start(|_, _| status(403));
        let initial = self::provider(&rejected, |builder| builder);
        assert_eq!(
            initial
                .initialize_provider(&EvaluationContext::default())
                .await
                .expect_err("forbidden")
                .status(),
            Some(403)
        );
        assert_eq!(initial.status(), ProviderStatus::Error);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn honors_retry_after() {
        let server = Server::start(|_, call| match call {
            1 => bulk("\"e\"", "v", "[]", None),
            2 => Reply::Status(429, vec![("Retry-After", "1".to_owned())], String::new()),
            _ => status(304),
        });
        let options = FlagshipClientOptions::builder()
            .endpoint(server.endpoint())
            .retries(3)
            .retry_delay(Duration::from_millis(1))
            .build()
            .expect("options");
        let provider = FlagshipClientProvider::builder(options)
            .poll_interval(Duration::from_millis(50))
            .build()
            .expect("provider");
        let mut events = provider.subscribe();
        provider
            .initialize_provider(&EvaluationContext::default())
            .await
            .expect("initialize");
        next_event(&mut events, |event| {
            matches!(event, ClientProviderEvent::Stale { .. })
        })
        .await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(server.calls().len(), 2);
        next_event(&mut events, |event| *event == ClientProviderEvent::Ready).await;
        provider.shutdown_provider();

        let now = UNIX_EPOCH + Duration::from_secs(1_771_622_880);
        assert_eq!(
            parse_retry_after("Fri, 20 Feb 2026 21:28:18 GMT", now),
            Some(Duration::from_secs(18))
        );
        assert_eq!(parse_retry_after("7", now), Some(Duration::from_secs(7)));
        assert_eq!(parse_retry_after("soon", now), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sse_hints_forward_metadata_across_every_stream() {
        let base = Arc::new(Mutex::new(String::new()));
        let shared = Arc::clone(&base);
        let server = Server::start(move |request, _| {
            if !request.path.starts_with("/v1/") {
                let events = "data: {\"type\":\"unknown\"}\n\ndata: {\"type\":\"refetchEvaluation\",\"etag\":\"v2\",\"lastModified\":1771622898}\n\n";
                return Reply::Stream(events.to_owned(), true);
            }
            if request.query("flagConfigEtag").as_deref() == Some("v2") {
                return bulk("\"e2\"", "v2", r#"[{"key":"f","value":true}]"#, None);
            }
            let base = lock(&shared).clone();
            let streams = format!(
                r#"[{{"type":"sse","url":"{base}/a?ticket=1"}},{{"type":"websocket","url":"wss://x.example.com"}},{{"type":"sse","endpoint":{{"requestUri":"/b?ticket=2"}}}}]"#
            );
            bulk(
                "\"e1\"",
                "v1",
                r#"[{"key":"f","value":false}]"#,
                Some(streams),
            )
        });
        base.lock().expect("base").clone_from(&server.base);
        let provider = provider(&server, |builder| builder);
        let mut events = provider.subscribe();
        provider
            .initialize_provider(&EvaluationContext::default())
            .await
            .expect("initialize");

        next_event(&mut events, |event| {
            matches!(event, ClientProviderEvent::ConfigurationChanged { .. })
        })
        .await;
        assert!(
            provider
                .resolve_bool_value("f", &EvaluationContext::default())
                .await
                .expect("value")
                .value
        );
        let calls = server.calls();
        assert_eq!(calls[0].header("flagship-event-stream"), Some("sse"));
        let hinted: Vec<_> = calls
            .iter()
            .filter(|call| call.query("flagConfigEtag").as_deref() == Some("v2"))
            .collect();
        assert_eq!(
            hinted.len(),
            1,
            "two streams announcing one change coalesce"
        );
        assert_eq!(
            hinted[0].query("flagConfigLastModified").as_deref(),
            Some("1771622898")
        );
        assert_eq!(hinted[0].header("if-none-match"), Some("\"e1\""));
        provider.shutdown_provider();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reconnects_with_a_fresh_ticket_unconditionally() {
        let base = Arc::new(Mutex::new(String::new()));
        let shared = Arc::clone(&base);
        let opened = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&opened);
        let server = Server::start(move |request, call| {
            if request.path.starts_with("/s") {
                lock(&seen).push(request.path.clone());
                return Reply::Stream(String::new(), false);
            }
            let base = lock(&shared).clone();
            bulk(
                "\"e\"",
                "v1",
                "[]",
                Some(format!(
                    r#"[{{"type":"sse","url":"{base}/s?ticket={call}"}}]"#
                )),
            )
        });
        base.lock().expect("base").clone_from(&server.base);
        let provider = provider(&server, |builder| builder);
        provider
            .initialize_provider(&EvaluationContext::default())
            .await
            .expect("initialize");
        wait_until(|| lock(&opened).len() >= 2).await;
        assert_eq!(
            lock(&opened)[..2],
            ["/s?ticket=1".to_owned(), "/s?ticket=2".to_owned()]
        );
        let second = &server.calls()[1];
        assert!(second.header("if-none-match").is_none());
        assert_eq!(second.header("flagship-event-stream"), Some("sse"));
        provider.shutdown_provider();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pause_closes_idle_streams_and_resume_refetches_unconditionally() {
        let base = Arc::new(Mutex::new(String::new()));
        let shared = Arc::clone(&base);
        let server = Server::start(move |request, _| {
            if request.path.starts_with("/s") {
                return Reply::Stream(": connected\n\n".to_owned(), true);
            }
            let base = lock(&shared).clone();
            bulk(
                "\"e\"",
                "v1",
                "[]",
                Some(format!(
                    r#"[{{"type":"sse","url":"{base}/s?ticket=t","inactivityDelaySec":1}}]"#
                )),
            )
        });
        base.lock().expect("base").clone_from(&server.base);
        let provider = provider(&server, |builder| {
            builder.inactivity_delay(Duration::from_millis(50))
        });
        let mut events = provider.subscribe();
        provider
            .initialize_provider(&EvaluationContext::default())
            .await
            .expect("initialize");
        let control = lock(&provider.inner.state)
            .session
            .clone()
            .expect("session");
        wait_until(|| control.connected.load(Ordering::Acquire)).await;

        provider.pause();
        wait_until(|| !control.connected.load(Ordering::Acquire)).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(server.calls().len(), 1);
        provider.resume().await;
        wait_until(|| server.calls().len() == 2).await;
        assert!(server.calls()[1].header("if-none-match").is_none());
        assert!(
            matches!(
                events.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ),
            "a pause is not STALE"
        );
        provider.shutdown_provider();
    }

    #[test]
    fn parses_hints_and_last_modified() {
        let mut parser = SseParser::default();
        let hints = parser
            .push(b": keepalive\n\nevent: other\ndata: {\"type\":\"refetchEvaluation\"}\n\ndata: {\"type\":\"refetchEvaluation\",\r\ndata: \"etag\":\"v2\",\"lastModified\":\"2026-02-20T21:28:18Z\"}\r\n\r\n")
            .expect("parse");
        assert_eq!(
            hints,
            vec![Hint {
                etag: Some("v2".to_owned()),
                last_modified: Some("2026-02-20T21:28:18Z".to_owned())
            }]
        );
        assert_eq!(
            last_modified(&serde_json::json!(1_771_622_898)).as_deref(),
            Some("1771622898")
        );
        assert_eq!(last_modified(&serde_json::json!(-1)), None);
        assert_eq!(last_modified(&serde_json::json!("yesterday")), None);
        assert_eq!(
            last_modified(&serde_json::json!("2026-02-20T21:28:18+05:30")).as_deref(),
            Some("2026-02-20T21:28:18+05:30")
        );
    }
}
