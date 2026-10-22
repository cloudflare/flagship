//! Native asynchronous `OpenFeature` provider integration.
//!
//! `open-feature` 0.3 does not pass a default value to provider resolution
//! methods and returns provider errors directly. A disabled Flagship flag is a
//! successful resolution and returns its served value after exact type checking;
//! this deliberately differs from the Go provider's default-value fallback. The
//! same `open-feature` version has no null value and only exposes structured
//! objects at the resolution root. JSON nulls (including nested nulls),
//! unsupported numbers, and array roots consequently produce a type mismatch.

use std::fmt;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use open_feature::provider::{
    FeatureProvider, ProviderMetadata, ProviderStatus, ResolutionDetails,
};
use open_feature::{
    EvaluationContext, EvaluationError, EvaluationErrorCode, EvaluationReason as OpenFeatureReason,
    EvaluationResult, Hook, HookWrapper, StructValue, Value as OpenFeatureValue, async_trait,
};
use reqwest::header::HeaderValue;
use serde_json::Value as JsonValue;
use tokio::sync::watch;
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::{Interval, MissedTickBehavior};

use crate::cache::{CacheToken, ResponseCache};
use crate::client::FlagshipClient;
use crate::config::FlagshipClientOptions;
use crate::context::{ExpectedFlagType, NormalizedContext, normalize_context};
use crate::definitions::{DefinitionsFetchResult, DefinitionsFetcher, DefinitionsSnapshot};
use crate::endpoint::resolve_definitions_endpoint;
use crate::error::{ConfigError, ErrorCode, FlagshipError};
use crate::local::evaluate_flag;
use crate::types::{EvaluationReason, FlagshipEvaluationResponse};

const PROVIDER_NAME: &str = "Flagship Server Provider";
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_CACHE_MAX_ENTRIES: usize = 1_000;

const STATUS_NOT_READY: u8 = 0;
const STATUS_READY: u8 = 1;
const STATUS_ERROR: u8 = 2;
const STATUS_STALE: u8 = 3;

/// Builder for [`FlagshipServerProvider`].
pub struct FlagshipServerProviderBuilder {
    client_options: FlagshipClientOptions,
    local_evaluation: bool,
    refresh_interval: Duration,
    cache_ttl: Duration,
    cache_max_entries: usize,
    hooks: Vec<HookWrapper>,
}

impl FlagshipServerProviderBuilder {
    /// Enables or disables local evaluation, which defaults to disabled.
    ///
    /// Local mode requires an account ID and an evaluation endpoint from which
    /// the definitions endpoint can be derived. It cannot use the response cache.
    #[must_use]
    pub fn local_evaluation(mut self, enabled: bool) -> Self {
        self.local_evaluation = enabled;

        self
    }

    /// Sets the local definitions refresh period, which defaults to 30 seconds.
    ///
    /// This option is validated and used only in local mode.
    #[must_use]
    pub fn refresh_interval(mut self, interval: Duration) -> Self {
        self.refresh_interval = interval;

        self
    }

    /// Sets the remote cache TTL. It defaults to zero, which disables caching.
    ///
    /// The cache applies only in remote mode.
    #[must_use]
    pub fn cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;

        self
    }

    /// Sets the remote cache capacity, which defaults to 1000 entries.
    ///
    /// Zero is rejected when caching is enabled and otherwise ignored.
    #[must_use]
    pub fn cache_max_entries(mut self, max_entries: usize) -> Self {
        self.cache_max_entries = max_entries;

        self
    }

    /// Adds a provider hook.
    #[must_use]
    pub fn hook(mut self, hook: impl Hook) -> Self {
        self.hooks.push(HookWrapper::new(hook));

        self
    }

    /// Validates the provider configuration and constructs the provider.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] for incompatible modes, invalid limits, a missing
    /// local-evaluation account ID, or an endpoint that cannot produce the
    /// required client URLs.
    pub fn build(self) -> Result<FlagshipServerProvider, ConfigError> {
        if self.local_evaluation && self.refresh_interval.is_zero() {
            return Err(ConfigError::new(
                "refresh_interval must be greater than zero",
            ));
        }
        if self.local_evaluation && !self.cache_ttl.is_zero() {
            return Err(ConfigError::new(
                "response caching cannot be used with local evaluation",
            ));
        }
        if !self.cache_ttl.is_zero() && self.cache_max_entries == 0 {
            return Err(ConfigError::new(
                "cache_max_entries must be greater than zero when caching is enabled",
            ));
        }

        let local_config = if self.local_evaluation {
            let account_id = self
                .client_options
                .endpoint()
                .account_id()
                .ok_or_else(|| ConfigError::new("account_id is required for local evaluation"))?
                .to_owned();
            let definitions_endpoint =
                resolve_definitions_endpoint(self.client_options.endpoint())?;

            Some((account_id, definitions_endpoint))
        } else {
            None
        };

        // Client construction is intentionally last so provider-specific validation
        // fails before any fallible transport setup.
        let client = FlagshipClient::new(self.client_options)?;
        let mode = if let Some((account_id, definitions_endpoint)) = local_config {
            ProviderMode::Local {
                source: Arc::new(DefinitionsFetcher::new(client, definitions_endpoint)),
                account_id,
                refresh_interval: self.refresh_interval,
            }
        } else {
            let capacity = NonZeroUsize::new(self.cache_max_entries).unwrap_or(NonZeroUsize::MIN);
            let cache = ResponseCache::new(self.cache_ttl, capacity).map(Arc::new);

            ProviderMode::Remote {
                client,
                cache,
                cache_max_entries: self.cache_max_entries,
            }
        };
        let initial_status = if matches!(mode, ProviderMode::Remote { .. }) {
            STATUS_READY
        } else {
            STATUS_NOT_READY
        };
        let inner = ProviderInner {
            mode,
            status: AtomicU8::new(initial_status),
            generation: AtomicU64::new(0),
            lifecycle: Mutex::new(LifecycleState::default()),
        };

        Ok(FlagshipServerProvider {
            inner: Arc::new(inner),
            metadata: ProviderMetadata::new(PROVIDER_NAME),
            hooks: self.hooks,
        })
    }
}

impl fmt::Debug for FlagshipServerProviderBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlagshipServerProviderBuilder")
            .field(
                "mode",
                &if self.local_evaluation {
                    "local"
                } else {
                    "remote"
                },
            )
            .field("refresh_interval", &self.refresh_interval)
            .field("cache_ttl", &self.cache_ttl)
            .field("cache_max_entries", &self.cache_max_entries)
            .field("hook_count", &self.hooks.len())
            .finish_non_exhaustive()
    }
}

/// Native asynchronous Flagship `OpenFeature` server provider.
///
/// Remote providers are ready immediately. Local providers start not-ready and
/// require [`Self::initialize_provider`] to complete the first definitions fetch.
/// The refresh task runs until explicit shutdown or the last provider clone is
/// dropped. `open-feature` 0.3 does not call a provider shutdown method.
///
/// In `open-feature` 0.3, `set_provider` holds the provider-registry write lock
/// while trait initialization runs. `set_named_provider` removes an existing
/// provider first, initializes without that write lock, then inserts the new
/// provider; lookups for that missing name temporarily fall back to the default
/// provider.
///
/// Pre-initialize local providers when startup errors must be observed. Registering
/// a ready provider preserves its active session. Registering immediately after a
/// failed pre-initialization starts a new attempt. Registering while explicit
/// initialization is still running does not cancel it: registration waits for the
/// same initialization result. If that attempt later fails, its error result is
/// visible only to the explicit caller, while every clone observes
/// [`ProviderStatus::Error`]; no automatic retry follows. Likewise, a trait-path
/// initial failure can only set Error status because `open-feature` 0.3 cannot
/// return or automatically retry the error. Call [`Self::initialize_provider`]
/// explicitly, or register the failed provider again, to recover.
///
/// ```no_run
/// # use cloudflare_flagship::config::FlagshipClientOptions;
/// # use cloudflare_flagship::provider::FlagshipServerProvider;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let options = FlagshipClientOptions::builder()
///     .app_id("app")
///     .account_id("account")
///     .build()?;
/// let provider = FlagshipServerProvider::builder(options)
///     .local_evaluation(true)
///     .build()?;
/// provider.initialize_provider().await?;
/// open_feature::OpenFeature::singleton_mut()
///     .await
///     .set_named_provider("flagship", provider)
///     .await;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct FlagshipServerProvider {
    inner: Arc<ProviderInner>,
    metadata: ProviderMetadata,
    hooks: Vec<HookWrapper>,
}

impl FlagshipServerProvider {
    /// Constructs a remote-evaluation provider with default provider options.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the evaluation endpoint cannot be resolved.
    pub fn new(client_options: FlagshipClientOptions) -> Result<Self, ConfigError> {
        Self::builder(client_options).build()
    }

    /// Starts a provider builder.
    #[must_use]
    pub fn builder(client_options: FlagshipClientOptions) -> FlagshipServerProviderBuilder {
        FlagshipServerProviderBuilder {
            client_options,
            local_evaluation: false,
            refresh_interval: DEFAULT_REFRESH_INTERVAL,
            cache_ttl: Duration::ZERO,
            cache_max_entries: DEFAULT_CACHE_MAX_ENTRIES,
            hooks: Vec::new(),
        }
    }

