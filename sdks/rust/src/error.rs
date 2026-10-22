//! Flagship client and provider errors.

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

type SharedError = Arc<dyn Error + Send + Sync + 'static>;

/// An SDK error-code string not recognized by this SDK version.
///
/// Its contents are private so a recognized code cannot be represented as an
/// unknown code. Parse codes through [`ErrorCode`] instead. This is distinct
/// from the raw backend code exposed by [`FlagshipError::server_code`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UnknownErrorCode(String);

impl UnknownErrorCode {
    /// Returns the exact unrecognized serialized SDK error category.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for UnknownErrorCode {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for UnknownErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(self.as_str())
    }
}

/// A machine-readable SDK error category.
///
/// This categorizes client and provider failures and is distinct from a raw
/// backend or `OpenFeature` error code. Unknown serialized category strings are
/// preserved exactly for forward compatibility. This enum is non-exhaustive so
/// newer SDK versions can add recognized categories. Its variants correspond
/// to the Go SDK's error-code constants; use [`ErrorCode::as_str`] for their
/// shared uppercase string values.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ErrorCode {
    /// The requested flag was absent, including an HTTP 404 response.
    FlagNotFound,
    /// The server rejected the request with HTTP 400.
    BadRequest,
    /// A transport, connection, or response-body read failed.
    NetworkError,
    /// The request exceeded its configured timeout.
    TimeoutError,
    /// A response or flag definition could not be parsed.
    ParseError,
    /// The evaluation context could not be serialized or validated.
    InvalidContext,
    /// The provider has not finished initialization.
    ProviderNotReady,
    /// Another failure occurred, including HTTP 401, 403, and other non-400/404 errors.
    General,
    /// A serialized SDK error category that this version does not recognize.
    Unknown(UnknownErrorCode),
}

impl ErrorCode {
    /// Returns the stable, Go-compatible string representation of this error code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::FlagNotFound => "FLAG_NOT_FOUND",
            Self::BadRequest => "BAD_REQUEST",
            Self::NetworkError => "NETWORK_ERROR",
            Self::TimeoutError => "TIMEOUT_ERROR",
            Self::ParseError => "PARSE_ERROR",
            Self::InvalidContext => "INVALID_CONTEXT",
            Self::ProviderNotReady => "PROVIDER_NOT_READY",
            Self::General => "GENERAL",
            Self::Unknown(code) => code.as_str(),
        }
    }

    /// Returns the Go-compatible default retry decision.
    ///
    /// | Default | Codes |
    /// |---|---|
    /// | Not retryable | `FLAG_NOT_FOUND`, `BAD_REQUEST`, `PARSE_ERROR`, `INVALID_CONTEXT` |
    /// | Retryable | `NETWORK_ERROR`, `TIMEOUT_ERROR`, `PROVIDER_NOT_READY`, `GENERAL`, unknown codes |
    ///
    /// `GENERAL` includes HTTP 401, 403, and other 4xx responses and therefore
    /// defaults to retryable for compatibility with the Go and Python SDKs.
    #[must_use]
    pub const fn is_retryable_by_default(&self) -> bool {
        !matches!(
            self,
            Self::FlagNotFound | Self::BadRequest | Self::ParseError | Self::InvalidContext
        )
    }
}

impl AsRef<str> for ErrorCode {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for ErrorCode {
    type Err = Infallible;

    fn from_str(code: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(code.to_owned()))
    }
}

impl From<String> for ErrorCode {
    fn from(code: String) -> Self {
        match code.as_str() {
            "FLAG_NOT_FOUND" => Self::FlagNotFound,
            "BAD_REQUEST" => Self::BadRequest,
            "NETWORK_ERROR" => Self::NetworkError,
            "TIMEOUT_ERROR" => Self::TimeoutError,
            "PARSE_ERROR" => Self::ParseError,
            "INVALID_CONTEXT" => Self::InvalidContext,
            "PROVIDER_NOT_READY" => Self::ProviderNotReady,
            "GENERAL" => Self::General,
            _ => Self::Unknown(UnknownErrorCode(code)),
        }
    }
}

