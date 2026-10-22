use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use open_feature::{
    EvaluationContext, EvaluationDetails, EvaluationError, EvaluationErrorCode, EvaluationReason,
    FlagMetadata, Hook, HookContext, HookHints, Value, async_trait,
};

/// The kind of telemetry callback represented by a [`TelemetryEvent`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TelemetryEventType {
    /// A provider evaluation reached this hook's `after` stage.
    Evaluation,
    /// An error from a before hook, provider (including the default no-op provider), or after hook
    /// reached this hook's `error` stage.
    Error,
}

/// Owned telemetry data from one `OpenFeature` hook callback.
///
/// This type deliberately contains the complete evaluation context, resolved value, variant, and
/// error message. Registering [`TelemetryHook`] therefore explicitly opts in to handing
/// potentially sensitive customer data to the callback. Its custom [`Debug`](fmt::Debug)
/// implementation does not print that data.
///
/// Timing is omitted with `open-feature` 0.3. That version gives each `before` call a temporary
/// [`HookContext`] and gives `after` or `error` a different, uncorrelatable context without an
/// evaluation ID. This hook does not guess by flag, context, thread, or task, and does not maintain
/// a collision-prone timing map. Because this event is non-exhaustive, timing can be added
/// compatibly if a future hook API provides safe correlation.
///
/// [`HookHints`] in `open-feature` 0.3 has no public non-default constructor, mutator, or accessor.
/// Applications therefore cannot attach readable hints and hooks cannot inspect or forward them.
///
/// `before` emits nothing. [`TelemetryEventType::Error`] can arise from a before hook, provider
/// (including the default no-op provider), or after hook, and its message may originate from any of
/// those sources. Depending on hook registration order, another after hook's failure can produce
/// `Evaluation` then `Error`, or `Error` only, while the caller still receives the provider's
/// successful value. Events are thus callback stages, not paired success/failure records or valid
/// success/failure counters. Structured-value conversion can also fail after `Evaluation` without
/// emitting `Error`.
#[non_exhaustive]
#[derive(Clone)]
pub struct TelemetryEvent {
    /// The kind of callback that produced this event.
    pub event_type: TelemetryEventType,
    /// The exact, unmodified flag key.
    pub flag_key: String,
    /// Wall-clock time at which this hook created the event.
    pub timestamp: SystemTime,
    /// A snapshot of the evaluation context supplied to this hook stage.
    ///
    /// For `Evaluation` events and `Error` events caused by provider or after-hook errors, this is
    /// the merged context used at provider resolution time. An `Error` caused by a before hook
    /// contains the context accumulated when the pipeline stopped. Cloning is shallow for opaque
    /// structured context fields held behind an [`Arc`].
    pub evaluation_context: EvaluationContext,
    /// The resolved value for an evaluation event.
    pub value: Option<Value>,
    /// The provider's evaluation reason for an evaluation event.
    pub reason: Option<EvaluationReason>,
    /// The provider's variant for an evaluation event.
    pub variant: Option<String>,
    /// The provider's flag metadata for an evaluation event.
    pub flag_metadata: Option<FlagMetadata>,
    /// The `OpenFeature` error code for an error event.
    pub error_code: Option<EvaluationErrorCode>,
    /// The `OpenFeature` error message for an error event.
    pub error_message: Option<String>,
}

impl TelemetryEvent {
    fn evaluation(context: &HookContext<'_>, details: &EvaluationDetails<Value>) -> Self {
        Self {
            event_type: TelemetryEventType::Evaluation,
            flag_key: context.flag_key.to_owned(),
            timestamp: SystemTime::now(),
            evaluation_context: context.evaluation_context.clone(),
            value: Some(details.value.clone()),
            reason: details.reason.clone(),
            variant: details.variant.clone(),
            flag_metadata: Some(details.flag_metadata.clone()),
            error_code: None,
            error_message: None,
        }
    }

    fn error(context: &HookContext<'_>, error: &EvaluationError) -> Self {
        Self {
            event_type: TelemetryEventType::Error,
            flag_key: context.flag_key.to_owned(),
            timestamp: SystemTime::now(),
            evaluation_context: context.evaluation_context.clone(),
            value: None,
            reason: None,
            variant: None,
            flag_metadata: None,
            error_code: Some(error.code.clone()),
            error_message: error.message.clone(),
        }
    }
}

