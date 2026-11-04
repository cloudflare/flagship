use std::fmt;
use std::sync::Arc;

use open_feature::{
    EvaluationContext, EvaluationDetails, EvaluationError, EvaluationErrorCode, EvaluationReason,
    Hook, HookContext, HookHints, Value, async_trait,
};

/// The hook stage that produced a [`LoggingEvent`], with its stage-specific data.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoggingEventKind {
    /// The hook's `before` stage ran.
    Before,
    /// The hook's `after` stage ran.
    After {
        /// The provider's evaluation reason.
        reason: Option<EvaluationReason>,
        /// The provider's variant.
        variant: Option<String>,
    },
    /// The hook's `error` stage ran.
    Error {
        /// The `OpenFeature` error code.
        code: EvaluationErrorCode,
        /// The `OpenFeature` error message.
        message: Option<String>,
    },
}

/// Information from one [`LoggingHook`] stage.
///
/// Never includes the evaluation context, default value, or resolved value. Strings are passed
/// raw and may contain control characters, so escape them as appropriate for your log format.
///
/// Events describe individual hook calls, not paired evaluations: `Error` can occur without
/// `Before`, and `After` can be followed by `Error` when a later `after` hook fails, even though
/// the caller still receives the provider's value.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoggingEvent {
    /// The stage that produced this event.
    pub kind: LoggingEventKind,
    /// The flag key.
    pub flag_key: String,
    /// The provider name.
    pub provider_name: String,
    /// The client name.
    pub client_name: String,
}

impl LoggingEvent {
    fn new(kind: LoggingEventKind, context: &HookContext<'_>) -> Self {
        Self {
            kind,
            flag_key: context.flag_key.to_owned(),
            provider_name: context.provider_metadata.name.clone(),
            client_name: context.client_metadata.name.clone(),
        }
    }
}

type LoggingCallback = dyn Fn(LoggingEvent) + Send + Sync;

/// An `OpenFeature` hook that emits structured [`LoggingEvent`]s.
///
/// The callback runs synchronously once per `before`, successful `after`, and `error` call;
/// `finally` does nothing. Clones share the callback and are safe for concurrent evaluations. The
/// callback receives each event by value, so it can queue it without cloning.
///
/// The callback should offload expensive work and must not panic: panics are not caught and can
/// skip remaining error or finally hooks.
///
/// # Example
///
/// ```no_run
/// use cloudflare_flagship::config::FlagshipClientOptions;
/// use cloudflare_flagship::hooks::LoggingHook;
/// use cloudflare_flagship::provider::FlagshipServerProvider;
/// use open_feature::OpenFeature;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let hook = LoggingHook::new(|event| {
///     eprintln!("{:?}: {:?}", event.kind, event.flag_key);
/// });
/// let options = FlagshipClientOptions::builder()
///     .app_id("app-123")
///     .account_id("account-456")
///     .build()?;
/// let provider = FlagshipServerProvider::builder(options).hook(hook).build()?;
/// let mut api = OpenFeature::default();
/// api.set_provider(provider).await;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct LoggingHook {
    callback: Arc<LoggingCallback>,
}

impl LoggingHook {
    /// Creates a logging hook from a thread-safe callback that receives each event by value.
    #[must_use]
    pub fn new(callback: impl Fn(LoggingEvent) + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    fn emit(&self, event: LoggingEvent) {
        (self.callback)(event);
    }
}

impl fmt::Debug for LoggingHook {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoggingHook")
            .field("callback", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Hook for LoggingHook {
    async fn before<'a>(
        &self,
        context: &HookContext<'a>,
        _hints: Option<&'a HookHints>,
    ) -> Result<Option<EvaluationContext>, EvaluationError> {
        self.emit(LoggingEvent::new(LoggingEventKind::Before, context));

        Ok(None)
    }

    async fn after<'a>(
        &self,
        context: &HookContext<'a>,
        details: &EvaluationDetails<Value>,
        _hints: Option<&'a HookHints>,
    ) -> Result<(), EvaluationError> {
        self.emit(LoggingEvent::new(
            LoggingEventKind::After {
                reason: details.reason.clone(),
                variant: details.variant.clone(),
            },
            context,
        ));

        Ok(())
    }