impl From<&str> for ErrorCode {
    fn from(code: &str) -> Self {
        Self::from(code.to_owned())
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from)
    }
}

/// An error produced while communicating with or evaluating Flagship.
///
/// Formatting intentionally excludes the source. Messages supplied by callers
/// must summarize the failure and must not copy source errors, URLs, headers,
/// credentials, tokens, or other request data. Sources are not automatically
/// sanitized by [`FlagshipError::with_source`]; callers must sanitize them
/// before attachment. The `reqwest` conversion used inside this crate removes
/// attached URLs. Sources remain available through [`Error::source`].
#[derive(Clone)]
pub struct FlagshipError {
    code: ErrorCode,
    message: String,
    status: Option<u16>,
    retryable: Option<bool>,
    server_code: Option<String>,
    source: Option<SharedError>,
}

impl FlagshipError {
    /// Creates an error using the code's default retryability.
    ///
    /// `message` must not contain source-error text, URLs, headers, credentials,
    /// tokens, or other request data.
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
            retryable: None,
            server_code: None,
            source: None,
        }
    }

    /// Sets the HTTP status when it is in `100..=999`.
    ///
    /// Zero and values outside that range clear a previously assigned status.
    #[must_use]
    pub const fn with_status(mut self, status: u16) -> Self {
        self.status = if status >= 100 && status <= 999 {
            Some(status)
        } else {
            None
        };

        self
    }

    /// Overrides the retry decision derived from the error code.
    #[must_use]
    pub const fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = Some(retryable);

        self
    }

    /// Clears an explicit retry override and restores the code-derived default.
    #[must_use]
    pub const fn with_default_retryability(mut self) -> Self {
        self.retryable = None;

        self
    }

    /// Sets the raw backend or `OpenFeature` error code used for provider mapping.
    ///
    /// An empty string clears a previously assigned server code. Non-empty
    /// strings are preserved exactly and are not interpreted as [`ErrorCode`]
    /// values. The value is untrusted backend data; callers must sanitize it
    /// before attaching it if it can contain credentials, tokens, URLs, or
    /// other sensitive data.
    #[must_use]
    pub fn with_server_code(mut self, server_code: impl Into<String>) -> Self {
        let server_code = server_code.into();
        self.server_code = (!server_code.is_empty()).then_some(server_code);

        self
    }

    /// Attaches or replaces a lower-level source error.
    ///
    /// This method does not sanitize the source. The caller must first remove
    /// URLs, credentials, tokens, headers, and other sensitive request data.
    #[must_use]
    pub fn with_source(
        mut self,
        source: impl Into<Box<dyn Error + Send + Sync + 'static>>,
    ) -> Self {
        self.source = Some(Arc::from(source.into()));

        self
    }

    /// Returns the machine-readable SDK error code.
    #[must_use]
    pub const fn code(&self) -> &ErrorCode {
        &self.code
    }

    /// Returns the message exactly as supplied to [`Self::new`].
    ///
    /// This may be empty; use [`Self::resolution_message`] when a non-empty SDK
    /// category fallback is appropriate.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the bare message suitable for an `OpenFeature` resolution detail.
    ///
    /// An empty message falls back to the SDK error code string.
    #[must_use]
    pub fn resolution_message(&self) -> &str {
        if self.message.is_empty() {
            return self.code.as_str();
        }

        &self.message
    }

    /// Returns the validated HTTP status code, if present.
    #[must_use]
    pub const fn status(&self) -> Option<u16> {
        self.status
    }

    /// Returns the uninterpreted backend or `OpenFeature` error code, if present.
    ///
    /// This value is separate from the SDK category returned by [`Self::code`].
    #[must_use]
    pub fn server_code(&self) -> Option<&str> {
        self.server_code.as_deref()
    }

    /// Returns the explicit retry override, if one was supplied.
    #[must_use]
    pub const fn retryable_override(&self) -> Option<bool> {
        self.retryable
    }

    /// Returns the explicit retry override or the Go-compatible code default.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        match self.retryable {
            Some(retryable) => retryable,
            None => self.code.is_retryable_by_default(),
        }
    }

    pub(crate) fn from_reqwest(error: reqwest::Error) -> Self {
        let is_timeout = error.is_timeout();
        let has_json_source = error
            .source()
            .is_some_and(<dyn Error>::is::<serde_json::Error>);
        let is_builder = error.is_builder();
        let status = error.status().map(|status| status.as_u16());
        let code = if is_timeout {
            ErrorCode::TimeoutError
        } else if has_json_source && is_builder {
            ErrorCode::InvalidContext
        } else if has_json_source {
            ErrorCode::ParseError
        } else if status == Some(404) {
            ErrorCode::FlagNotFound
        } else if status == Some(400) {
            ErrorCode::BadRequest
        } else if status.is_some() {
            ErrorCode::General
        } else {
            ErrorCode::NetworkError
        };
        let message = match &code {
            ErrorCode::TimeoutError => "request timed out",
            ErrorCode::InvalidContext => "request JSON could not be serialized",
            ErrorCode::ParseError => "response JSON could not be parsed",
            ErrorCode::FlagNotFound | ErrorCode::BadRequest | ErrorCode::General => {
                "HTTP request failed"
            }
            _ => "network request failed",
        };

        let mut flagship_error = Self::new(code, message).with_source(error.without_url());

        if let Some(status) = status {
            flagship_error = flagship_error.with_status(status);
        }

        flagship_error
    }
}