impl fmt::Debug for TelemetryEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelemetryEvent")
            .field("event_type", &self.event_type)
            .field("flag_key_len", &self.flag_key.len())
            .field("timestamp", &self.timestamp)
            .field(
                "evaluation_context",
                &RedactedContext(&self.evaluation_context),
            )
            .field("value", &RedactedValue(self.value.as_ref()))
            .field("reason", &RedactedReason(self.reason.as_ref()))
            .field("variant", &RedactedText(self.variant.as_deref()))
            .field(
                "flag_metadata_fields",
                &self
                    .flag_metadata
                    .as_ref()
                    .map(|metadata| metadata.values.len()),
            )
            .field("error_code", &RedactedErrorCode(self.error_code.as_ref()))
            .field(
                "error_message",
                &RedactedText(self.error_message.as_deref()),
            )
            .finish_non_exhaustive()
    }
}

struct RedactedContext<'a>(&'a EvaluationContext);

impl fmt::Debug for RedactedContext<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvaluationContext")
            .field("targeting_key_present", &self.0.targeting_key.is_some())
            .field("custom_field_count", &self.0.custom_fields.len())
            .finish_non_exhaustive()
    }
}

struct RedactedValue<'a>(Option<&'a Value>);

impl fmt::Debug for RedactedValue<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => formatter
                .debug_tuple("Some")
                .field(&value.get_type())
                .finish(),
            None => formatter.write_str("None"),
        }
    }
}

struct RedactedText<'a>(Option<&'a str>);

impl fmt::Debug for RedactedText<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => formatter
                .debug_struct("Some")
                .field("len", &value.len())
                .finish(),
            None => formatter.write_str("None"),
        }
    }
}

struct RedactedReason<'a>(Option<&'a EvaluationReason>);

impl fmt::Debug for RedactedReason<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(reason) = self.0 else {
            return formatter.write_str("None");
        };
        let name = match reason {
            EvaluationReason::Static => "Static",
            EvaluationReason::Default => "Default",
            EvaluationReason::TargetingMatch => "TargetingMatch",
            EvaluationReason::Split => "Split",
            EvaluationReason::Cached => "Cached",
            EvaluationReason::Disabled => "Disabled",
            EvaluationReason::Unknown => "Unknown",
            EvaluationReason::Error => "Error",
            EvaluationReason::Other(_) => "Other",
        };

        formatter.debug_tuple("Some").field(&name).finish()
    }
}

struct RedactedErrorCode<'a>(Option<&'a EvaluationErrorCode>);

impl fmt::Debug for RedactedErrorCode<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(code) = self.0 else {
            return formatter.write_str("None");
        };
        let name = match code {
            EvaluationErrorCode::ProviderNotReady => "ProviderNotReady",
            EvaluationErrorCode::FlagNotFound => "FlagNotFound",
            EvaluationErrorCode::ParseError => "ParseError",
            EvaluationErrorCode::TypeMismatch => "TypeMismatch",
            EvaluationErrorCode::TargetingKeyMissing => "TargetingKeyMissing",
            EvaluationErrorCode::InvalidContext => "InvalidContext",
            EvaluationErrorCode::General(_) => "General",
        };

        formatter.debug_tuple("Some").field(&name).finish()
    }
}

type TelemetryCallback = dyn Fn(TelemetryEvent) + Send + Sync;

/// An `OpenFeature` hook that synchronously emits owned telemetry events.
///
/// Clones cheaply share the callback through an [`Arc`]. `before` does not modify context or emit
/// an event, `after` emits one evaluation event, `error` emits one error event, and `finally` is a
/// no-op. The callback runs inline on the evaluating task and may be called concurrently. It should
/// not block; prefer handing events to a nonblocking channel serviced by separate telemetry work.
///
/// A callback panic is not caught. With unwinding enabled it unwinds through evaluation and skips
/// remaining hooks and `finally`; with `panic = "abort"` it aborts the process.
///
/// # Sensitive data
///
/// Telemetry events contain customer context, resolved values, flag keys, variants, and error
/// messages. The callback must store and transmit them appropriately.
///
/// # Example
///
/// ```no_run
/// use cloudflare_flagship::config::FlagshipClientOptions;
/// use cloudflare_flagship::hooks::TelemetryHook;
/// use cloudflare_flagship::provider::FlagshipServerProvider;
/// use open_feature::OpenFeature;
/// use std::sync::mpsc::sync_channel;
/// use std::thread;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// // This bounded handoff does not block the evaluating task. A full queue drops the event.
/// let (sender, receiver) = sync_channel(1_024);
/// thread::spawn(move || {
///     for event in receiver {
///         send_to_customer_data_pipeline(event);
///     }
/// });
/// // The callback receives sensitive customer context and resolved values.
/// let telemetry = TelemetryHook::new(move |event| {
///     let _ = sender.try_send(event);
/// });
/// let options = FlagshipClientOptions::builder()
///     .app_id("app-123")
///     .account_id("account-456")
///     .build()?;
/// let provider = FlagshipServerProvider::builder(options)
///     .hook(telemetry)
///     .build()?;
/// let mut api = OpenFeature::default();
/// api.set_provider(provider).await;
/// # Ok(())
/// # }
/// # fn send_to_customer_data_pipeline(_: cloudflare_flagship::hooks::TelemetryEvent) {}
/// ```
#[derive(Clone)]
pub struct TelemetryHook {
    callback: Arc<TelemetryCallback>,
}

