use std::fmt;
use std::sync::Arc;

use open_feature::{
    EvaluationContext, EvaluationDetails, EvaluationError, EvaluationErrorCode, EvaluationReason,
    Hook, HookContext, HookHints, Value, async_trait,
};

const FLAG_KEY_LIMIT: usize = 512;
const METADATA_LIMIT: usize = 256;
const VARIANT_LIMIT: usize = 256;
const REASON_LIMIT: usize = 256;
const ERROR_CODE_LIMIT: usize = 256;
const ERROR_MESSAGE_LIMIT: usize = 2_048;
const TRUNCATION_MARKER: &str = "…";
const _: () = assert!(TRUNCATION_MARKER.len() <= FLAG_KEY_LIMIT);
const _: () = assert!(TRUNCATION_MARKER.len() <= METADATA_LIMIT);
const _: () = assert!(TRUNCATION_MARKER.len() <= VARIANT_LIMIT);
const _: () = assert!(TRUNCATION_MARKER.len() <= REASON_LIMIT);
const _: () = assert!(TRUNCATION_MARKER.len() <= ERROR_CODE_LIMIT);
const _: () = assert!(TRUNCATION_MARKER.len() <= ERROR_MESSAGE_LIMIT);

/// The evaluation stage represented by a [`LoggingEvent`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LoggingStage {
    /// This hook's position in the hook chain has reached `before`.
    Before,
    /// This hook's position in the reverse hook chain has reached `after`.
    After,
    /// The `OpenFeature` client dispatched an error to this hook.
    Error,
}

/// Structured, privacy-conscious information from one hook lifecycle stage.
///
/// Unlike the TypeScript logging hook, this event intentionally omits the evaluation context,
/// default value, and resolved value because they can contain sensitive customer data. All owned
/// text is bounded and uses unambiguous escaping before the callback receives it. Variant, custom
/// reason, general error-code content, and error messages remain untrusted after sanitization.
///
/// Events describe individual hook calls, not paired evaluations. In `open-feature` 0.3, an
/// [`LoggingStage::Error`] can occur without this hook's [`LoggingStage::Before`], and
/// [`LoggingStage::After`] can be followed by [`LoggingStage::Error`] when a later `after` hook
/// fails, even though `open-feature` 0.3 still returns the provider's value to the caller.
/// Conversion from an `OpenFeature` structured value can also fail after the hook pipeline has
/// already emitted `After`, without a subsequent `Error` event.
#[non_exhaustive]
#[derive(Clone, Eq, PartialEq)]
pub struct LoggingEvent {
    stage: LoggingStage,
    flag_key: String,
    provider_name: String,
    client_name: String,
    reason: Option<EvaluationReason>,
    variant: Option<String>,
    error_code: Option<EvaluationErrorCode>,
    error_message: Option<String>,
}

impl LoggingEvent {
    /// Returns the lifecycle stage for this event.
    #[must_use]
    pub const fn stage(&self) -> LoggingStage {
        self.stage
    }

    /// Returns the bounded, escaped flag key.
    #[must_use]
    pub fn flag_key(&self) -> &str {
        &self.flag_key
    }

    /// Returns the bounded, escaped provider name.
    #[must_use]
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// Returns the bounded, escaped client name.
    #[must_use]
    pub fn client_name(&self) -> &str {
        &self.client_name
    }

    /// Returns the success reason for an `After` event.
    ///
    /// An [`EvaluationReason::Other`] payload is sanitized but remains untrusted.
    #[must_use]
    pub const fn reason(&self) -> Option<&EvaluationReason> {
        self.reason.as_ref()
    }

    /// Returns the sanitized but untrusted variant for an `After` event.
    #[must_use]
    pub fn variant(&self) -> Option<&str> {
        self.variant.as_deref()
    }

    /// Returns the error code for an `Error` event.
    ///
    /// An [`EvaluationErrorCode::General`] payload is sanitized but remains untrusted.
    #[must_use]
    pub const fn error_code(&self) -> Option<&EvaluationErrorCode> {
        self.error_code.as_ref()
    }

    /// Returns the sanitized but untrusted error message for an `Error` event.
    #[must_use]
    pub fn error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    fn before(context: &HookContext<'_>) -> Self {
        Self::common(LoggingStage::Before, context)
    }

    fn after(context: &HookContext<'_>, details: &EvaluationDetails<Value>) -> Self {
        let mut event = Self::common(LoggingStage::After, context);
        event.reason = details.reason.as_ref().map(sanitize_reason);
        event.variant = details
            .variant
            .as_deref()
            .map(|variant| sanitize(variant, VARIANT_LIMIT));

        event
    }