    /// Initializes the provider and, in local mode, starts periodic refreshes.
    ///
    /// Reinitialization invalidates and cancels the previous session. A concurrent
    /// newer initialize or shutdown deterministically prevents an older fetch from
    /// publishing state.
    ///
    /// # Errors
    ///
    /// Returns the sanitized definitions-fetch failure in local mode, a general
    /// error if a definitions-fetch task ends unexpectedly, or a provider-not-ready
    /// error if this initialization was superseded. Dropping this method's future
    /// after its session is spawned does not cancel that session.
    ///
    /// # Panics
    ///
    /// Local initialization requires a Tokio runtime with time enabled. A missing
    /// runtime or time driver panics in the caller while spawning the session or
    /// creating its interval. Missing Tokio I/O support instead ends the isolated
    /// fetch task and returns a general error with provider status
    /// [`ProviderStatus::Error`].
    pub async fn initialize_provider(&self) -> Result<(), FlagshipError> {
        let ProviderMode::Local {
            source,
            account_id: _,
            refresh_interval,
        } = &self.inner.mode
        else {
            if self.status() != ProviderStatus::Ready {
                self.inner.transition_remote(STATUS_READY);
            }

            return Ok(());
        };

        let mut interval = tokio::time::interval(*refresh_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let (initialized, initialization) = watch::channel(InitializationState::Pending);
        let waiter = initialization.clone();
        let (generation, old_task) = self.inner.begin_session(Some(initialization));
        let mut install_guard = SessionInstallGuard::new(Arc::clone(&self.inner), generation);
        abort_task(old_task).await;

        let weak = Arc::downgrade(&self.inner);
        let source = Arc::clone(source);
        let task = tokio::spawn(run_local_session(
            weak,
            generation,
            source,
            interval,
            initialized,
        ));

        let Err(task) = self.inner.install_task(generation, task) else {
            install_guard.disarm();

            return finish_initialization(
                &self.inner,
                generation,
                wait_for_initialization(waiter).await,
            );
        };

        abort_task(Some(task)).await;

        Err(superseded_error())
    }

    /// Stops refresh work, clears snapshots and cache state, and leaves the
    /// provider not-ready.
    pub async fn shutdown_provider(&self) {
        if matches!(&self.inner.mode, ProviderMode::Remote { .. }) {
            self.inner.transition_remote(STATUS_NOT_READY);

            return;
        }

        let (_, task) = self.inner.begin_session(None);

        abort_task(task).await;
    }

    /// Returns the provider's current lifecycle status.
    #[must_use]
    pub fn status(&self) -> ProviderStatus {
        self.inner.provider_status()
    }

    async fn resolve_typed<T>(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
        expected_type: ExpectedFlagType,
        expected_name: &'static str,
        convert: impl FnOnce(&JsonValue) -> Result<T, ()>,
    ) -> EvaluationResult<ResolutionDetails<T>> {
        let (response, cache_token, generation) = match &self.inner.mode {
            ProviderMode::Remote { client, cache, .. } => {
                let generation = self.inner.remote_generation()?;
                let normalized = normalize_context(context).map_err(map_flagship_error)?;
                let (response, token) = evaluate_remote(
                    client,
                    cache.as_deref(),
                    flag_key,
                    &normalized,
                    expected_type,
                )
                .await?;

                (response, token, generation)
            }
            ProviderMode::Local { account_id, .. } => {
                let (generation, snapshot) = self.inner.local_snapshot()?;
                let definition = snapshot.get(flag_key).ok_or_else(flag_not_found)?;
                let definition = definition.map_err(map_flagship_error)?;
                if !definition.is_object() {
                    return Err(parse_evaluation_error());
                }
                let normalized = normalize_context(context).map_err(map_flagship_error)?;
                let result = evaluate_flag(definition, normalized.values(), account_id)
                    .map_err(|_| parse_evaluation_error())?;
                let response = FlagshipEvaluationResponse::new(flag_key, result.value)
                    .with_variant(result.variant)
                    .with_reason(result.reason);

                (response, None, generation)
            }
        };

        let value =
            convert(&response.value).map_err(|()| type_mismatch(expected_name, &response.value))?;
        let details = ResolutionDetails {
            value,
            variant: (!response.variant.is_empty()).then(|| response.variant.clone()),
            reason: Some(map_reason(&response.reason)),
            flag_metadata: None,
        };

        if response.reason != EvaluationReason::DISABLED
            && self.inner.is_current(generation)
            && let Some(token) = cache_token
            && let ProviderMode::Remote {
                cache: Some(cache), ..
            } = &self.inner.mode
        {
            let _ = cache.insert(token, response);
        }

        Ok(details)
    }
}

impl fmt::Debug for FlagshipServerProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (mode, refresh_interval, cache_enabled, cache_max_entries) = match &self.inner.mode {
            ProviderMode::Remote {
                cache,
                cache_max_entries,
                ..
            } => (
                "remote",
                None,
                cache.is_some(),
                cache.as_ref().map(|_| *cache_max_entries),
            ),
            ProviderMode::Local {
                refresh_interval, ..
            } => ("local", Some(*refresh_interval), false, None),
        };

        formatter
            .debug_struct("FlagshipServerProvider")
            .field("mode", &mode)
            .field("status", &self.status())
            .field("refresh_interval", &refresh_interval)
            .field("cache_enabled", &cache_enabled)
            .field("cache_max_entries", &cache_max_entries)
            .field("hook_count", &self.hooks.len())
            .finish_non_exhaustive()
    }
}

enum ProviderMode {
    Remote {
        client: FlagshipClient,
        cache: Option<Arc<ResponseCache>>,
        cache_max_entries: usize,
    },
    Local {
        source: Arc<dyn DefinitionsSource>,
        account_id: String,
        refresh_interval: Duration,
    },
}

#[async_trait]
trait DefinitionsSource: Send + Sync {
    async fn fetch(
        &self,
        prior_etag: Option<&HeaderValue>,
    ) -> Result<DefinitionsSourceResult, FlagshipError>;
}

enum DefinitionsSourceResult {
    Updated {
        snapshot: DefinitionsSnapshot,
        etag: Option<HeaderValue>,
    },
    NotModified,
}

#[derive(Clone)]
enum InitializationState {
    Pending,
    Complete(Result<(), FlagshipError>),
}

#[async_trait]
impl DefinitionsSource for DefinitionsFetcher {
    async fn fetch(
        &self,
        prior_etag: Option<&HeaderValue>,
    ) -> Result<DefinitionsSourceResult, FlagshipError> {
        match DefinitionsFetcher::fetch(self, prior_etag).await? {
            DefinitionsFetchResult::Updated(snapshot) => {
                let etag = snapshot.etag().cloned();

                Ok(DefinitionsSourceResult::Updated { snapshot, etag })
            }
            DefinitionsFetchResult::NotModified => Ok(DefinitionsSourceResult::NotModified),
        }
    }
}

struct ProviderInner {
    mode: ProviderMode,
    status: AtomicU8,
    generation: AtomicU64,
    lifecycle: Mutex<LifecycleState>,
}

#[derive(Default)]
struct LifecycleState {
    snapshot: Option<Arc<DefinitionsSnapshot>>,
    etag: Option<HeaderValue>,
    initialization: Option<watch::Receiver<InitializationState>>,
    task: Option<JoinHandle<()>>,
}

impl ProviderInner {
    fn set_status(&self, status: u8) {
        self.status.store(status, Ordering::Release);
    }

    fn provider_status(&self) -> ProviderStatus {
        match self.status.load(Ordering::Acquire) {
            STATUS_READY => ProviderStatus::Ready,
            STATUS_ERROR => ProviderStatus::Error,
            STATUS_STALE => ProviderStatus::STALE,
            _ => ProviderStatus::NotReady,
        }
    }

    fn remote_generation(&self) -> Result<u64, EvaluationError> {
        loop {
            let generation = self.generation.load(Ordering::Acquire);
            let status = self.status.load(Ordering::Acquire);
            if generation != self.generation.load(Ordering::Acquire) {
                continue;
            }
            if matches!(status, STATUS_NOT_READY | STATUS_ERROR) {
                return Err(provider_not_ready());
            }

            return Ok(generation);
        }
    }

    fn local_snapshot(&self) -> Result<(u64, Arc<DefinitionsSnapshot>), EvaluationError> {
        let state = self.lifecycle();
        if matches!(
            self.provider_status(),
            ProviderStatus::NotReady | ProviderStatus::Error
        ) {
            return Err(provider_not_ready());
        }

        let generation = self.generation.load(Ordering::Acquire);
        let snapshot = state.snapshot.clone().ok_or_else(provider_not_ready)?;

        Ok((generation, snapshot))
    }

    #[cfg(test)]
    fn has_active_local_session(&self) -> bool {
        if !matches!(&self.mode, ProviderMode::Local { .. }) {
            return false;
        }

        let state = self.lifecycle();

        self.status.load(Ordering::Acquire) != STATUS_ERROR
            && state.task.as_ref().is_some_and(|task| !task.is_finished())
    }

    fn begin_session(
        &self,
        initialization: Option<watch::Receiver<InitializationState>>,
    ) -> (u64, Option<JoinHandle<()>>) {
        let mut state = self.lifecycle();
        let generation = self
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        self.set_status(STATUS_NOT_READY);
        state.snapshot = None;
        state.etag = None;
        state.initialization = initialization;
        let task = state.task.take();

        (generation, task)
    }

    fn transition_remote(&self, status: u8) {
        self.set_status(STATUS_NOT_READY);
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.clear_remote_cache();
        self.set_status(status);
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::Acquire) == generation
    }

    fn lifecycle(&self) -> MutexGuard<'_, LifecycleState> {
        self.lifecycle
            .lock()
            .expect("provider lifecycle mutex poisoned")
    }

    fn install_task(&self, generation: u64, task: JoinHandle<()>) -> Result<(), JoinHandle<()>> {
        let mut state = self.lifecycle();
        if !self.is_current(generation) {
            task.abort();

            return Err(task);
        }
        if let Some(old) = state.task.replace(task) {
            old.abort();
        }

        Ok(())
    }

    fn initialization_receiver(&self) -> Option<(u64, watch::Receiver<InitializationState>)> {
        let state = self.lifecycle();

        state
            .initialization
            .clone()
            .map(|receiver| (self.generation.load(Ordering::Acquire), receiver))
    }

    fn clear_uninstalled_session(&self, generation: u64) {
        let Ok(mut state) = self.lifecycle.lock() else {
            return;
        };

        if self.is_current(generation) && state.task.is_none() {
            state.initialization = None;
        }
    }

    fn set_current_status(&self, generation: u64, status: u8) {
        let _state = self.lifecycle();
        if self.is_current(generation) {
            self.set_status(status);
        }
    }

    fn publish_snapshot(
        &self,
        generation: u64,
        snapshot: DefinitionsSnapshot,
        etag: Option<HeaderValue>,
    ) -> bool {
        let mut state = self.lifecycle();
        if !self.is_current(generation) {
            return false;
        }

        state.snapshot = Some(Arc::new(snapshot));
        state.etag = etag;
        self.set_status(STATUS_READY);

        true
    }

    fn clear_remote_cache(&self) {
        if let ProviderMode::Remote {
            cache: Some(cache), ..
        } = &self.mode
        {
            cache.clear();
        }
    }
}