impl TelemetryHook {
    /// Creates a telemetry hook from a thread-safe callback that owns every event.
    #[must_use]
    pub fn new(callback: impl Fn(TelemetryEvent) + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    fn emit(&self, event: TelemetryEvent) {
        (self.callback)(event);
    }
}

impl fmt::Debug for TelemetryHook {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelemetryHook")
            .field("callback", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Hook for TelemetryHook {
    async fn before<'a>(
        &self,
        _context: &HookContext<'a>,
        _hints: Option<&'a HookHints>,
    ) -> Result<Option<EvaluationContext>, EvaluationError> {
        Ok(None)
    }

    async fn after<'a>(
        &self,
        context: &HookContext<'a>,
        details: &EvaluationDetails<Value>,
        _hints: Option<&'a HookHints>,
    ) -> Result<(), EvaluationError> {
        self.emit(TelemetryEvent::evaluation(context, details));

        Ok(())
    }

    async fn error<'a>(
        &self,
        context: &HookContext<'a>,
        error: &EvaluationError,
        _hints: Option<&'a HookHints>,
    ) {
        self.emit(TelemetryEvent::error(context, error));
    }

    async fn finally<'a>(
        &self,
        _context: &HookContext<'a>,
        _evaluation_details: &EvaluationDetails<Value>,
        _hints: Option<&'a HookHints>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    use open_feature::provider::{FeatureProvider, ProviderMetadata, ResolutionDetails};
    use open_feature::{
        Client, ClientMetadata, EvaluationContext, EvaluationDetails, EvaluationError,
        EvaluationErrorCode, EvaluationOptions, EvaluationReason, FlagMetadata, Hook, HookContext,
        HookHints, HookWrapper, OpenFeature, StructValue, Type, Value, async_trait,
    };

    use super::{TelemetryEvent, TelemetryEventType, TelemetryHook};