    fn error(context: &HookContext<'_>, error: &EvaluationError) -> Self {
        let mut event = Self::common(LoggingStage::Error, context);
        event.error_code = Some(sanitize_error_code(&error.code));
        event.error_message = error
            .message
            .as_deref()
            .map(|message| sanitize(message, ERROR_MESSAGE_LIMIT));

        event
    }

    fn common(stage: LoggingStage, context: &HookContext<'_>) -> Self {
        Self {
            stage,
            flag_key: sanitize(context.flag_key, FLAG_KEY_LIMIT),
            provider_name: sanitize(&context.provider_metadata.name, METADATA_LIMIT),
            client_name: sanitize(&context.client_metadata.name, METADATA_LIMIT),
            reason: None,
            variant: None,
            error_code: None,
            error_message: None,
        }
    }
}

impl fmt::Debug for LoggingEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("LoggingEvent");
        debug
            .field("stage", &self.stage)
            .field("flag_key", &self.flag_key)
            .field("provider_name", &self.provider_name)
            .field("client_name", &self.client_name)
            .field("reason", &self.reason)
            .field("variant", &self.variant)
            .field("error_code", &self.error_code)
            .field("error_message_present", &self.error_message.is_some())
            .field(
                "error_message_len",
                &self.error_message.as_ref().map(String::len),
            )
            .finish_non_exhaustive()
    }
}

type LoggingCallback = dyn Fn(LoggingEvent) + Send + Sync;

/// An `OpenFeature` hook that emits bounded, structured lifecycle events.
///
/// Clones share the callback, so a hook is cheap to clone and safe to use for concurrent
/// evaluations. The callback takes ownership of each event, allowing it to hand the event to a
/// queue without cloning. It runs synchronously once whenever this hook's `before`, successful
/// `after`, or `error` method is called. `finally` is deliberately a no-op.
///
/// These calls follow `open-feature` 0.3 hook-chain order, so they are not guaranteed to form
/// `Before`/`After` or `Before`/`Error` pairs. An error can originate in a provider, client, or
/// another hook.
///
/// A panic from user callback code is not caught. It propagates as a user-code panic and can skip
/// remaining error or finally hooks. Callbacks should avoid panicking and should offload expensive
/// work themselves.
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
///     eprintln!("{:?}: {}", event.stage(), event.flag_key());
/// });
/// let options = FlagshipClientOptions::builder()
///     .app_id("app-123")
///     .account_id("account-456")
///     .build()?;
/// let provider = FlagshipServerProvider::builder(options).hook(hook).build()?;
///
/// // This API value is isolated from OpenFeature's process-wide singleton.
/// let mut api = OpenFeature::default();
/// api.set_provider(provider).await;
/// let client = api.create_named_client("example");
/// let _result = client.get_bool_value("feature", None, None).await;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct LoggingHook {
    callback: Arc<LoggingCallback>,
}

impl LoggingHook {
    /// Creates a logging hook from a thread-safe callback that takes ownership of each event.
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
        self.emit(LoggingEvent::before(context));

        Ok(None)
    }

    async fn after<'a>(
        &self,
        context: &HookContext<'a>,
        details: &EvaluationDetails<Value>,
        _hints: Option<&'a HookHints>,
    ) -> Result<(), EvaluationError> {
        self.emit(LoggingEvent::after(context, details));

        Ok(())
    }

    async fn error<'a>(
        &self,
        context: &HookContext<'a>,
        error: &EvaluationError,
        _hints: Option<&'a HookHints>,
    ) {
        self.emit(LoggingEvent::error(context, error));
    }

    async fn finally<'a>(
        &self,
        _context: &HookContext<'a>,
        _evaluation_details: &EvaluationDetails<Value>,
        _hints: Option<&'a HookHints>,
    ) {
    }
}

fn sanitize_reason(reason: &EvaluationReason) -> EvaluationReason {
    match reason {
        EvaluationReason::Static => EvaluationReason::Static,
        EvaluationReason::Default => EvaluationReason::Default,
        EvaluationReason::TargetingMatch => EvaluationReason::TargetingMatch,
        EvaluationReason::Split => EvaluationReason::Split,
        EvaluationReason::Cached => EvaluationReason::Cached,
        EvaluationReason::Disabled => EvaluationReason::Disabled,
        EvaluationReason::Unknown => EvaluationReason::Unknown,
        EvaluationReason::Error => EvaluationReason::Error,
        EvaluationReason::Other(reason) => EvaluationReason::Other(sanitize(reason, REASON_LIMIT)),
    }
}