impl Drop for ProviderInner {
    fn drop(&mut self) {
        // Do not recover poisoned state during drop. A panic here could mask the
        // original panic, so abandoning the inaccessible task handle is safer.
        let Ok(state) = self.lifecycle.get_mut() else {
            return;
        };

        if let Some(task) = state.task.take() {
            task.abort();
        }
    }
}

struct SessionInstallGuard {
    inner: Arc<ProviderInner>,
    generation: u64,
    armed: bool,
}

impl SessionInstallGuard {
    fn new(inner: Arc<ProviderInner>, generation: u64) -> Self {
        Self {
            inner,
            generation,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SessionInstallGuard {
    fn drop(&mut self) {
        if self.armed {
            self.inner.clear_uninstalled_session(self.generation);
        }
    }
}

async fn abort_task(task: Option<JoinHandle<()>>) {
    if let Some(task) = task {
        task.abort();
        let _ = task.await;
    }
}

async fn wait_for_initialization(
    mut receiver: watch::Receiver<InitializationState>,
) -> Result<Result<(), FlagshipError>, ()> {
    loop {
        let state = receiver.borrow().clone();

        match state {
            InitializationState::Pending => receiver.changed().await.map_err(|_| ())?,
            InitializationState::Complete(result) => return Ok(result),
        }
    }
}

fn finish_initialization(
    inner: &ProviderInner,
    generation: u64,
    result: Result<Result<(), FlagshipError>, ()>,
) -> Result<(), FlagshipError> {
    if !inner.is_current(generation) {
        return Err(superseded_error());
    }

    if let Ok(result) = result {
        return result;
    }

    inner.set_current_status(generation, STATUS_ERROR);

    Err(FlagshipError::new(
        ErrorCode::General,
        "provider initialization task ended unexpectedly",
    ))
}

struct AbortOnDrop {
    handle: Option<AbortHandle>,
}

impl AbortOnDrop {
    fn new(handle: AbortHandle) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    fn disarm(&mut self) {
        self.handle = None;
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

async fn fetch_definitions(
    source: Arc<dyn DefinitionsSource>,
    prior_etag: Option<HeaderValue>,
) -> Result<DefinitionsSourceResult, FlagshipError> {
    let task = tokio::spawn(async move { source.fetch(prior_etag.as_ref()).await });
    let mut abort_on_drop = AbortOnDrop::new(task.abort_handle());
    let result = task.await;
    abort_on_drop.disarm();

    result.unwrap_or_else(|_| {
        Err(FlagshipError::new(
            ErrorCode::General,
            "definitions fetch task ended unexpectedly",
        ))
    })
}

async fn run_local_session(
    inner: Weak<ProviderInner>,
    generation: u64,
    source: Arc<dyn DefinitionsSource>,
    mut interval: Interval,
    initialized: watch::Sender<InitializationState>,
) {
    let initial = fetch_definitions(Arc::clone(&source), None).await;
    let Some(provider) = inner.upgrade() else {
        return;
    };
    if !provider.is_current(generation) {
        return;
    }

    match initial {
        Ok(DefinitionsSourceResult::Updated { snapshot, etag }) => {
            if !provider.publish_snapshot(generation, snapshot, etag) {
                return;
            }
            initialized.send_replace(InitializationState::Complete(Ok(())));
        }
        Ok(DefinitionsSourceResult::NotModified) => {
            let error = FlagshipError::new(
                ErrorCode::ParseError,
                "initial definitions request unexpectedly returned not modified",
            );
            provider.set_current_status(generation, STATUS_ERROR);
            initialized.send_replace(InitializationState::Complete(Err(error)));

            return;
        }
        Err(error) => {
            provider.set_current_status(generation, STATUS_ERROR);
            initialized.send_replace(InitializationState::Complete(Err(error)));

            return;
        }
    }
    drop(provider);

    interval.reset();

    loop {
        interval.tick().await;
        let Some(provider) = inner.upgrade() else {
            return;
        };
        if !provider.is_current(generation) {
            return;
        }
        let prior_etag = provider.lifecycle().etag.clone();
        drop(provider);

        let result = fetch_definitions(Arc::clone(&source), prior_etag).await;
        let Some(provider) = inner.upgrade() else {
            return;
        };
        if !provider.is_current(generation) {
            return;
        }

        match result {
            Ok(DefinitionsSourceResult::Updated { snapshot, etag }) => {
                if !provider.publish_snapshot(generation, snapshot, etag) {
                    return;
                }
            }
            Ok(DefinitionsSourceResult::NotModified) => {
                provider.set_current_status(generation, STATUS_READY);
            }
            Err(_) => provider.set_current_status(generation, STATUS_STALE),
        }
    }
}

async fn evaluate_remote(
    client: &FlagshipClient,
    cache: Option<&ResponseCache>,
    flag_key: &str,
    context: &NormalizedContext,
    expected_type: ExpectedFlagType,
) -> Result<(FlagshipEvaluationResponse, Option<CacheToken>), EvaluationError> {
    let token = if let Some(cache) = cache {
        let lookup = cache
            .lookup(flag_key, expected_type, context)
            .map_err(map_flagship_error)?;
        if let Some(mut response) = lookup.response {
            response.reason = EvaluationReason::from("CACHED");

            return Ok((response, None));
        }

        Some(lookup.token)
    } else {
        None
    };

    let response = client
        .evaluate_normalized(flag_key, context)
        .await
        .map_err(map_flagship_error)?;

    Ok((response, token))
}

fn map_reason(reason: &EvaluationReason) -> OpenFeatureReason {
    match reason.as_str() {
        "STATIC" => OpenFeatureReason::Static,
        "TARGETING_MATCH" => OpenFeatureReason::TargetingMatch,
        "DEFAULT" => OpenFeatureReason::Default,
        "DISABLED" => OpenFeatureReason::Disabled,
        "SPLIT" => OpenFeatureReason::Split,
        "CACHED" => OpenFeatureReason::Cached,
        "ERROR" => OpenFeatureReason::Error,
        "UNKNOWN" => OpenFeatureReason::Unknown,
        other => OpenFeatureReason::Other(other.to_owned()),
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "owned signature composes directly with Result::map_err"
)]
fn map_flagship_error(error: FlagshipError) -> EvaluationError {
    // `server_code` is deliberately ignored for Go/Python parity: an unknown
    // backend code must not replace the SDK's established error category.
    let (code, category, summary) = match error.code() {
        ErrorCode::ProviderNotReady => (
            EvaluationErrorCode::ProviderNotReady,
            "PROVIDER_NOT_READY",
            "Flagship provider is not ready",
        ),
        ErrorCode::FlagNotFound => (
            EvaluationErrorCode::FlagNotFound,
            "FLAG_NOT_FOUND",
            "requested flag was not found",
        ),
        ErrorCode::ParseError => (
            EvaluationErrorCode::ParseError,
            "PARSE_ERROR",
            "Flagship data could not be parsed",
        ),
        ErrorCode::InvalidContext => (
            EvaluationErrorCode::InvalidContext,
            "INVALID_CONTEXT",
            "evaluation context is invalid",
        ),
        ErrorCode::BadRequest => (
            EvaluationErrorCode::General("GENERAL".to_owned()),
            "BAD_REQUEST",
            "Flagship evaluation failed",
        ),
        ErrorCode::NetworkError => (
            EvaluationErrorCode::General("GENERAL".to_owned()),
            "NETWORK_ERROR",
            "Flagship evaluation failed",
        ),
        ErrorCode::TimeoutError => (
            EvaluationErrorCode::General("GENERAL".to_owned()),
            "TIMEOUT_ERROR",
            "Flagship evaluation failed",
        ),
        ErrorCode::General | ErrorCode::Unknown(_) => (
            EvaluationErrorCode::General("GENERAL".to_owned()),
            "GENERAL",
            "Flagship evaluation failed",
        ),
    };
    let status = error
        .status()
        .map_or_else(String::new, |status| format!(" (HTTP {status})"));

    EvaluationError {
        code,
        message: Some(format!("{category}{status}: {summary}")),
    }
}

fn type_mismatch(expected: &str, actual: &JsonValue) -> EvaluationError {
    EvaluationError {
        code: EvaluationErrorCode::TypeMismatch,
        message: Some(format!(
            "TYPE_MISMATCH: expected {expected}; actual JSON kind is {}",
            json_kind(actual)
        )),
    }
}

fn json_kind(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

fn provider_not_ready() -> EvaluationError {
    map_flagship_error(FlagshipError::new(
        ErrorCode::ProviderNotReady,
        "provider is not ready",
    ))
}

fn flag_not_found() -> EvaluationError {
    map_flagship_error(FlagshipError::new(
        ErrorCode::FlagNotFound,
        "requested flag was not found",
    ))
}

fn parse_evaluation_error() -> EvaluationError {
    map_flagship_error(FlagshipError::new(
        ErrorCode::ParseError,
        "flag definition could not be evaluated",
    ))
}

fn superseded_error() -> FlagshipError {
    FlagshipError::new(
        ErrorCode::ProviderNotReady,
        "provider initialization was superseded",
    )
}

fn json_value(value: &JsonValue) -> Result<OpenFeatureValue, ()> {
    match value {
        JsonValue::Null => Err(()),
        JsonValue::Bool(value) => Ok(OpenFeatureValue::Bool(*value)),
        JsonValue::String(value) => Ok(OpenFeatureValue::String(value.clone())),
        JsonValue::Number(value) => {
            if let Some(value) = value.as_i64() {
                return Ok(OpenFeatureValue::Int(value));
            }

            value
                .as_f64()
                .filter(|value| value.is_finite())
                .map(OpenFeatureValue::Float)
                .ok_or(())
        }
        JsonValue::Array(values) => values
            .iter()
            .map(json_value)
            .collect::<Result<Vec<_>, _>>()
            .map(OpenFeatureValue::Array),
        JsonValue::Object(_) => json_struct(value).map(OpenFeatureValue::Struct),
    }
}

fn json_struct(value: &JsonValue) -> Result<StructValue, ()> {
    let object = value.as_object().ok_or(())?;
    let fields = object
        .iter()
        .map(|(key, value)| Ok((key.clone(), json_value(value)?)))
        .collect::<Result<_, ()>>()?;

    Ok(StructValue { fields })
}

#[async_trait]
impl FeatureProvider for FlagshipServerProvider {
    async fn initialize(&mut self, _context: &EvaluationContext) {
        let mut waited_for_initialization = false;

        loop {
            let status = self.status();
            if status == ProviderStatus::Ready {
                return;
            }

            if status == ProviderStatus::NotReady
                && let Some((generation, receiver)) = self.inner.initialization_receiver()
            {
                waited_for_initialization = true;
                let result = wait_for_initialization(receiver).await;
                if self.inner.is_current(generation) {
                    if result.is_err() && self.inner.initialization_receiver().is_some() {
                        self.inner.set_current_status(generation, STATUS_ERROR);
                    }

                    return;
                }

                if self.inner.initialization_receiver().is_none() {
                    return;
                }

                continue;
            }

            if waited_for_initialization {
                return;
            }

            break;
        }

        let _ = self.initialize_provider().await;
    }

    fn status(&self) -> ProviderStatus {
        FlagshipServerProvider::status(self)
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
        self.resolve_typed(
            flag_key,
            context,
            ExpectedFlagType::Bool,
            "boolean",
            |value| value.as_bool().ok_or(()),
        )
        .await
    }

    async fn resolve_int_value(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<i64>> {
        self.resolve_typed(
            flag_key,
            context,
            ExpectedFlagType::Int,
            "integer",
            |value| value.as_i64().ok_or(()),
        )
        .await
    }

    async fn resolve_float_value(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<f64>> {
        self.resolve_typed(
            flag_key,
            context,
            ExpectedFlagType::Float,
            "finite number",
            |value| value.as_f64().filter(|value| value.is_finite()).ok_or(()),
        )
        .await
    }

    async fn resolve_string_value(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<String>> {
        self.resolve_typed(
            flag_key,
            context,
            ExpectedFlagType::String,
            "string",
            |value| value.as_str().map(str::to_owned).ok_or(()),
        )
        .await
    }

    async fn resolve_struct_value(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult<ResolutionDetails<StructValue>> {
        self.resolve_typed(
            flag_key,
            context,
            ExpectedFlagType::Object,
            "OpenFeature-compatible object",
            json_struct,
        )
        .await
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc as std_mpsc;

    use open_feature::OpenFeature;
    use serde_json::json;
    use tokio::sync::{mpsc, oneshot};

    use super::*;

    struct TestRequest {
        etag: Option<String>,
        response: oneshot::Sender<TestResponse>,
    }

    enum TestResponse {
        Updated(&'static [u8], Option<&'static str>),
        NotModified,
        Error(ErrorCode, Option<u16>),
        Panic,
    }

    struct ControlledSource {
        requests: mpsc::UnboundedSender<TestRequest>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl DefinitionsSource for ControlledSource {
        async fn fetch(
            &self,
            prior_etag: Option<&HeaderValue>,
        ) -> Result<DefinitionsSourceResult, FlagshipError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (sender, receiver) = oneshot::channel();
            self.requests
                .send(TestRequest {
                    etag: prior_etag
                        .and_then(|etag| etag.to_str().ok())
                        .map(str::to_owned),
                    response: sender,
                })
                .map_err(|_| FlagshipError::new(ErrorCode::General, "test source closed"))?;

            match receiver.await {
                Ok(response) => test_result(response),
                Err(_) => Err(FlagshipError::new(
                    ErrorCode::General,
                    "controlled response dropped",
                )),
            }
        }
    }

    struct SequenceSource {
        responses: Mutex<VecDeque<TestResponse>>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl DefinitionsSource for SequenceSource {
        async fn fetch(
            &self,
            _prior_etag: Option<&HeaderValue>,
        ) -> Result<DefinitionsSourceResult, FlagshipError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let response = self
                .responses
                .lock()
                .expect("sequence source mutex poisoned")
                .pop_front()
                .ok_or_else(|| FlagshipError::new(ErrorCode::General, "no test response"))?;

            test_result(response)
        }
    }

    fn test_result(response: TestResponse) -> Result<DefinitionsSourceResult, FlagshipError> {
        match response {
            TestResponse::Updated(body, etag) => {
                let snapshot = DefinitionsSnapshot::from_json_bytes(body)?;
                let etag = etag.map(HeaderValue::from_static);

                Ok(DefinitionsSourceResult::Updated { snapshot, etag })
            }
            TestResponse::NotModified => Ok(DefinitionsSourceResult::NotModified),
            TestResponse::Error(code, status) => {
                let mut error = FlagshipError::new(code, "controlled source failure");
                if let Some(status) = status {
                    error = error.with_status(status);
                }

                Err(error)
            }
            TestResponse::Panic => panic!("controlled source panic"),
        }
    }

    struct ControlledProvider {
        provider: FlagshipServerProvider,
        requests: mpsc::UnboundedReceiver<TestRequest>,
    }

    fn controlled_provider(refresh_interval: Duration) -> ControlledProvider {
        let (requests_sender, requests) = mpsc::unbounded_channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let source = ControlledSource {
            requests: requests_sender,
            calls: Arc::clone(&calls),
        };
        let inner = ProviderInner {
            mode: ProviderMode::Local {
                source: Arc::new(source),
                account_id: "account".to_owned(),
                refresh_interval,
            },
            status: AtomicU8::new(STATUS_NOT_READY),
            generation: AtomicU64::new(0),
            lifecycle: Mutex::new(LifecycleState::default()),
        };
        let provider = FlagshipServerProvider {
            inner: Arc::new(inner),
            metadata: ProviderMetadata::new(PROVIDER_NAME),
            hooks: Vec::new(),
        };

        ControlledProvider { provider, requests }
    }

    fn definitions(value: bool) -> &'static [u8] {
        if value {
            br#"{"flags":{"flag":{"key":"flag","enabled":true,"default_variation":"on","variations":{"on":true},"rules":[]}}}"#
        } else {
            br#"{"flags":{"flag":{"key":"flag","enabled":true,"default_variation":"off","variations":{"off":false},"rules":[]}}}"#
        }
    }

    fn start_initialize(
        provider: &FlagshipServerProvider,
    ) -> JoinHandle<Result<(), FlagshipError>> {
        let provider = provider.clone();

        tokio::spawn(async move { provider.initialize_provider().await })
    }

    async fn respond(request: TestRequest, response: TestResponse) {
        request.response.send(response).ok();
        tokio::task::yield_now().await;
    }

    async fn bool_value(provider: &FlagshipServerProvider) -> bool {
        provider
            .resolve_bool_value("flag", &EvaluationContext::default())
            .await
            .expect("bool resolution")
            .value
    }

    #[tokio::test]
    async fn completed_initialization_result_is_rejected_after_shutdown() {
        let controlled = controlled_provider(Duration::from_secs(10));
        let (sender, receiver) = watch::channel(InitializationState::Pending);
        let (generation, task) = controlled.provider.inner.begin_session(Some(receiver));
        assert!(task.is_none());
        sender.send_replace(InitializationState::Complete(Ok(())));

        controlled.provider.shutdown_provider().await;

        let error = finish_initialization(&controlled.provider.inner, generation, Ok(Ok(())))
            .expect_err("shutdown must supersede completed initialization");
        assert_eq!(error.code(), &ErrorCode::ProviderNotReady);
        assert_eq!(controlled.provider.status(), ProviderStatus::NotReady);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_before_task_install_leaves_a_recoverable_provider() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let (started_sender, started_receiver) = oneshot::channel();
        let (done_sender, done_receiver) = oneshot::channel();
        let (release_sender, release_receiver) = std_mpsc::channel();
        let old_task = tokio::task::spawn_blocking(move || {
            started_sender.send(()).ok();
            release_receiver.recv().ok();
            done_sender.send(()).ok();
        });
        controlled.provider.inner.lifecycle().task = Some(old_task);
        started_receiver.await.unwrap();

        let initialization = start_initialize(&controlled.provider);
        for _ in 0..10 {
            if controlled.provider.inner.generation.load(Ordering::Acquire) != 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            controlled.provider.inner.generation.load(Ordering::Acquire),
            1
        );
        assert!(
            controlled
                .provider
                .inner
                .initialization_receiver()
                .is_some()
        );
        assert!(controlled.provider.inner.lifecycle().task.is_none());

        let mut waiting_provider = controlled.provider.clone();
        let waiting_trait_initialization = tokio::spawn(async move {
            FeatureProvider::initialize(&mut waiting_provider, &EvaluationContext::default()).await;
        });
        tokio::task::yield_now().await;
        assert!(!waiting_trait_initialization.is_finished());

        initialization.abort();
        let _ = initialization.await;
        waiting_trait_initialization.await.unwrap();
        release_sender.send(()).unwrap();
        done_receiver.await.unwrap();

        assert_eq!(controlled.provider.status(), ProviderStatus::NotReady);
        assert!(
            controlled
                .provider
                .inner
                .initialization_receiver()
                .is_none()
        );
        assert!(controlled.provider.inner.lifecycle().task.is_none());

        let mut trait_provider = controlled.provider.clone();
        let trait_initialization = tokio::spawn(async move {
            FeatureProvider::initialize(&mut trait_provider, &EvaluationContext::default()).await;
        });
        let request = controlled.requests.recv().await.unwrap();
        respond(request, TestResponse::Updated(definitions(true), None)).await;
        trait_initialization.await.unwrap();
        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);

        controlled.provider.shutdown_provider().await;
        let recovery = start_initialize(&controlled.provider);
        let request = controlled.requests.recv().await.unwrap();
        respond(request, TestResponse::Updated(definitions(false), None)).await;
        recovery.await.unwrap().unwrap();
        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);
        assert!(!bool_value(&controlled.provider).await);
    }

    #[tokio::test(start_paused = true)]
    async fn etag_failures_panic_and_recovery_are_deterministic() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let initialize = start_initialize(&controlled.provider);
        let initial = controlled.requests.recv().await.expect("initial request");
        assert_eq!(initial.etag, None);
        respond(
            initial,
            TestResponse::Updated(definitions(true), Some("v1")),
        )
        .await;
        initialize.await.unwrap().unwrap();
        assert!(bool_value(&controlled.provider).await);

        tokio::time::advance(Duration::from_secs(10)).await;
        let unchanged = controlled
            .requests
            .recv()
            .await
            .expect("conditional refresh");
        assert_eq!(unchanged.etag.as_deref(), Some("v1"));
        respond(unchanged, TestResponse::NotModified).await;
        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);
        assert!(bool_value(&controlled.provider).await);

        tokio::time::advance(Duration::from_secs(10)).await;
        let failed = controlled.requests.recv().await.expect("failed refresh");
        respond(failed, TestResponse::Error(ErrorCode::NetworkError, None)).await;
        assert_eq!(controlled.provider.status(), ProviderStatus::STALE);
        assert!(bool_value(&controlled.provider).await);

        tokio::time::advance(Duration::from_secs(10)).await;
        let recovered = controlled.requests.recv().await.expect("304 recovery");
        assert_eq!(recovered.etag.as_deref(), Some("v1"));
        respond(recovered, TestResponse::NotModified).await;
        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);
        assert!(bool_value(&controlled.provider).await);

        tokio::time::advance(Duration::from_secs(10)).await;
        let panicked = controlled.requests.recv().await.expect("panicked refresh");
        respond(panicked, TestResponse::Panic).await;
        assert_eq!(controlled.provider.status(), ProviderStatus::STALE);
        assert!(bool_value(&controlled.provider).await);

        tokio::time::advance(Duration::from_secs(10)).await;
        let updated = controlled.requests.recv().await.expect("updated recovery");
        respond(updated, TestResponse::Updated(definitions(false), None)).await;
        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);
        assert!(!bool_value(&controlled.provider).await);

        tokio::time::advance(Duration::from_secs(10)).await;
        let no_etag = controlled
            .requests
            .recv()
            .await
            .expect("unconditional refresh");
        assert_eq!(no_etag.etag, None);
        respond(no_etag, TestResponse::NotModified).await;
        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_initialization_recovers_and_dead_current_task_becomes_error() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let first = start_initialize(&controlled.provider);
        let request = controlled.requests.recv().await.unwrap();
        respond(
            request,
            TestResponse::Error(ErrorCode::NetworkError, Some(503)),
        )
        .await;
        let error = first.await.unwrap().unwrap_err();
        assert_eq!(error.code(), &ErrorCode::NetworkError);
        assert_eq!(controlled.provider.status(), ProviderStatus::Error);

        let recovery = start_initialize(&controlled.provider);
        let request = controlled.requests.recv().await.unwrap();
        respond(request, TestResponse::Updated(definitions(true), None)).await;
        recovery.await.unwrap().unwrap();
        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);

        let panic_initialization = start_initialize(&controlled.provider);
        let request = controlled.requests.recv().await.unwrap();
        respond(request, TestResponse::Panic).await;
        let error = panic_initialization.await.unwrap().unwrap_err();
        assert_eq!(error.code(), &ErrorCode::General);
        assert_eq!(controlled.provider.status(), ProviderStatus::Error);
    }

    #[tokio::test]
    async fn error_status_with_unfinished_task_is_not_an_active_session() {
        let controlled = controlled_provider(Duration::from_secs(10));
        let unfinished = tokio::spawn(std::future::pending());
        controlled.provider.inner.lifecycle().task = Some(unfinished);
        controlled.provider.inner.set_status(STATUS_READY);
        assert!(controlled.provider.inner.has_active_local_session());

        controlled.provider.inner.set_status(STATUS_ERROR);

        assert!(!controlled.provider.inner.has_active_local_session());
        let task = controlled.provider.inner.lifecycle().task.take();
        abort_task(task).await;
    }

    #[test]
    fn poisoned_lifecycle_fails_fast_and_drop_does_not_panic() {
        let controlled = controlled_provider(Duration::from_secs(10));
        let poison_target = Arc::clone(&controlled.provider.inner);
        std::thread::spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _state = poison_target.lifecycle.lock().expect("poison test lock");
                panic!("poison lifecycle mutex");
            }));
        })
        .join()
        .expect("poisoning thread");

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = controlled.provider.inner.lifecycle();
        }));
        assert!(result.is_err());

        let guard = SessionInstallGuard::new(Arc::clone(&controlled.provider.inner), 1);
        drop(guard);
        drop(controlled.provider);
    }

    #[tokio::test(start_paused = true)]
    async fn registration_immediately_retries_failed_explicit_initialization() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let initialization = start_initialize(&controlled.provider);
        let failed_request = controlled.requests.recv().await.unwrap();
        respond(
            failed_request,
            TestResponse::Error(ErrorCode::NetworkError, None),
        )
        .await;
        initialization.await.unwrap().unwrap_err();
        assert_eq!(controlled.provider.status(), ProviderStatus::Error);

        let provider = controlled.provider.clone();
        let registration = tokio::spawn(async move {
            let mut api = OpenFeature::default();
            api.set_named_provider("retry", provider).await;
            api
        });
        let retry = controlled
            .requests
            .recv()
            .await
            .expect("registration retry");
        respond(retry, TestResponse::Updated(definitions(true), None)).await;
        let api = registration.await.unwrap();

        assert!(api.named_provider_metadata("retry").await.is_some());
        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);
        assert!(bool_value(&controlled.provider).await);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_and_newer_initializations_supersede_blocked_work() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let blocked = start_initialize(&controlled.provider);
        let stale_request = controlled.requests.recv().await.unwrap();

        controlled.provider.shutdown_provider().await;
        let error = blocked.await.unwrap().unwrap_err();
        assert_eq!(error.code(), &ErrorCode::ProviderNotReady);
        assert_eq!(controlled.provider.status(), ProviderStatus::NotReady);
        assert!(
            stale_request
                .response
                .send(TestResponse::Updated(definitions(true), Some("old")))
                .is_err()
        );

        let older = start_initialize(&controlled.provider);
        let older_request = controlled.requests.recv().await.unwrap();
        let newer = start_initialize(&controlled.provider);
        let newer_request = controlled.requests.recv().await.unwrap();
        assert_eq!(
            older.await.unwrap().unwrap_err().code(),
            &ErrorCode::ProviderNotReady
        );
        assert!(
            older_request
                .response
                .send(TestResponse::Updated(definitions(false), Some("old")))
                .is_err()
        );
        respond(
            newer_request,
            TestResponse::Updated(definitions(true), Some("new")),
        )
        .await;
        newer.await.unwrap().unwrap();
        assert!(bool_value(&controlled.provider).await);
    }

    #[tokio::test(start_paused = true)]
    async fn old_refresh_cannot_overwrite_shutdown_and_reinitialize() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let initialize = start_initialize(&controlled.provider);
        let request = controlled.requests.recv().await.unwrap();
        respond(
            request,
            TestResponse::Updated(definitions(true), Some("v1")),
        )
        .await;
        initialize.await.unwrap().unwrap();

        tokio::time::advance(Duration::from_secs(10)).await;
        let stale_refresh = controlled.requests.recv().await.unwrap();
        controlled.provider.shutdown_provider().await;
        let reinitialize = start_initialize(&controlled.provider);
        let new_request = controlled.requests.recv().await.unwrap();
        assert!(
            stale_refresh
                .response
                .send(TestResponse::Updated(definitions(false), Some("stale")))
                .is_err()
        );
        respond(
            new_request,
            TestResponse::Updated(definitions(true), Some("current")),
        )
        .await;
        reinitialize.await.unwrap().unwrap();
        assert!(bool_value(&controlled.provider).await);
    }

    #[tokio::test(start_paused = true)]
    async fn registration_after_200_does_not_issue_the_configured_second_500_fetch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let source = SequenceSource {
            responses: Mutex::new(VecDeque::from([
                TestResponse::Updated(definitions(true), Some("v1")),
                TestResponse::Error(ErrorCode::General, Some(500)),
            ])),
            calls: Arc::clone(&calls),
        };
        let provider = FlagshipServerProvider {
            inner: Arc::new(ProviderInner {
                mode: ProviderMode::Local {
                    source: Arc::new(source),
                    account_id: "account".to_owned(),
                    refresh_interval: Duration::from_secs(10),
                },
                status: AtomicU8::new(STATUS_NOT_READY),
                generation: AtomicU64::new(0),
                lifecycle: Mutex::new(LifecycleState::default()),
            }),
            metadata: ProviderMetadata::new(PROVIDER_NAME),
            hooks: Vec::new(),
        };
        provider.initialize_provider().await.unwrap();

        let mut api = OpenFeature::default();
        api.set_named_provider("preinitialized", provider.clone())
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.status(), ProviderStatus::Ready);
        assert!(bool_value(&provider).await);
    }

    #[tokio::test(start_paused = true)]
    async fn trait_initialization_waits_for_explicit_initialization_in_progress() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let initialize = start_initialize(&controlled.provider);
        let request = controlled.requests.recv().await.unwrap();
        let mut registration_clone = controlled.provider.clone();

        let registration = tokio::spawn(async move {
            FeatureProvider::initialize(&mut registration_clone, &EvaluationContext::default())
                .await;
        });

        tokio::task::yield_now().await;
        assert!(!registration.is_finished());
        assert!(controlled.requests.try_recv().is_err());
        respond(request, TestResponse::Updated(definitions(true), None)).await;
        initialize.await.unwrap().unwrap();
        registration.await.unwrap();
        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);
    }

    #[tokio::test(start_paused = true)]
    async fn trait_initialization_waiting_for_shutdown_does_not_restart() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let initialize = start_initialize(&controlled.provider);
        let request = controlled.requests.recv().await.unwrap();
        let mut registration_clone = controlled.provider.clone();
        let registration = tokio::spawn(async move {
            FeatureProvider::initialize(&mut registration_clone, &EvaluationContext::default())
                .await;
        });

        tokio::task::yield_now().await;
        assert!(!registration.is_finished());
        assert!(controlled.requests.try_recv().is_err());

        controlled.provider.shutdown_provider().await;
        let initialization_error = initialize.await.unwrap().unwrap_err();
        registration.await.unwrap();

        assert_eq!(initialization_error.code(), &ErrorCode::ProviderNotReady);
        assert_eq!(controlled.provider.status(), ProviderStatus::NotReady);
        assert!(controlled.requests.try_recv().is_err());
        assert!(request.response.is_closed());
    }

    #[tokio::test(start_paused = true)]
    async fn trait_initialization_follows_a_replacement_session() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let first = start_initialize(&controlled.provider);
        let first_request = controlled.requests.recv().await.unwrap();
        let mut registration_clone = controlled.provider.clone();
        let registration = tokio::spawn(async move {
            FeatureProvider::initialize(&mut registration_clone, &EvaluationContext::default())
                .await;
        });

        tokio::task::yield_now().await;
        assert!(!registration.is_finished());

        let replacement = start_initialize(&controlled.provider);
        let replacement_request = controlled.requests.recv().await.unwrap();
        assert!(first_request.response.is_closed());
        assert!(!registration.is_finished());

        respond(
            replacement_request,
            TestResponse::Updated(definitions(true), None),
        )
        .await;
        replacement.await.unwrap().unwrap();
        registration.await.unwrap();
        assert_eq!(
            first.await.unwrap().unwrap_err().code(),
            &ErrorCode::ProviderNotReady
        );
        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_explicit_initialize_future_does_not_cancel_spawned_session() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let initialize = start_initialize(&controlled.provider);
        let request = controlled.requests.recv().await.unwrap();

        initialize.abort();
        respond(request, TestResponse::Updated(definitions(true), None)).await;

        assert_eq!(controlled.provider.status(), ProviderStatus::Ready);
        assert!(bool_value(&controlled.provider).await);
    }

    #[tokio::test]
    async fn local_lookup_errors_precede_context_normalization() {
        #[derive(Debug)]
        struct UnsupportedContext;

        let controlled = controlled_provider(Duration::from_secs(10));
        let snapshot = DefinitionsSnapshot::from_json_bytes(
            br#"{"flags":{"malformed":7,"good":{"key":"good","enabled":true,"default_variation":"on","variations":{"on":true},"rules":[]}}}"#,
        )
        .unwrap();
        assert!(
            controlled
                .provider
                .inner
                .publish_snapshot(0, snapshot, None)
        );
        let context = EvaluationContext::default().with_custom_field(
            "private-context",
            open_feature::EvaluationContextFieldValue::new_struct(UnsupportedContext),
        );

        let missing = controlled
            .provider
            .resolve_bool_value("missing", &context)
            .await
            .unwrap_err();
        let malformed = controlled
            .provider
            .resolve_bool_value("malformed", &context)
            .await
            .unwrap_err();
        let invalid_context = controlled
            .provider
            .resolve_bool_value("good", &context)
            .await
            .unwrap_err();

        assert_eq!(missing.code, EvaluationErrorCode::FlagNotFound);
        assert_eq!(malformed.code, EvaluationErrorCode::ParseError);
        assert_eq!(invalid_context.code, EvaluationErrorCode::InvalidContext);
    }

    #[tokio::test(start_paused = true)]
    async fn last_clone_drop_stops_the_refresh_session() {
        let mut controlled = controlled_provider(Duration::from_secs(10));
        let initialize = start_initialize(&controlled.provider);
        let request = controlled.requests.recv().await.unwrap();
        respond(request, TestResponse::Updated(definitions(true), None)).await;
        initialize.await.unwrap().unwrap();
        let weak = Arc::downgrade(&controlled.provider.inner);

        drop(controlled.provider);
        tokio::task::yield_now().await;

        assert!(weak.upgrade().is_none());
        assert!(controlled.requests.is_closed());
        assert!(matches!(
            controlled.requests.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn pure_reason_conversion_and_safe_error_mapping_need_no_tls_client() {
        assert_eq!(
            map_reason(&EvaluationReason::from("ERROR")),
            OpenFeatureReason::Error
        );
        assert_eq!(
            map_reason(&EvaluationReason::from("CACHED")),
            OpenFeatureReason::Cached
        );
        assert_eq!(
            map_reason(&EvaluationReason::from("FUTURE")),
            OpenFeatureReason::Other("FUTURE".to_owned())
        );

        let mapped = map_flagship_error(
            FlagshipError::new(ErrorCode::BadRequest, "secret body")
                .with_status(400)
                .with_server_code("FLAG_NOT_FOUND"),
        );
        assert_eq!(
            mapped.code,
            EvaluationErrorCode::General("GENERAL".to_owned())
        );
        assert_eq!(
            mapped.message.as_deref(),
            Some("BAD_REQUEST (HTTP 400): Flagship evaluation failed")
        );
    }

    #[test]
    fn pure_json_conversion_reports_kinds_without_values() {
        assert_eq!(json_kind(&json!(null)), "null");
        assert!(json_struct(&json!({"nested": [true, 1, "x"]})).is_ok());
        assert!(json_struct(&json!({"private": null})).is_err());
        let error = type_mismatch("integer", &json!(1.5));
        assert_eq!(
            error.message.as_deref(),
            Some("TYPE_MISMATCH: expected integer; actual JSON kind is number")
        );
    }

    #[cfg(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    ))]
    #[test]
    fn providerless_rustls_compiles_with_an_injected_client() {
        let _build_provider = |http_client: reqwest::Client| {
            let options = FlagshipClientOptions::builder()
                .endpoint("http://127.0.0.1:1/evaluate")
                .http_client(http_client)
                .build()?;

            FlagshipServerProvider::new(options)
        };
    }
}

#[cfg(all(
    test,
    not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    ))
))]
mod tests {
    use std::future::Future;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};
    use std::thread::JoinHandle;

    use open_feature::provider::FeatureProvider;
    use serde_json::json;
    use tokio::sync::oneshot;

    use super::*;

    fn options(account_id: bool) -> FlagshipClientOptions {
        let mut builder = FlagshipClientOptions::builder()
            .endpoint("http://127.0.0.1:1/evaluate")
            .retries(0)
            .disable_timeout();
        if account_id {
            builder = builder.account_id("private-account");
        }

        builder.build().expect("test options")
    }

    fn local_provider() -> FlagshipServerProvider {
        FlagshipServerProvider::builder(options(true))
            .local_evaluation(true)
            .build()
            .expect("local provider")
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "test helper consumes inline JSON"
    )]
    fn install(provider: &FlagshipServerProvider, body: JsonValue) {
        let snapshot = DefinitionsSnapshot::from_json_bytes(
            &serde_json::to_vec(&body).expect("serialize definitions"),
        )
        .expect("definitions snapshot");

        assert!(provider.inner.publish_snapshot(0, snapshot, None));
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "test helper consumes inline JSON"
    )]
    fn flag(value: JsonValue, reason_enabled: bool) -> JsonValue {
        json!({
            "key": "flag",
            "enabled": reason_enabled,
            "default_variation": "value",
            "variations": {"value": value},
            "rules": []
        })
    }

    fn assert_send_sync<T: Send + Sync>() {}

    fn assert_send_future<T: Future + Send>(_: T) {}

    struct TestHook;

    #[async_trait]
    impl Hook for TestHook {
        async fn before<'a>(
            &self,
            _context: &open_feature::HookContext<'a>,
            _hints: Option<&'a open_feature::HookHints>,
        ) -> Result<Option<EvaluationContext>, EvaluationError> {
            Ok(None)
        }

        async fn after<'a>(
            &self,
            _context: &open_feature::HookContext<'a>,
            _details: &open_feature::EvaluationDetails<OpenFeatureValue>,
            _hints: Option<&'a open_feature::HookHints>,
        ) -> Result<(), EvaluationError> {
            Ok(())
        }

        async fn error<'a>(
            &self,
            _context: &open_feature::HookContext<'a>,
            _error: &EvaluationError,
            _hints: Option<&'a open_feature::HookHints>,
        ) {
        }

        async fn finally<'a>(
            &self,
            _context: &open_feature::HookContext<'a>,
            _details: &open_feature::EvaluationDetails<OpenFeatureValue>,
            _hints: Option<&'a open_feature::HookHints>,
        ) {
        }
    }

    fn server(body: &'static [u8]) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).expect("read request");
                assert_ne!(count, 0, "request ended before headers");
                request.extend_from_slice(&buffer[..count]);
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("response headers");
            stream.write_all(body).expect("response body");
        });

        (format!("http://{address}/evaluate"), thread)
    }

    fn scripted_server(
        responses: Vec<(u16, &'static [u8])>,
    ) -> (String, Receiver<Vec<u8>>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let (requests_sender, requests) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).expect("read request");
                    assert_ne!(count, 0, "request ended before headers");
                    request.extend_from_slice(&buffer[..count]);
                }
                requests_sender.send(request).expect("record request");
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .expect("response headers");
                stream.write_all(body).expect("response body");
            }
        });

        (format!("http://{address}/evaluate"), requests, thread)
    }

    fn blocking_server(
        body: &'static [u8],
    ) -> (
        String,
        oneshot::Receiver<()>,
        mpsc::Sender<()>,
        JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let (accepted_sender, accepted) = oneshot::channel();
        let (release, release_receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).expect("read request");
                assert_ne!(count, 0, "request ended before headers");
                request.extend_from_slice(&buffer[..count]);
            }
            accepted_sender.send(()).ok();
            release_receiver.recv().expect("release response");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("response headers");
            stream.write_all(body).expect("response body");
        });

        (
            format!("http://{address}/evaluate"),
            accepted,
            release,
            thread,
        )
    }

    fn endpoint_options(endpoint: &str, account_id: bool) -> FlagshipClientOptions {
        let mut builder = FlagshipClientOptions::builder()
            .endpoint(endpoint)
            .retries(0)
            .disable_timeout();
        if account_id {
            builder = builder.account_id("account");
        }

        builder.build().expect("endpoint options")
    }

    #[test]
    fn provider_implements_required_thread_safe_trait() {
        assert_send_sync::<FlagshipServerProvider>();
        let provider = FlagshipServerProvider::new(options(false)).expect("remote provider");

        assert_send_future(provider.resolve_bool_value("flag", &EvaluationContext::default()));
    }

    #[tokio::test]
    async fn hook_and_metadata_are_exposed_during_open_feature_registration() {
        let provider = FlagshipServerProvider::builder(options(false))
            .hook(TestHook)
            .build()
            .unwrap();
        assert_eq!(provider.hooks().len(), 1);
        let mut api = open_feature::OpenFeature::default();

        api.set_named_provider("metadata", provider).await;

        assert_eq!(
            api.named_provider_metadata("metadata").await.unwrap().name,
            PROVIDER_NAME
        );
    }

    #[test]
    fn construction_validates_modes_and_limits() {
        let error = FlagshipServerProvider::builder(options(false))
            .local_evaluation(true)
            .build()
            .expect_err("local mode requires an account");
        assert_eq!(
            error.message(),
            "account_id is required for local evaluation"
        );

        let error = FlagshipServerProvider::builder(options(true))
            .local_evaluation(true)
            .cache_ttl(Duration::from_secs(1))
            .build()
            .expect_err("local cache conflict");
        assert_eq!(
            error.message(),
            "response caching cannot be used with local evaluation"
        );

        FlagshipServerProvider::builder(options(false))
            .refresh_interval(Duration::ZERO)
            .build()
            .expect("remote refresh interval is ignored");
        let error = FlagshipServerProvider::builder(options(true))
            .local_evaluation(true)
            .refresh_interval(Duration::ZERO)
            .build()
            .expect_err("zero local refresh interval");
        assert_eq!(
            error.message(),
            "refresh_interval must be greater than zero"
        );

        let error = FlagshipServerProvider::builder(options(false))
            .cache_ttl(Duration::from_secs(1))
            .cache_max_entries(0)
            .build()
            .expect_err("zero enabled cache capacity");
        assert_eq!(
            error.message(),
            "cache_max_entries must be greater than zero when caching is enabled"
        );
    }

    #[tokio::test]
    async fn remote_lifecycle_is_ready_noop_and_shutdown_is_not_ready() {
        let provider = FlagshipServerProvider::new(options(false)).expect("remote provider");
        assert_eq!(provider.status(), ProviderStatus::Ready);

        provider
            .initialize_provider()
            .await
            .expect("remote initialization");
        assert_eq!(provider.status(), ProviderStatus::Ready);

        provider.shutdown_provider().await;
        assert_eq!(provider.status(), ProviderStatus::NotReady);
        let error = provider
            .resolve_bool_value("flag", &EvaluationContext::default())
            .await
            .expect_err("shutdown provider rejects evaluations");
        assert_eq!(error.code, EvaluationErrorCode::ProviderNotReady);

        provider
            .initialize_provider()
            .await
            .expect("remote reinitialization");
        assert_eq!(provider.status(), ProviderStatus::Ready);
    }

    #[tokio::test]
    async fn remote_cache_hits_preserve_value_variant_and_use_cached_reason() {
        let (endpoint, server) =
            server(br#"{"flagKey":"flag","value":true,"variant":"on","reason":"STATIC"}"#);
        let provider = FlagshipServerProvider::builder(endpoint_options(&endpoint, false))
            .cache_ttl(Duration::from_secs(60))
            .build()
            .expect("cached provider");
        let context = EvaluationContext::default();

        let first = provider.resolve_bool_value("flag", &context).await.unwrap();
        server.join().expect("server thread");
        let generation = provider.inner.generation.load(Ordering::Acquire);

        provider.initialize_provider().await.unwrap();
        provider.initialize_provider().await.unwrap();
        let mut api = open_feature::OpenFeature::default();
        api.set_named_provider("first", provider.clone()).await;
        api.set_named_provider("second", provider.clone()).await;

        let second = provider.resolve_bool_value("flag", &context).await.unwrap();

        assert_eq!(provider.status(), ProviderStatus::Ready);
        assert_eq!(
            provider.inner.generation.load(Ordering::Acquire),
            generation
        );
        assert!(first.value);
        assert_eq!(first.variant.as_deref(), Some("on"));
        assert_eq!(first.reason, Some(OpenFeatureReason::Static));
        assert_eq!(second.value, first.value);
        assert_eq!(second.variant, first.variant);
        assert_eq!(second.reason, Some(OpenFeatureReason::Cached));
    }

    #[tokio::test]
    async fn remote_errors_map_404_and_malformed_json_end_to_end() {
        let (endpoint, requests, server) = scripted_server(vec![
            (
                404,
                br#"{"errorCode":"PRIVATE_BACKEND_CODE","message":"private body"}"#,
            ),
            (200, br#"{"flagKey":"broken""#),
        ]);
        let provider = FlagshipServerProvider::new(endpoint_options(&endpoint, false)).unwrap();
        let context = EvaluationContext::default();

        let missing = provider
            .resolve_bool_value("missing", &context)
            .await
            .unwrap_err();
        let malformed = provider
            .resolve_bool_value("broken", &context)
            .await
            .unwrap_err();

        assert_eq!(missing.code, EvaluationErrorCode::FlagNotFound);
        assert_eq!(
            missing.message.as_deref(),
            Some("FLAG_NOT_FOUND (HTTP 404): requested flag was not found")
        );
        assert_eq!(malformed.code, EvaluationErrorCode::ParseError);
        assert_eq!(
            malformed.message.as_deref(),
            Some("PARSE_ERROR (HTTP 200): Flagship data could not be parsed")
        );
        for _ in 0..2 {
            requests.recv().expect("network request");
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn remote_open_feature_client_resolves_all_types_and_enforces_exact_types() {
        let (endpoint, requests, server) = scripted_server(vec![
            (
                200,
                br#"{"flagKey":"bool","value":true,"variant":"on","reason":"STATIC"}"#,
            ),
            (200, br#"{"flagKey":"string","value":"text"}"#),
            (200, br#"{"flagKey":"int","value":-7}"#),
            (200, br#"{"flagKey":"float","value":2.5}"#),
            (200, br#"{"flagKey":"whole-float","value":3}"#),
            (
                200,
                br#"{"flagKey":"object","value":{"nested":[true,3,"x"]}}"#,
            ),
            (
                200,
                br#"{"flagKey":"disabled","value":false,"reason":"DISABLED"}"#,
            ),
            (200, br#"{"flagKey":"wrong-bool","value":"true"}"#),
            (200, br#"{"flagKey":"wrong-int","value":1.5}"#),
            (200, br#"{"flagKey":"wrong-string","value":true}"#),
            (200, br#"{"flagKey":"wrong-object","value":[1,2]}"#),
        ]);
        let provider = FlagshipServerProvider::new(endpoint_options(&endpoint, false)).unwrap();
        let mut api = open_feature::OpenFeature::default();
        api.set_provider(provider).await;
        let client = api.create_named_client("typed-parity-test");
        let context = EvaluationContext::default();

        let boolean = client
            .get_bool_details("bool", Some(&context), None)
            .await
            .unwrap();
        let string = client
            .get_string_value("string", Some(&context), None)
            .await
            .unwrap();
        let integer = client
            .get_int_value("int", Some(&context), None)
            .await
            .unwrap();
        let float = client
            .get_float_value("float", Some(&context), None)
            .await
            .unwrap();
        let whole_float = client
            .get_float_value("whole-float", Some(&context), None)
            .await
            .unwrap();
        let object = client
            .get_struct_value::<StructValue>("object", Some(&context), None)
            .await
            .unwrap();
        let disabled = client
            .get_bool_details("disabled", Some(&context), None)
            .await
            .unwrap();
        let mismatches = [
            client
                .get_bool_value("wrong-bool", Some(&context), None)
                .await
                .unwrap_err(),
            client
                .get_int_value("wrong-int", Some(&context), None)
                .await
                .unwrap_err(),
            client
                .get_string_value("wrong-string", Some(&context), None)
                .await
                .unwrap_err(),
            client
                .get_struct_value::<StructValue>("wrong-object", Some(&context), None)
                .await
                .unwrap_err(),
        ];

        assert!(boolean.value);
        assert_eq!(boolean.variant.as_deref(), Some("on"));
        assert_eq!(boolean.reason, Some(OpenFeatureReason::Static));
        assert_eq!(string, "text");
        assert_eq!(integer, -7);
        assert_eq!(float, 2.5);
        assert_eq!(whole_float, 3.0);
        assert!(matches!(
            object.fields["nested"],
            OpenFeatureValue::Array(_)
        ));
        assert!(!disabled.value);
        assert_eq!(disabled.reason, Some(OpenFeatureReason::Disabled));
        assert!(
            mismatches
                .iter()
                .all(|error| error.code == EvaluationErrorCode::TypeMismatch)
        );

        for _ in 0..11 {
            requests.recv().expect("typed resolution request");
        }
        server.join().expect("test server thread");
    }

    #[tokio::test]
    async fn invalid_remote_context_fails_without_network() {
        #[derive(Debug)]
        struct UnsupportedContext;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking test listener");
        let endpoint = format!(
            "http://{}/evaluate",
            listener.local_addr().expect("listener address")
        );
        let provider = FlagshipServerProvider::new(endpoint_options(&endpoint, false)).unwrap();
        let context = EvaluationContext::default().with_custom_field(
            "unsupported",
            open_feature::EvaluationContextFieldValue::new_struct(UnsupportedContext),
        );

        let error = provider
            .resolve_bool_value("flag", &context)
            .await
            .unwrap_err();

        assert_eq!(error.code, EvaluationErrorCode::InvalidContext);
        assert!(matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ));
    }

    #[tokio::test]
    async fn remote_cache_excludes_disabled_type_mismatch_and_errors() {
        let (endpoint, requests, server) = scripted_server(vec![
            (
                200,
                br#"{"flagKey":"flag","value":false,"reason":"DISABLED"}"#,
            ),
            (200, br#"{"flagKey":"flag","value":true,"reason":"STATIC"}"#),
            (
                200,
                br#"{"flagKey":"typed","value":"wrong","reason":"STATIC"}"#,
            ),
            (
                200,
                br#"{"flagKey":"typed","value":true,"reason":"STATIC"}"#,
            ),
            (500, br#"{"error":"private backend body"}"#),
            (
                200,
                br#"{"flagKey":"error","value":true,"reason":"STATIC"}"#,
            ),
        ]);
        let provider = FlagshipServerProvider::builder(endpoint_options(&endpoint, false))
            .cache_ttl(Duration::from_secs(60))
            .build()
            .unwrap();
        let context = EvaluationContext::default();

        assert!(
            !provider
                .resolve_bool_value("flag", &context)
                .await
                .unwrap()
                .value
        );
        assert!(
            provider
                .resolve_bool_value("flag", &context)
                .await
                .unwrap()
                .value
        );
        assert_eq!(
            provider
                .resolve_bool_value("typed", &context)
                .await
                .unwrap_err()
                .code,
            EvaluationErrorCode::TypeMismatch
        );
        assert!(
            provider
                .resolve_bool_value("typed", &context)
                .await
                .unwrap()
                .value
        );
        let remote_error = provider
            .resolve_bool_value("error", &context)
            .await
            .unwrap_err();
        assert_eq!(
            remote_error.code,
            EvaluationErrorCode::General("GENERAL".to_owned())
        );
        assert!(!remote_error.message.unwrap().contains("private"));
        assert!(
            provider
                .resolve_bool_value("error", &context)
                .await
                .unwrap()
                .value
        );

        for _ in 0..6 {
            requests.recv().expect("network request");
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn remote_cache_isolates_expected_type_and_normalized_context() {
        let (endpoint, requests, server) = scripted_server(vec![
            (200, br#"{"flagKey":"same","value":true}"#),
            (200, br#"{"flagKey":"same","value":"text"}"#),
            (200, br#"{"flagKey":"context","value":true}"#),
            (200, br#"{"flagKey":"context","value":false}"#),
        ]);
        let provider = FlagshipServerProvider::builder(endpoint_options(&endpoint, false))
            .cache_ttl(Duration::from_secs(60))
            .build()
            .unwrap();
        let empty = EvaluationContext::default();
        let first = EvaluationContext::default().with_targeting_key("first");
        let second = EvaluationContext::default().with_targeting_key("second");

        assert!(
            provider
                .resolve_bool_value("same", &empty)
                .await
                .unwrap()
                .value
        );
        assert_eq!(
            provider
                .resolve_string_value("same", &empty)
                .await
                .unwrap()
                .value,
            "text"
        );
        assert!(
            provider
                .resolve_bool_value("context", &first)
                .await
                .unwrap()
                .value
        );
        assert!(
            !provider
                .resolve_bool_value("context", &second)
                .await
                .unwrap()
                .value
        );
        server.join().unwrap();

        assert_eq!(
            provider
                .resolve_bool_value("same", &empty)
                .await
                .unwrap()
                .reason,
            Some(OpenFeatureReason::Cached)
        );
        assert_eq!(
            provider
                .resolve_string_value("same", &empty)
                .await
                .unwrap()
                .reason,
            Some(OpenFeatureReason::Cached)
        );
        assert!(
            provider
                .resolve_bool_value("context", &first)
                .await
                .unwrap()
                .value
        );
        assert!(
            !provider
                .resolve_bool_value("context", &second)
                .await
                .unwrap()
                .value
        );
        for _ in 0..4 {
            requests.recv().expect("isolated network request");
        }
    }

    #[tokio::test]
    async fn shutdown_cache_clear_rejects_stale_insertion() {
        let provider = FlagshipServerProvider::builder(options(false))
            .cache_ttl(Duration::from_secs(60))
            .build()
            .unwrap();
        let ProviderMode::Remote {
            cache: Some(cache), ..
        } = &provider.inner.mode
        else {
            panic!("expected cached remote mode");
        };
        let context = normalize_context(&EvaluationContext::default()).unwrap();
        let token = cache
            .lookup("flag", ExpectedFlagType::Bool, &context)
            .unwrap()
            .token;

        provider.shutdown_provider().await;

        assert!(!cache.insert(token, FlagshipEvaluationResponse::new("flag", json!(true))));
    }

    #[tokio::test]
    async fn in_flight_remote_response_cannot_repopulate_cache_after_reinitialize() {
        let (endpoint, accepted, release, server) =
            blocking_server(br#"{"flagKey":"flag","value":true,"variant":"on","reason":"STATIC"}"#);
        let provider = FlagshipServerProvider::builder(endpoint_options(&endpoint, false))
            .cache_ttl(Duration::from_secs(60))
            .build()
            .unwrap();
        let resolving_provider = provider.clone();
        let resolution = tokio::spawn(async move {
            resolving_provider
                .resolve_bool_value("flag", &EvaluationContext::default())
                .await
        });
        accepted.await.expect("request accepted");

        provider.shutdown_provider().await;
        provider.initialize_provider().await.unwrap();
        release.send(()).expect("release response");
        assert!(resolution.await.unwrap().unwrap().value);
        server.join().unwrap();

        let ProviderMode::Remote {
            cache: Some(cache), ..
        } = &provider.inner.mode
        else {
            panic!("expected cached remote mode");
        };
        let context = normalize_context(&EvaluationContext::default()).unwrap();
        let lookup = cache
            .lookup("flag", ExpectedFlagType::Bool, &context)
            .unwrap();

        assert!(lookup.response.is_none());
    }

    #[tokio::test]
    async fn local_initialize_blocks_for_initial_snapshot_and_shutdown_clears_it() {
        let (endpoint, server) = server(br#"{"flags":{"flag":{"key":"flag","enabled":true,"default_variation":"on","variations":{"on":true},"rules":[]}}}"#);
        let provider = FlagshipServerProvider::builder(endpoint_options(&endpoint, true))
            .local_evaluation(true)
            .build()
            .expect("local provider");

        provider
            .initialize_provider()
            .await
            .expect("initial definitions load");
        server.join().expect("server thread");
        assert_eq!(provider.status(), ProviderStatus::Ready);
        assert!(
            provider
                .resolve_bool_value("flag", &EvaluationContext::default())
                .await
                .expect("local evaluation")
                .value
        );

        provider.shutdown_provider().await;
        assert_eq!(provider.status(), ProviderStatus::NotReady);
        assert!(provider.inner.lifecycle().snapshot.is_none());
    }

    #[tokio::test]
    async fn local_provider_resolves_all_open_feature_types_and_reasons() {
        let provider = local_provider();
        install(
            &provider,
            json!({"flags": {
                "bool": flag(json!(true), true),
                "string": flag(json!("text"), true),
                "int": flag(json!(-7), true),
                "float": flag(json!(2.5), true),
                "object": flag(json!({"nested": [true, 3, "x"]}), true),
                "disabled": flag(json!(false), false)
            }}),
        );
        let context = EvaluationContext::default();

        let boolean = provider.resolve_bool_value("bool", &context).await.unwrap();
        let string = provider
            .resolve_string_value("string", &context)
            .await
            .unwrap();
        let integer = provider.resolve_int_value("int", &context).await.unwrap();
        let float = provider
            .resolve_float_value("float", &context)
            .await
            .unwrap();
        let object = provider
            .resolve_struct_value("object", &context)
            .await
            .unwrap();
        let disabled = provider
            .resolve_bool_value("disabled", &context)
            .await
            .unwrap();

        assert!(boolean.value);
        assert_eq!(string.value, "text");
        assert_eq!(integer.value, -7);
        assert_eq!(float.value, 2.5);
        assert!(matches!(
            object.value.fields["nested"],
            OpenFeatureValue::Array(_)
        ));
        assert_eq!(disabled.reason, Some(OpenFeatureReason::Disabled));
        assert!(!disabled.value);
        assert_eq!(boolean.reason, Some(OpenFeatureReason::Static));
        assert_eq!(boolean.variant.as_deref(), Some("value"));
    }

    #[tokio::test]
    async fn exact_types_and_open_feature_null_limits_are_enforced() {
        let provider = local_provider();
        install(
            &provider,
            json!({"flags": {
                "fraction": flag(json!(1.5), true),
                "overflow": flag(json!(u64::MAX), true),
                "null": flag(JsonValue::Null, true),
                "nested_null": flag(json!({"value": null}), true),
                "array": flag(json!([1, 2]), true)
            }}),
        );
        let context = EvaluationContext::default();

        for error in [
            provider
                .resolve_int_value("fraction", &context)
                .await
                .unwrap_err(),
            provider
                .resolve_int_value("overflow", &context)
                .await
                .unwrap_err(),
            provider
                .resolve_struct_value("null", &context)
                .await
                .unwrap_err(),
            provider
                .resolve_struct_value("nested_null", &context)
                .await
                .unwrap_err(),
            provider
                .resolve_struct_value("array", &context)
                .await
                .unwrap_err(),
        ] {
            assert_eq!(error.code, EvaluationErrorCode::TypeMismatch);
            let message = error.message.expect("type mismatch message");
            assert!(message.starts_with("TYPE_MISMATCH: expected "));
            assert!(message.contains("actual JSON kind is "));
        }
    }

    #[tokio::test]
    async fn local_not_ready_missing_and_malformed_errors_are_isolated() {
        let provider = local_provider();
        let context = EvaluationContext::default();
        let error = provider
            .resolve_bool_value("secret", &context)
            .await
            .unwrap_err();
        assert_eq!(error.code, EvaluationErrorCode::ProviderNotReady);

        install(
            &provider,
            json!({"flags": {"bad": 7, "good": flag(json!(true), true)}}),
        );
        let missing = provider
            .resolve_bool_value("private\nmissing", &context)
            .await
            .unwrap_err();
        let malformed = provider
            .resolve_bool_value("bad", &context)
            .await
            .unwrap_err();
        let good = provider.resolve_bool_value("good", &context).await.unwrap();

        assert_eq!(missing.code, EvaluationErrorCode::FlagNotFound);
        assert!(!missing.message.unwrap().contains("private"));
        assert_eq!(malformed.code, EvaluationErrorCode::ParseError);
        assert!(good.value);
    }

    #[test]
    fn mappings_are_safe_and_unknown_reasons_are_preserved() {
        let response = FlagshipEvaluationResponse::new("private-key", json!(true))
            .with_variant("variant")
            .with_reason("FUTURE_REASON");
        assert_eq!(
            map_reason(&response.reason),
            OpenFeatureReason::Other("FUTURE_REASON".to_owned())
        );

        let error = FlagshipError::new(ErrorCode::BadRequest, "private URL and token")
            .with_server_code("FLAG_NOT_FOUND");
        let mapped = map_flagship_error(error);
        assert_eq!(
            mapped.code,
            EvaluationErrorCode::General("GENERAL".to_owned())
        );
        assert!(!mapped.message.unwrap().contains("private"));
    }

    #[test]
    fn debug_redacts_configuration() {
        let provider = local_provider();
        let debug = format!("{provider:?}");

        assert!(debug.contains("local"));
        assert!(debug.contains("hook_count"));
        assert!(!debug.contains("127.0.0.1"));
        assert!(!debug.contains("private-account"));
    }
}