    fn hook_context<'a>(
        flag_key: &'a str,
        evaluation_context: &'a EvaluationContext,
    ) -> HookContext<'a> {
        HookContext {
            flag_key,
            flag_type: Type::String,
            evaluation_context,
            provider_metadata: ProviderMetadata::new("provider"),
            default_value: Some(Value::String("default secret".to_owned())),
            client_metadata: ClientMetadata {
                name: "client".to_owned(),
            },
        }
    }

    fn details() -> EvaluationDetails<Value> {
        EvaluationDetails {
            flag_key: "detail-key".to_owned(),
            value: Value::String("resolved secret".to_owned()),
            reason: Some(EvaluationReason::Other("custom secret".to_owned())),
            variant: Some("variant secret".to_owned()),
            flag_metadata: FlagMetadata::default().with_value("metadata secret", "value secret"),
        }
    }

    fn evaluation_error() -> EvaluationError {
        EvaluationError::builder()
            .code(EvaluationErrorCode::General("code secret".to_owned()))
            .message("message secret")
            .build()
    }

    fn recording_hook() -> (TelemetryHook, Arc<Mutex<Vec<TelemetryEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback_events = Arc::clone(&events);
        let hook = TelemetryHook::new(move |event| {
            callback_events
                .lock()
                .expect("recording telemetry events mutex must not be poisoned")
                .push(event);
        });

        (hook, events)
    }

    fn recorded(events: &Arc<Mutex<Vec<TelemetryEvent>>>) -> Vec<TelemetryEvent> {
        events
            .lock()
            .expect("recorded telemetry events mutex must not be poisoned")
            .clone()
    }

    #[tokio::test]
    async fn direct_events_preserve_exact_owned_fields_and_timestamp() {
        let (hook, events) = recording_hook();
        let mut evaluation_context = EvaluationContext::default()
            .with_targeting_key("customer secret")
            .with_custom_field("email", "person@example.com");
        let context = hook_context("exact flag\nkey", &evaluation_context);
        let mut evaluation_details = details();
        let earliest = SystemTime::now();

        hook.after(&context, &evaluation_details, None)
            .await
            .expect("after");
        let latest = SystemTime::now();

        drop(context);
        evaluation_context.targeting_key = Some("changed".to_owned());
        evaluation_details.value = Value::String("changed".to_owned());

        let events = recorded(&events);
        let event = &events[0];
        assert_eq!(event.event_type, TelemetryEventType::Evaluation);
        assert_eq!(event.flag_key, "exact flag\nkey");
        assert!(event.timestamp >= earliest && event.timestamp <= latest);
        assert_eq!(
            event.evaluation_context.targeting_key.as_deref(),
            Some("customer secret")
        );
        assert_eq!(
            event.value,
            Some(Value::String("resolved secret".to_owned()))
        );
        assert_eq!(
            event.reason,
            Some(EvaluationReason::Other("custom secret".to_owned()))
        );
        assert_eq!(event.variant.as_deref(), Some("variant secret"));
        assert_eq!(
            event.flag_metadata.as_ref().map(|value| value.values.len()),
            Some(1)
        );
        assert_eq!(event.error_code, None);
        assert_eq!(event.error_message, None);
    }

    #[tokio::test]
    async fn error_event_has_exact_error_fields() {
        let (hook, events) = recording_hook();
        let evaluation_context = EvaluationContext::default();
        let context = hook_context("flag", &evaluation_context);
        let error = evaluation_error();

        hook.error(&context, &error, None).await;

        let events = recorded(&events);
        let event = &events[0];
        assert_eq!(event.event_type, TelemetryEventType::Error);
        assert_eq!(
            event.error_code,
            Some(EvaluationErrorCode::General("code secret".to_owned()))
        );
        assert_eq!(event.error_message.as_deref(), Some("message secret"));
        assert_eq!(event.value, None);
        assert_eq!(event.reason, None);
        assert_eq!(event.variant, None);
        assert_eq!(event.flag_metadata, None);
    }

    #[tokio::test]
    async fn before_and_finally_are_noops() {
        let (hook, events) = recording_hook();
        let evaluation_context = EvaluationContext::default();
        let context = hook_context("flag", &evaluation_context);
        let details = details();

        let before = hook.before(&context, None).await.expect("before");
        hook.finally(&context, &details, None).await;

        assert_eq!(before, None);
        assert_eq!(recorded(&events).len(), 0);
    }

    #[tokio::test]
    async fn clones_share_callback_and_stages_are_not_forced_into_pairs() {
        let (hook, events) = recording_hook();
        let evaluation_context = EvaluationContext::default();
        let context = hook_context("flag", &evaluation_context);

        hook.clone()
            .after(&context, &details(), None)
            .await
            .expect("after");
        hook.error(&context, &evaluation_error(), None).await;

        let event_types: Vec<_> = recorded(&events)
            .iter()
            .map(|event| event.event_type)
            .collect();
        assert_eq!(
            event_types,
            vec![TelemetryEventType::Evaluation, TelemetryEventType::Error]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_evaluations_keep_owned_contexts_and_values_distinct() {
        let (hook, events) = recording_hook();
        let mut tasks = Vec::new();

        for index in 0..64 {
            let hook = hook.clone();
            tasks.push(tokio::spawn(async move {
                let key = format!("flag-{index}");
                let evaluation_context =
                    EvaluationContext::default().with_targeting_key(format!("customer-{index}"));
                let context = hook_context(&key, &evaluation_context);
                let mut details = details();
                details.value = Value::Int(index);

                hook.after(&context, &details, None).await.expect("after");
            }));
        }

        for task in tasks {
            task.await.expect("task");
        }

        let mut records: Vec<_> = recorded(&events)
            .iter()
            .map(|event| {
                (
                    event.flag_key.clone(),
                    event.evaluation_context.targeting_key.clone(),
                    event.value.clone(),
                )
            })
            .collect();
        records.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(records.len(), 64);
        assert!(records.iter().all(|(key, context, value)| {
            let index = key.strip_prefix("flag-").expect("key index");

            context.as_deref() == Some(&format!("customer-{index}"))
                && value == &Some(Value::Int(index.parse().expect("numeric index")))
        }));
    }

    #[test]
    fn debug_redacts_all_sensitive_text_and_reports_types_and_presence() {
        let evaluation_context = EvaluationContext::default()
            .with_targeting_key("target secret")
            .with_custom_field("field secret", "context secret");
        let context = hook_context("flag secret", &evaluation_context);
        let mut event = TelemetryEvent::evaluation(&context, &details());
        event.error_code = Some(EvaluationErrorCode::General("code secret".to_owned()));
        event.error_message = Some("message secret".to_owned());

        let output = format!("{event:?}");

        for secret in [
            "flag secret",
            "target secret",
            "field secret",
            "context secret",
            "resolved secret",
            "custom secret",
            "variant secret",
            "metadata secret",
            "code secret",
            "message secret",
        ] {
            assert!(!output.contains(secret), "debug leaked {secret:?}");
        }
        assert!(output.contains("flag_key_len: 11"));
        assert!(output.contains("value: Some(String)"));
        assert!(output.contains("targeting_key_present: true"));
        assert!(!output.contains("targeting_key_len"));
        assert!(output.contains("custom_field_count: 1"));
        assert!(output.contains("error_message: Some { len: 14 }"));

        let error_output = format!("{:?}", TelemetryEvent::error(&context, &evaluation_error()));
        assert!(error_output.contains("value: None"));
        assert!(error_output.contains("reason: None"));
        assert!(error_output.contains("variant: None"));
    }

    #[test]
    fn hook_is_send_sync_and_constructs_hook_wrapper() {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}

        assert_send_sync_static::<TelemetryHook>();
        assert_send_sync_static::<TelemetryEvent>();
        let hook = TelemetryHook::new(|_| {});
        let output = format!("{hook:?}");
        let _wrapper = HookWrapper::new(hook);
        assert!(output.contains("[REDACTED]"));
    }

    #[derive(Clone, Copy)]
    enum ProviderBehavior {
        Success,
        Error,
    }

    struct TestProvider {
        metadata: ProviderMetadata,
        behavior: ProviderBehavior,
        bool_calls: Option<Arc<AtomicUsize>>,
        hooks: Vec<HookWrapper>,
    }

    impl TestProvider {
        fn new(behavior: ProviderBehavior) -> Self {
            Self {
                metadata: ProviderMetadata::new("test-provider"),
                behavior,
                bool_calls: None,
                hooks: Vec::new(),
            }
        }

        fn with_bool_calls(mut self, calls: Arc<AtomicUsize>) -> Self {
            self.bool_calls = Some(calls);

            self
        }

        fn with_hook(mut self, hook: impl Hook) -> Self {
            self.hooks.push(HookWrapper::new(hook));

            self
        }
    }

    #[async_trait]
    impl FeatureProvider for TestProvider {
        fn metadata(&self) -> &ProviderMetadata {
            &self.metadata
        }

        fn hooks(&self) -> &[HookWrapper] {
            &self.hooks
        }

        async fn resolve_bool_value(
            &self,
            _flag_key: &str,
            _evaluation_context: &EvaluationContext,
        ) -> Result<ResolutionDetails<bool>, EvaluationError> {
            if let Some(calls) = &self.bool_calls {
                calls.fetch_add(1, Ordering::Relaxed);
            }

            match self.behavior {
                ProviderBehavior::Success => Ok(ResolutionDetails::new(true)),
                ProviderBehavior::Error => Err(evaluation_error()),
            }
        }

        async fn resolve_int_value(
            &self,
            _flag_key: &str,
            _evaluation_context: &EvaluationContext,
        ) -> Result<ResolutionDetails<i64>, EvaluationError> {
            Ok(ResolutionDetails::new(1))
        }

        async fn resolve_float_value(
            &self,
            _flag_key: &str,
            _evaluation_context: &EvaluationContext,
        ) -> Result<ResolutionDetails<f64>, EvaluationError> {
            Ok(ResolutionDetails::new(1.0))
        }

        async fn resolve_string_value(
            &self,
            _flag_key: &str,
            _evaluation_context: &EvaluationContext,
        ) -> Result<ResolutionDetails<String>, EvaluationError> {
            Ok(ResolutionDetails::new("value".to_owned()))
        }

        async fn resolve_struct_value(
            &self,
            _flag_key: &str,
            _evaluation_context: &EvaluationContext,
        ) -> Result<ResolutionDetails<StructValue>, EvaluationError> {
            Ok(ResolutionDetails::new(StructValue::default()))
        }
    }

    async fn client(behavior: ProviderBehavior) -> Client {
        let mut api = OpenFeature::default();
        api.set_provider(TestProvider::new(behavior)).await;

        api.create_named_client("pipeline-client")
    }

    #[tokio::test]
    async fn isolated_pipeline_emits_one_success_event() {
        let (hook, events) = recording_hook();
        let client = client(ProviderBehavior::Success).await.with_hook(hook);
        let context = EvaluationContext::default().with_targeting_key("pipeline customer");

        let result = client
            .get_bool_value("pipeline flag", Some(&context), None)
            .await;

        assert_eq!(result, Ok(true));
        let events = recorded(&events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, TelemetryEventType::Evaluation);
        assert_eq!(events[0].flag_key, "pipeline flag");
        assert_eq!(events[0].value, Some(Value::Bool(true)));
    }

    #[tokio::test]
    async fn isolated_pipeline_emits_one_provider_error_event() {
        let (hook, events) = recording_hook();
        let client = client(ProviderBehavior::Error).await.with_hook(hook);

        let result = client.get_bool_value("pipeline flag", None, None).await;

        assert_eq!(result, Err(evaluation_error()));
        let events = recorded(&events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, TelemetryEventType::Error);
        assert_eq!(events[0].error_code, Some(evaluation_error().code));
    }

    struct FailingBeforeHook;

    #[async_trait]
    impl Hook for FailingBeforeHook {
        async fn before<'a>(
            &self,
            _context: &HookContext<'a>,
            _hints: Option<&'a HookHints>,
        ) -> Result<Option<EvaluationContext>, EvaluationError> {
            Err(evaluation_error())
        }

        async fn after<'a>(
            &self,
            _context: &HookContext<'a>,
            _details: &EvaluationDetails<Value>,
            _hints: Option<&'a HookHints>,
        ) -> Result<(), EvaluationError> {
            Ok(())
        }

        async fn error<'a>(
            &self,
            _context: &HookContext<'a>,
            _error: &EvaluationError,
            _hints: Option<&'a HookHints>,
        ) {
        }

        async fn finally<'a>(
            &self,
            _context: &HookContext<'a>,
            _details: &EvaluationDetails<Value>,
            _hints: Option<&'a HookHints>,
        ) {
        }
    }

    #[tokio::test]
    async fn pipeline_before_failure_emits_error_only_and_skips_provider() {
        let (hook, events) = recording_hook();
        let calls = Arc::new(AtomicUsize::new(0));
        let provider =
            TestProvider::new(ProviderBehavior::Success).with_bool_calls(Arc::clone(&calls));
        let mut api = OpenFeature::default();
        api.set_provider(provider).await;
        let client = api
            .create_named_client("pipeline-client")
            .with_hook(FailingBeforeHook)
            .with_hook(hook);

        let result = client.get_bool_value("flag", None, None).await;

        assert_eq!(result, Err(evaluation_error()));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        let events = recorded(&events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, TelemetryEventType::Error);
    }

    struct ContextHook;

    #[async_trait]
    impl Hook for ContextHook {
        async fn before<'a>(
            &self,
            _context: &HookContext<'a>,
            _hints: Option<&'a HookHints>,
        ) -> Result<Option<EvaluationContext>, EvaluationError> {
            Ok(Some(
                EvaluationContext::default()
                    .with_targeting_key("hook target")
                    .with_custom_field("from-hook", true),
            ))
        }

        async fn after<'a>(
            &self,
            _context: &HookContext<'a>,
            _details: &EvaluationDetails<Value>,
            _hints: Option<&'a HookHints>,
        ) -> Result<(), EvaluationError> {
            Ok(())
        }

        async fn error<'a>(
            &self,
            _context: &HookContext<'a>,
            _error: &EvaluationError,
            _hints: Option<&'a HookHints>,
        ) {
        }

        async fn finally<'a>(
            &self,
            _context: &HookContext<'a>,
            _details: &EvaluationDetails<Value>,
            _hints: Option<&'a HookHints>,
        ) {
        }
    }

    #[tokio::test]
    async fn pipeline_event_owns_context_merged_by_before_hooks() {
        let (hook, events) = recording_hook();
        let client = client(ProviderBehavior::Success)
            .await
            .with_hook(ContextHook)
            .with_hook(hook);
        let invocation_context =
            EvaluationContext::default().with_custom_field("from-invocation", true);

        let result = client
            .get_bool_value("flag", Some(&invocation_context), None)
            .await;

        assert_eq!(result, Ok(true));
        let events = recorded(&events);
        let context = &events[0].evaluation_context;
        assert_eq!(context.targeting_key.as_deref(), Some("hook target"));
        assert!(context.custom_fields.contains_key("from-hook"));
        assert!(context.custom_fields.contains_key("from-invocation"));
    }

    #[tokio::test]
    async fn provider_registered_hook_emits_evaluation_event() {
        let (hook, events) = recording_hook();
        let provider = TestProvider::new(ProviderBehavior::Success).with_hook(hook);
        let mut api = OpenFeature::default();
        api.set_provider(provider).await;
        let client = api.create_named_client("pipeline-client");

        let result = client.get_bool_value("flag", None, None).await;

        assert_eq!(result, Ok(true));
        let events = recorded(&events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, TelemetryEventType::Evaluation);
    }

    struct FailingAfterHook;

    #[async_trait]
    impl Hook for FailingAfterHook {
        async fn before<'a>(
            &self,
            _context: &HookContext<'a>,
            _hints: Option<&'a HookHints>,
        ) -> Result<Option<EvaluationContext>, EvaluationError> {
            Ok(None)
        }

        async fn after<'a>(
            &self,
            _context: &HookContext<'a>,
            _details: &EvaluationDetails<Value>,
            _hints: Option<&'a HookHints>,
        ) -> Result<(), EvaluationError> {
            Err(evaluation_error())
        }

        async fn error<'a>(
            &self,
            _context: &HookContext<'a>,
            _error: &EvaluationError,
            _hints: Option<&'a HookHints>,
        ) {
        }

        async fn finally<'a>(
            &self,
            _context: &HookContext<'a>,
            _details: &EvaluationDetails<Value>,
            _hints: Option<&'a HookHints>,
        ) {
        }
    }

    #[tokio::test]
    async fn pipeline_after_can_be_followed_by_error_from_another_hook() {
        let (hook, events) = recording_hook();
        let client = client(ProviderBehavior::Success)
            .await
            .with_hook(FailingAfterHook)
            .with_hook(hook);

        let result = client.get_bool_value("flag", None, None).await;

        assert_eq!(result, Ok(true));
        let event_types: Vec<_> = recorded(&events)
            .iter()
            .map(|event| event.event_type)
            .collect();
        assert_eq!(
            event_types,
            vec![TelemetryEventType::Evaluation, TelemetryEventType::Error]
        );
    }

    #[tokio::test]
    async fn invocation_after_failure_emits_error_only_and_returns_provider_value() {
        let (hook, events) = recording_hook();
        let client = client(ProviderBehavior::Success).await.with_hook(hook);
        let options = EvaluationOptions::default().with_hook(FailingAfterHook);

        let result = client.get_bool_value("flag", None, Some(&options)).await;

        assert_eq!(result, Ok(true));
        let events = recorded(&events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, TelemetryEventType::Error);
        assert_eq!(events[0].error_message.as_deref(), Some("message secret"));
    }

    #[derive(Debug)]
    struct ConversionFailure;

    impl TryFrom<StructValue> for ConversionFailure {
        type Error = ();

        fn try_from(_value: StructValue) -> Result<Self, Self::Error> {
            Err(())
        }
    }

    #[tokio::test]
    async fn pipeline_conversion_failure_after_evaluation_emits_no_error_event() {
        let (hook, events) = recording_hook();
        let client = client(ProviderBehavior::Success).await.with_hook(hook);

        let result = client
            .get_struct_value::<ConversionFailure>("flag", None, None)
            .await;

        assert_eq!(
            result.expect_err("conversion must fail").code,
            EvaluationErrorCode::TypeMismatch
        );
        let events = recorded(&events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, TelemetryEventType::Evaluation);
        assert_eq!(events[0].value, Some(Value::Struct(StructValue::default())));
    }
}