impl fmt::Debug for FlagshipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlagshipError")
            .field("code", &self.code)
            .field("message", &self.message)
            .field("status", &self.status)
            .field("retryable", &self.retryable)
            .field("server_code", &self.server_code)
            .field("source", &self.source.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl fmt::Display for FlagshipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            if self.code.as_str().is_empty() {
                return formatter.pad("Flagship error");
            }

            return fmt::Display::fmt(&self.code, formatter);
        }

        if self.code.as_str().is_empty() {
            return formatter.pad(&self.message);
        }

        formatter.pad(&format!("{}: {}", self.code.as_str(), self.message))
    }
}

impl Error for FlagshipError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

/// An error caused by invalid SDK configuration.
///
/// Configuration failures are local construction errors and are never modeled
/// as retryable remote `GENERAL` failures. Messages supplied by callers must
/// not copy source errors, URLs, headers, credentials, or tokens. Sources are
/// not automatically sanitized by [`ConfigError::with_source`]; callers must
/// sanitize them before attachment.
#[derive(Clone)]
pub struct ConfigError {
    message: String,
    source: Option<SharedError>,
}

impl ConfigError {
    /// Creates a configuration error without a source.
    ///
    /// `message` must not contain source-error text, URLs, headers, credentials,
    /// tokens, or other request data.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Attaches or replaces a lower-level source error.
    ///
    /// This method does not sanitize the source. The caller must first remove
    /// URLs, credentials, tokens, headers, and other sensitive request data.
    #[must_use]
    pub fn with_source(
        mut self,
        source: impl Into<Box<dyn Error + Send + Sync + 'static>>,
    ) -> Self {
        self.source = Some(Arc::from(source.into()));

        self
    }