    async fn error<'a>(
        &self,
        context: &HookContext<'a>,
        error: &EvaluationError,
        _hints: Option<&'a HookHints>,
    ) {
        self.emit(LoggingEvent::new(
            LoggingEventKind::Error {
                code: error.code.clone(),
                message: error.message.clone(),
            },
            context,
        ));
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
    use std::sync::{Arc, Mutex};

    use open_feature::provider::{FeatureProvider, ProviderMetadata, ResolutionDetails};
    use open_feature::{
        Client, ClientMetadata, EvaluationContext, EvaluationDetails, EvaluationError,
        EvaluationErrorCode, EvaluationReason, FlagMetadata, Hook, HookContext, HookHints,
        HookWrapper, OpenFeature, StructValue, Type, Value, async_trait,
    };

    use super::{LoggingEvent, LoggingEventKind, LoggingHook};

    fn context<'a>(
        flag_key: &'a str,
        evaluation_context: &'a EvaluationContext,
    ) -> HookContext<'a> {
        HookContext {
            flag_key,
            flag_type: Type::Bool,
            evaluation_context,
            provider_metadata: ProviderMetadata::new("provider"),
            default_value: Some(Value::String("secret default".to_owned())),
            client_metadata: ClientMetadata {
                name: "client".to_owned(),
            },
        }
    }

    fn details(reason: EvaluationReason, variant: &str) -> EvaluationDetails<Value> {
        EvaluationDetails {
            flag_key: "flag".to_owned(),
            value: Value::String("secret result".to_owned()),
            reason: Some(reason),
            variant: Some(variant.to_owned()),
            flag_metadata: FlagMetadata::default(),
        }
    }

    fn recording_hook() -> (LoggingHook, Arc<Mutex<Vec<LoggingEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback_events = Arc::clone(&events);
        let hook = LoggingHook::new(move |event| {
            callback_events
                .lock()
                .expect("recording logging events mutex must not be poisoned")
                .push(event);
        });

        (hook, events)
    }

    fn stages(events: &Arc<Mutex<Vec<LoggingEvent>>>) -> Vec<&'static str> {
        events
            .lock()
            .expect("recorded logging events mutex must not be poisoned")
            .iter()
            .map(|event| match event.kind {
                LoggingEventKind::Before => "before",
                LoggingEventKind::After { .. } => "after",
                LoggingEventKind::Error { .. } => "error",
            })
            .collect()
    }

    #[tokio::test]
    async fn direct_methods_emit_raw_stage_data_and_finally_is_noop() {
        let (hook, events) = recording_hook();
        let evaluation_context = EvaluationContext::default()
            .with_targeting_key("secret target")
            .with_custom_field("email", "secret@example.com");
        let hook_context = context("flag\n\u{202e}", &evaluation_context);
        let long_variant = "v".repeat(4_096);
        let details = details(
            EvaluationReason::Other("custom\n".to_owned()),
            &long_variant,
        );
        let error = EvaluationError::builder()
            .code(EvaluationErrorCode::General("code\n".to_owned()))
            .message("not\nfound")
            .build();

        let before_result = hook.before(&hook_context, None).await.expect("before");
        hook.after(&hook_context, &details, None)
            .await
            .expect("after");
        hook.error(&hook_context, &error, None).await;
        hook.finally(&hook_context, &details, None).await;

        assert_eq!(before_result, None);
        let events = events
            .lock()
            .expect("recorded logging events mutex must not be poisoned");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].kind, LoggingEventKind::Before);
        assert_eq!(events[0].flag_key, "flag\n\u{202e}");
        assert_eq!(events[0].provider_name, "provider");
        assert_eq!(events[0].client_name, "client");
        assert_eq!(
            events[1].kind,
            LoggingEventKind::After {
                reason: Some(EvaluationReason::Other("custom\n".to_owned())),
                variant: Some(long_variant),
            }
        );
        assert_eq!(
            events[2].kind,
            LoggingEventKind::Error {
                code: EvaluationErrorCode::General("code\n".to_owned()),
                message: Some("not\nfound".to_owned()),
            }
        );

        let output = format!("{events:?}");
        for (index, secret) in [
            "secret target",
            "secret@example.com",
            "secret default",
            "secret result",
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                !output.contains(secret),
                "event leaked sensitive fixture #{index}"
            );
        }
    }

    #[test]
    fn hook_debug_redacts_callback() {
        let output = format!("{:?}", LoggingHook::new(|_| {}));

        assert!(output.contains("callback: \"[REDACTED]\""));
        assert!(output.ends_with(".. }"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_callbacks_receive_distinct_owned_events() {
        let (hook, events) = recording_hook();
        let mut tasks = Vec::new();

        for index in 0..64 {
            let hook = hook.clone();
            tasks.push(tokio::spawn(async move {
                let evaluation_context = EvaluationContext::default();
                let key = format!("flag-{index}");
                let hook_context = context(&key, &evaluation_context);
                hook.before(&hook_context, None).await.expect("before");
            }));
        }

        for task in tasks {
            task.await.expect("task");
        }

        let mut keys: Vec<_> = events
            .lock()
            .expect("recorded logging events mutex must not be poisoned")
            .iter()
            .map(|event| event.flag_key.clone())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 64);
        assert_eq!(keys.first().map(String::as_str), Some("flag-0"));
        assert!(keys.iter().all(|key| key.starts_with("flag-")));
    }

    #[test]
    fn is_send_sync_and_constructs_hook_wrapper() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<LoggingHook>();
        assert_send_sync::<LoggingEvent>();
        let _wrapper = HookWrapper::new(LoggingHook::new(|_| {}));
    }

    #[derive(Clone, Copy)]
    enum ProviderBehavior {
        Success,
        BoolFailure,
    }

    struct TestProvider {
        metadata: ProviderMetadata,
        behavior: ProviderBehavior,
    }

    impl TestProvider {
        fn new(behavior: ProviderBehavior) -> Self {
            Self {
                metadata: ProviderMetadata::new("test-provider"),
                behavior,
            }
        }
    }

    #[async_trait]
    impl FeatureProvider for TestProvider {
        fn metadata(&self) -> &ProviderMetadata {
            &self.metadata
        }

        async fn resolve_bool_value(
            &self,
            _flag_key: &str,
            _evaluation_context: &EvaluationContext,
        ) -> Result<ResolutionDetails<bool>, EvaluationError> {
            match self.behavior {
                ProviderBehavior::Success => Ok(ResolutionDetails::new(true)),
                ProviderBehavior::BoolFailure => Err(test_error()),
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

    struct FailingHook {
        before: bool,
        after: bool,
    }

    #[async_trait]
    impl Hook for FailingHook {
        async fn before<'a>(
            &self,
            _context: &HookContext<'a>,
            _hints: Option<&'a HookHints>,
        ) -> Result<Option<EvaluationContext>, EvaluationError> {
            if self.before {
                return Err(test_error());
            }

            Ok(None)
        }

        async fn after<'a>(
            &self,
            _context: &HookContext<'a>,
            _details: &EvaluationDetails<Value>,
            _hints: Option<&'a HookHints>,
        ) -> Result<(), EvaluationError> {
            if self.after {
                return Err(test_error());
            }

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

    async fn client(provider: TestProvider) -> Client {
        let mut api = OpenFeature::default();
        api.set_provider(provider).await;

        api.create_named_client("pipeline-client")
    }

    fn test_error() -> EvaluationError {
        EvaluationError::builder()
            .code(EvaluationErrorCode::General("test failure".to_owned()))
            .build()
    }

    #[tokio::test]
    async fn pipeline_provider_failure_emits_before_then_error() {
        let (hook, events) = recording_hook();
        let client = client(TestProvider::new(ProviderBehavior::BoolFailure))
            .await
            .with_hook(hook);

        let result = client.get_bool_value("flag", None, None).await;

        assert_eq!(result, Err(test_error()));
        assert_eq!(stages(&events), ["before", "error"]);
    }

    #[tokio::test]
    async fn pipeline_earlier_before_failure_emits_error_without_before() {
        let (hook, events) = recording_hook();
        let client = client(TestProvider::new(ProviderBehavior::Success))
            .await
            .with_hook(FailingHook {
                before: true,
                after: false,
            })
            .with_hook(hook);

        let result = client.get_bool_value("flag", None, None).await;

        assert_eq!(result, Err(test_error()));
        assert_eq!(stages(&events), ["error"]);
    }

    #[tokio::test]
    async fn pipeline_later_after_failure_emits_before_after_error() {
        let (hook, events) = recording_hook();
        let client = client(TestProvider::new(ProviderBehavior::Success))
            .await
            .with_hook(FailingHook {
                before: false,
                after: true,
            })
            .with_hook(hook);

        let result = client.get_bool_value("flag", None, None).await;

        assert_eq!(result, Ok(true));
        assert_eq!(stages(&events), ["before", "after", "error"]);
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
    async fn pipeline_struct_conversion_failure_occurs_after_before_and_after() {
        let (hook, events) = recording_hook();
        let client = client(TestProvider::new(ProviderBehavior::Success))
            .await
            .with_hook(hook);

        let result = client
            .get_struct_value::<ConversionFailure>("flag", None, None)
            .await;

        assert_eq!(
            result.expect_err("conversion must fail").code,
            EvaluationErrorCode::TypeMismatch
        );
        assert_eq!(stages(&events), ["before", "after"]);
    }
}