fn sanitize_error_code(code: &EvaluationErrorCode) -> EvaluationErrorCode {
    match code {
        EvaluationErrorCode::ProviderNotReady => EvaluationErrorCode::ProviderNotReady,
        EvaluationErrorCode::FlagNotFound => EvaluationErrorCode::FlagNotFound,
        EvaluationErrorCode::ParseError => EvaluationErrorCode::ParseError,
        EvaluationErrorCode::TypeMismatch => EvaluationErrorCode::TypeMismatch,
        EvaluationErrorCode::TargetingKeyMissing => EvaluationErrorCode::TargetingKeyMissing,
        EvaluationErrorCode::InvalidContext => EvaluationErrorCode::InvalidContext,
        EvaluationErrorCode::General(code) => {
            EvaluationErrorCode::General(sanitize(code, ERROR_CODE_LIMIT))
        }
    }
}

fn sanitize(input: &str, limit: usize) -> String {
    if input.len() <= limit && !input.chars().any(should_escape) {
        return input.to_owned();
    }
    if limit < TRUNCATION_MARKER.len() {
        return String::new();
    }

    let mut output = String::with_capacity(input.len().min(limit));
    let mut segment_lengths = Vec::new();
    let mut truncated = false;

    for character in input.chars() {
        let segment_length = sanitized_character_length(character);
        if output.len() + segment_length > limit {
            truncated = true;
            break;
        }

        segment_lengths.push(segment_length);
        push_sanitized_character(&mut output, character);
    }

    if truncated {
        while output.len() + TRUNCATION_MARKER.len() > limit {
            let Some(segment_length) = segment_lengths.pop() else {
                return TRUNCATION_MARKER.to_owned();
            };
            output.truncate(output.len() - segment_length);
        }
        output.push_str(TRUNCATION_MARKER);
    }

    output
}

fn should_escape(character: char) -> bool {
    if character == '\u{034f}' {
        return true;
    }

    if character == '\\' || character.is_control() {
        return true;
    }

    escape_debug_hex_after_base(character)
}

fn escape_debug_hex_after_base(character: char) -> bool {
    let mut buffer = [0_u8; 5];
    buffer[0] = b'a';
    let encoded_length = character.encode_utf8(&mut buffer[1..]).len();
    let Ok(prefixed) = str::from_utf8(&buffer[..=encoded_length]) else {
        return true;
    };
    let mut escaped = prefixed.escape_debug().skip(1);

    escaped.next() == Some('\\') && escaped.next() == Some('u')
}

fn sanitized_character_length(character: char) -> usize {
    if should_escape(character) {
        return character.escape_unicode().len();
    }

    character.len_utf8()
}