    /// Returns the message exactly as supplied to [`Self::new`].
    ///
    /// This may be empty even though [`fmt::Display`] always produces a
    /// non-empty configuration error description.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Debug for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigError")
            .field("message", &self.message)
            .field("source", &self.source.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.message.is_empty() {
            return formatter.pad("invalid Flagship configuration");
        }

        formatter.pad(&format!("invalid Flagship configuration: {}", self.message))
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io;
    use std::str::FromStr;

    use super::{ConfigError, ErrorCode, FlagshipError};

    #[test]
    fn known_codes_have_stable_strings_and_normalize() {
        let cases = [
            (ErrorCode::FlagNotFound, "FLAG_NOT_FOUND"),
            (ErrorCode::BadRequest, "BAD_REQUEST"),
            (ErrorCode::NetworkError, "NETWORK_ERROR"),
            (ErrorCode::TimeoutError, "TIMEOUT_ERROR"),
            (ErrorCode::ParseError, "PARSE_ERROR"),
            (ErrorCode::InvalidContext, "INVALID_CONTEXT"),
            (ErrorCode::ProviderNotReady, "PROVIDER_NOT_READY"),
            (ErrorCode::General, "GENERAL"),
        ];

        for (expected, wire) in cases {
            let parsed = ErrorCode::from_str(wire);
            assert_eq!(parsed, Ok(expected.clone()));
            assert_eq!(expected.as_str(), wire);
            assert_eq!(expected.as_ref(), wire);
            assert_eq!(expected.to_string(), wire);

            let serialized = serde_json::to_string(&expected);
            assert!(serialized.is_ok_and(|json| json == format!("\"{wire}\"")));
        }
    }

    #[test]
    fn codes_round_trip_and_unknown_strings_remain_exact() {
        let unknown_json = r#""SERVER_\"TOO_BUSY""#;
        let decoded = serde_json::from_str::<ErrorCode>(unknown_json);
        assert!(matches!(
            decoded,
            Ok(ErrorCode::Unknown(ref code)) if code.as_str() == "SERVER_\"TOO_BUSY"
        ));

        let code = ErrorCode::from("SERVER_\"TOO_BUSY");
        let serialized = serde_json::to_string(&code);
        assert!(matches!(serialized, Ok(ref json) if json == unknown_json));

        let known = serde_json::from_str::<ErrorCode>("\"GENERAL\"");
        assert!(known.is_ok_and(|code| code == ErrorCode::General));

        let lowercase = ErrorCode::from("general");
        assert!(matches!(lowercase, ErrorCode::Unknown(ref code) if code.as_str() == "general"));
    }

    #[test]
    fn serde_rejects_non_strings_and_supports_null_option() {
        assert!(serde_json::from_str::<ErrorCode>("42").is_err());
        assert!(serde_json::from_str::<ErrorCode>("null").is_err());
        assert!(serde_json::from_str::<ErrorCode>("{}").is_err());

        let optional = serde_json::from_str::<Option<ErrorCode>>("null");
        assert!(optional.is_ok_and(|value| value.is_none()));
    }

    #[test]
    fn retry_defaults_match_go_and_overrides_are_replaceable() {
        let non_retryable = [
            ErrorCode::FlagNotFound,
            ErrorCode::BadRequest,
            ErrorCode::ParseError,
            ErrorCode::InvalidContext,
        ];
        let retryable = [
            ErrorCode::NetworkError,
            ErrorCode::TimeoutError,
            ErrorCode::ProviderNotReady,
            ErrorCode::General,
            ErrorCode::from("FUTURE_CODE"),
        ];

        for code in non_retryable {
            assert!(!FlagshipError::new(code, "failure").is_retryable());
        }
        for code in retryable {
            assert!(FlagshipError::new(code, "failure").is_retryable());
        }

        let error = FlagshipError::new(ErrorCode::General, "unauthorized")
            .with_status(401)
            .with_retryable(false)
            .with_retryable(true);
        assert!(error.is_retryable());
        assert_eq!(error.retryable_override(), Some(true));

        let defaulted = error.with_default_retryability();
        assert!(defaulted.is_retryable());
        assert_eq!(defaulted.retryable_override(), None);
    }

    #[test]
    fn status_and_server_code_validate_and_overwrite() {
        let error = FlagshipError::new(ErrorCode::General, "failure")
            .with_status(100)
            .with_status(999)
            .with_server_code("BACKEND_CODE")
            .with_server_code("OPEN_FEATURE_CODE");
        assert_eq!(error.status(), Some(999));
        assert_eq!(error.server_code(), Some("OPEN_FEATURE_CODE"));

        let cleared = error.with_status(1_000).with_server_code("");
        assert_eq!(cleared.status(), None);
        assert_eq!(cleared.server_code(), None);
        assert_eq!(cleared.clone().with_status(0).status(), None);
        assert_eq!(cleared.with_status(99).status(), None);
    }

    #[test]
    fn display_and_resolution_message_are_stable() {
        let error = FlagshipError::new(ErrorCode::NetworkError, "service unavailable");
        assert_eq!(error.message(), "service unavailable");
        assert_eq!(error.resolution_message(), "service unavailable");
        assert_eq!(error.to_string(), "NETWORK_ERROR: service unavailable");

        let empty_unknown = FlagshipError::new(ErrorCode::from(""), "");
        assert_eq!(empty_unknown.to_string(), "Flagship error");
        assert_eq!(empty_unknown.resolution_message(), "");

        let empty_known = FlagshipError::new(ErrorCode::ParseError, "");
        assert_eq!(empty_known.resolution_message(), "PARSE_ERROR");
        assert_eq!(format!("{:>16}", ErrorCode::General), "         GENERAL");
    }

    #[test]
    fn clone_and_source_overwrite_preserve_safe_formatting() {
        let boxed: Box<dyn Error + Send + Sync> = Box::new(io::Error::other("first secret"));
        let error = FlagshipError::new(ErrorCode::NetworkError, "request failed")
            .with_source(boxed)
            .with_source(io::Error::other("second secret"));
        let cloned = error.clone();

        assert_eq!(
            cloned.source().map(ToString::to_string).as_deref(),
            Some("second secret")
        );
        assert_eq!(cloned.to_string(), "NETWORK_ERROR: request failed");
        assert_eq!(format!("{cloned:?}").matches("<redacted>").count(), 1);
        assert!(!format!("{cloned:?}").contains("second secret"));
    }

    #[test]
    fn transport_conversion_strips_attached_urls() {
        let Err(proxy_error) = reqwest::Proxy::all("://invalid-proxy") else {
            panic!("invalid proxy URL unexpectedly succeeded");
        };
        let Ok(url) = reqwest::Url::parse("https://example.com/?token=secret") else {
            panic!("static test URL failed to parse");
        };
        let error = FlagshipError::from_reqwest(proxy_error.with_url(url));
        let source = error
            .source()
            .and_then(|source| source.downcast_ref::<reqwest::Error>());

        assert_eq!(error.code(), &ErrorCode::NetworkError);
        assert_eq!(error.status(), None);
        assert!(matches!(source, Some(attached_error) if attached_error.url().is_none()));
        assert!(!error.to_string().contains("secret"));
        assert!(!format!("{error:?}").contains("secret"));
    }

    #[test]
    fn errors_without_sources_report_none() {
        let flagship = FlagshipError::new(ErrorCode::General, "failure");
        let config = ConfigError::new("invalid endpoint");

        assert!(flagship.source().is_none());
        assert!(config.source().is_none());
    }

    #[test]
    fn config_error_is_cloneable_distinct_and_chainable() {
        let boxed: Box<dyn Error + Send + Sync> = Box::new(io::Error::other("first secret"));
        let error = ConfigError::new("endpoint must use HTTPS")
            .with_source(boxed)
            .with_source(io::Error::other("second secret"));
        let cloned = error.clone();

        assert_eq!(cloned.message(), "endpoint must use HTTPS");
        assert_eq!(
            cloned.to_string(),
            "invalid Flagship configuration: endpoint must use HTTPS"
        );
        assert_eq!(
            cloned.source().map(ToString::to_string).as_deref(),
            Some("second secret")
        );
        assert!(format!("{cloned:?}").contains("<redacted>"));
        assert!(!format!("{cloned:?}").contains("second secret"));

        assert_eq!(
            ConfigError::new("").to_string(),
            "invalid Flagship configuration"
        );
    }

    #[test]
    fn public_errors_are_send_sync_and_static() {
        fn assert_bounds<T: Send + Sync + 'static>() {}

        assert_bounds::<ErrorCode>();
        assert_bounds::<FlagshipError>();
        assert_bounds::<ConfigError>();
    }
}