fn push_sanitized_character(output: &mut String, character: char) {
    if should_escape(character) {
        output.extend(character.escape_unicode());
    } else {
        output.push(character);
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

    use super::{
        ERROR_CODE_LIMIT, ERROR_MESSAGE_LIMIT, FLAG_KEY_LIMIT, LoggingEvent, LoggingHook,
        LoggingStage, REASON_LIMIT, sanitize,
    };

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

    fn stages(events: &Arc<Mutex<Vec<LoggingEvent>>>) -> Vec<LoggingStage> {
        events
            .lock()
            .expect("recorded logging events mutex must not be poisoned")
            .iter()
            .map(LoggingEvent::stage)
            .collect()
    }

    #[tokio::test]
    async fn direct_methods_emit_complete_stage_data_and_finally_is_noop() {
        let (hook, events) = recording_hook();
        let evaluation_context = EvaluationContext::default();
        let hook_context = context("flag", &evaluation_context);
        let details = details(EvaluationReason::TargetingMatch, "blue\u{202e}");
        let error = EvaluationError::builder()
            .code(EvaluationErrorCode::FlagNotFound)
            .message("not found")
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
        assert_eq!(events[0].stage(), LoggingStage::Before);
        assert_eq!(events[0].flag_key(), "flag");
        assert_eq!(events[0].provider_name(), "provider");
        assert_eq!(events[0].client_name(), "client");
        assert_eq!(events[1].reason(), Some(&EvaluationReason::TargetingMatch));
        assert_eq!(events[1].variant(), Some("blue\\u{202e}"));
        assert_eq!(
            events[2].error_code(),
            Some(&EvaluationErrorCode::FlagNotFound)
        );
        assert_eq!(events[2].error_message(), Some("not found"));
    }

    #[tokio::test]
    async fn custom_reason_general_code_and_message_are_independently_bounded() {
        let (hook, events) = recording_hook();
        let evaluation_context = EvaluationContext::default();
        let hook_context = context("flag", &evaluation_context);
        let reason = EvaluationReason::Other("r".repeat(REASON_LIMIT + 1));
        let details = details(reason, &"v".repeat(FLAG_KEY_LIMIT));
        let error = EvaluationError::builder()
            .code(EvaluationErrorCode::General(
                "c".repeat(ERROR_CODE_LIMIT + 1),
            ))
            .message("m".repeat(ERROR_MESSAGE_LIMIT + 1))
            .build();

        hook.after(&hook_context, &details, None)
            .await
            .expect("after");
        hook.error(&hook_context, &error, None).await;

        let events = events
            .lock()
            .expect("recorded logging events mutex must not be poisoned");
        let EvaluationReason::Other(reason) = events[0].reason().expect("reason") else {
            panic!("expected custom reason");
        };
        let EvaluationErrorCode::General(code) = events[1].error_code().expect("code") else {
            panic!("expected general code");
        };
        assert_eq!(reason.len(), REASON_LIMIT);
        assert!(reason.ends_with('…'));
        assert_eq!(
            events[0].variant().expect("variant").len(),
            super::VARIANT_LIMIT
        );
        assert_eq!(code.len(), ERROR_CODE_LIMIT);
        assert!(code.ends_with('…'));
        assert_eq!(
            events[1].error_message().expect("message").len(),
            ERROR_MESSAGE_LIMIT
        );
    }

    #[test]
    fn every_reason_variant_is_preserved_and_custom_content_is_sanitized() {
        let cases = [
            (EvaluationReason::Static, EvaluationReason::Static),
            (EvaluationReason::Default, EvaluationReason::Default),
            (
                EvaluationReason::TargetingMatch,
                EvaluationReason::TargetingMatch,
            ),
            (EvaluationReason::Split, EvaluationReason::Split),
            (EvaluationReason::Cached, EvaluationReason::Cached),
            (EvaluationReason::Disabled, EvaluationReason::Disabled),
            (EvaluationReason::Unknown, EvaluationReason::Unknown),
            (EvaluationReason::Error, EvaluationReason::Error),
            (
                EvaluationReason::Other("custom\u{202e}".to_owned()),
                EvaluationReason::Other("custom\\u{202e}".to_owned()),
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(super::sanitize_reason(&input), expected);
        }
    }

    #[test]
    fn sanitizer_escapes_ambiguous_and_invisible_characters_only() {
        let input = concat!(
            "letters \"quote\" emoji🙂 e\u{301} ... ",
            "\\\n\u{2028}\u{2029}\u{202e}\u{200b}\u{2060}\u{feff}\u{ad}\u{e0001}"
        );
        let expected = concat!(
            "letters \"quote\" emoji🙂 e\u{301} ... ",
            "\\u{5c}\\u{a}\\u{2028}\\u{2029}\\u{202e}\\u{200b}\\u{2060}",
            "\\u{feff}\\u{ad}\\u{e0001}"
        );

        assert_eq!(sanitize(input, 512), expected);
        assert_eq!(
            sanitize("visible \"🙂e\u{301}...", 64),
            "visible \"🙂e\u{301}..."
        );
    }

    #[test]
    fn sanitizer_uses_contextual_printability_for_combining_characters() {
        let input = "\u{0301}\u{20e3}\u{fe20}\u{034f}\u{1aff}\u{20ff}";
        let expected = "\u{0301}\u{20e3}\u{fe20}\\u{34f}\\u{1aff}\\u{20ff}";

        assert_eq!(sanitize(input, 128), expected);
    }

    #[test]
    fn sanitizer_honors_exact_byte_boundaries_without_partial_sequences() {
        let cases = [
            ("abcd", 4, "abcd"),
            ("abcde", 4, "a…"),
            ("...", 3, "..."),
            ("\\", 6, "\\u{5c}"),
            ("\\x", 6, "…"),
            ("\n", 5, "\\u{a}"),
            ("\n", 4, "…"),
            ("éx", 3, "éx"),
            ("éxy", 3, "…"),
            ("🙂x", 7, "🙂x"),
            ("🙂xy", 7, "🙂xy"),
            ("🙂xyza", 7, "🙂…"),
        ];

        for (input, limit, expected) in cases {
            let actual = sanitize(input, limit);
            assert_eq!(actual, expected, "input={input:?}, limit={limit}");
            assert!(actual.len() <= limit);
            assert!(actual.is_char_boundary(actual.len()));
        }
    }

    #[test]
    fn sanitizer_bounds_huge_inputs_at_the_exact_limit() {
        let clean = "x".repeat(FLAG_KEY_LIMIT + 1);
        let escaped = "\u{202e}\u{200b}\\".repeat(ERROR_MESSAGE_LIMIT);
        let clean_result = sanitize(&clean, FLAG_KEY_LIMIT);
        let escaped_result = sanitize(&escaped, ERROR_MESSAGE_LIMIT);

        assert_eq!(clean_result.len(), FLAG_KEY_LIMIT);
        assert!(clean_result.ends_with('…'));
        assert!(escaped_result.len() <= ERROR_MESSAGE_LIMIT);
        assert!(escaped_result.ends_with('…'));
        assert!(!escaped_result.contains('\u{202e}'));
        assert!(!escaped_result.contains('\u{200b}'));
        assert!(!escaped_result.ends_with("\\u"));
    }

    #[test]
    fn sanitizer_property_checks_many_limits_for_complete_escapes() {
        let corpus = concat!(
            "visible \"🙂e\u{301}...",
            "\\\n\u{2028}\u{2029}\u{202e}\u{200b}\u{2060}\u{feff}\u{ad}\u{e0001}"
        )
        .repeat(8);

        for limit in 3..=512 {
            let output = sanitize(&corpus, limit);
            assert!(output.len() <= limit, "limit={limit}, output={output:?}");
            assert_complete_unicode_escapes(&output);
        }
    }

    fn assert_complete_unicode_escapes(output: &str) {
        let mut remainder = output;

        while let Some(index) = remainder.find('\\') {
            let escape = &remainder[index..];
            assert!(
                escape.starts_with("\\u{"),
                "incomplete escape in {output:?}"
            );
            let close = escape.find('}').expect("unicode escape terminator");
            let digits = &escape[3..close];
            assert_ne!(digits, "");
            assert!(digits.bytes().all(|byte| byte.is_ascii_hexdigit()));
            remainder = &escape[close + 1..];
        }
    }

    #[test]
    fn debug_is_structured_and_omits_error_message_content() {
        let hook = LoggingHook::new(|_| {});
        let evaluation_context = EvaluationContext::default();
        let hook_context = context("flag\n", &evaluation_context);
        let error = EvaluationError::builder()
            .code(EvaluationErrorCode::General("safe\ncode".to_owned()))
            .message("secret message")
            .build();
        let event = LoggingEvent::error(&hook_context, &error);

        let hook_debug = format!("{hook:?}");
        let event_debug = format!("{event:?}");
        assert!(hook_debug.contains("callback: \"[REDACTED]\""));
        assert!(hook_debug.ends_with(".. }"));
        assert!(event_debug.contains("stage: Error"));
        assert!(event_debug.contains("flag_key: \"flag\\\\u{a}\""));
        assert!(event_debug.contains("provider_name: \"provider\""));
        assert!(event_debug.contains("General(\"safe\\\\u{a}code\")"));
        assert!(event_debug.contains("error_message_present: true"));
        assert!(event_debug.contains("error_message_len: Some(14)"));
        assert!(!event_debug.contains("secret message"));
        assert!(event_debug.ends_with(".. }"));
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
            .map(|event| event.flag_key().to_owned())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 64);
        assert_eq!(keys.first().map(String::as_str), Some("flag-0"));
        assert!(keys.iter().all(|key| key.starts_with("flag-")));
    }

    #[test]
    fn is_send_sync_hashable_stage_and_constructs_hook_wrapper() {
        fn assert_send_sync<T: Send + Sync>() {}
        fn assert_hash<T: std::hash::Hash>() {}

        assert_send_sync::<LoggingHook>();
        assert_hash::<LoggingStage>();
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
        assert_eq!(
            stages(&events),
            vec![LoggingStage::Before, LoggingStage::Error]
        );
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
        assert_eq!(stages(&events), vec![LoggingStage::Error]);
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
        assert_eq!(
            stages(&events),
            vec![
                LoggingStage::Before,
                LoggingStage::After,
                LoggingStage::Error
            ]
        );
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
        assert_eq!(
            stages(&events),
            vec![LoggingStage::Before, LoggingStage::After]
        );
    }
}
