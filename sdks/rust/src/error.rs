//! Flagship client and provider errors.

use std::error::Error;
use std::fmt;
use std::sync::Arc;

type SharedError = Arc<dyn Error + Send + Sync + 'static>;

/// A machine-readable SDK error category.
///
/// Distinct from raw backend or `OpenFeature` codes. Use [`ErrorCode::as_str`]
/// for the stable uppercase string form (e.g. `FLAG_NOT_FOUND`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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
    /// Any other failure, including HTTP statuses other than 400 and 404.
    General,
}

impl ErrorCode {
    /// Returns the stable string form of this code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FlagNotFound => "FLAG_NOT_FOUND",
            Self::BadRequest => "BAD_REQUEST",
            Self::NetworkError => "NETWORK_ERROR",
            Self::TimeoutError => "TIMEOUT_ERROR",
            Self::ParseError => "PARSE_ERROR",
            Self::InvalidContext => "INVALID_CONTEXT",
            Self::ProviderNotReady => "PROVIDER_NOT_READY",
            Self::General => "GENERAL",
        }
    }

    const fn is_retryable_by_default(self) -> bool {
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

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad(self.as_str())
    }
}

/// An error produced while communicating with or evaluating Flagship.
///
/// `Display` and `Debug` never include the source, which is available through
/// [`Error::source`].
#[derive(Clone)]
pub struct FlagshipError {
    code: ErrorCode,
    message: String,
    status: Option<u16>,
    retryable: bool,
    server_code: Option<String>,
    source: Option<SharedError>,
}

impl FlagshipError {
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
            retryable: code.is_retryable_by_default(),
            server_code: None,
            source: None,
        }
    }

    pub(crate) const fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);

        self
    }

    pub(crate) const fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;

        self
    }

    pub(crate) fn with_server_code(mut self, server_code: impl Into<String>) -> Self {
        self.server_code = Some(server_code.into());

        self
    }

    pub(crate) fn with_source(
        mut self,
        source: impl Into<Box<dyn Error + Send + Sync + 'static>>,
    ) -> Self {
        self.source = Some(Arc::from(source.into()));

        self
    }

    /// Returns the SDK error code.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the error message, which may be empty.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the HTTP status code, if present.
    #[must_use]
    pub const fn status(&self) -> Option<u16> {
        self.status
    }

    /// Returns the raw backend error code, if present.
    #[must_use]
    pub fn server_code(&self) -> Option<&str> {
        self.server_code.as_deref()
    }

    /// Returns whether the client would retry this error.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        self.retryable
    }

    pub(crate) fn from_reqwest(error: reqwest::Error) -> Self {
        let (code, message) = if error.is_timeout() {
            (ErrorCode::TimeoutError, "request timed out")
        } else {
            (ErrorCode::NetworkError, "network request failed")
        };

        Self::new(code, message).with_source(error.without_url())
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
            return fmt::Display::fmt(&self.code, formatter);
        }

        formatter.pad(&format!("{}: {}", self.code, self.message))
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
/// `Display` and `Debug` never include the source.
#[derive(Clone)]
pub struct ConfigError {
    message: String,
    source: Option<SharedError>,
}

impl ConfigError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn with_source(
        mut self,
        source: impl Into<Box<dyn Error + Send + Sync + 'static>>,
    ) -> Self {
        self.source = Some(Arc::from(source.into()));

        self
    }

    /// Returns the error message.
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

    use super::{ConfigError, ErrorCode, FlagshipError};

    #[test]
    fn codes_have_stable_strings() {
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

        for (code, expected) in cases {
            assert_eq!(code.as_str(), expected);
            assert_eq!(code.as_ref(), expected);
            assert_eq!(code.to_string(), expected);
        }
        assert_eq!(format!("{:>16}", ErrorCode::General), "         GENERAL");
    }

    #[test]
    fn retryability_defaults_by_code_and_can_be_overridden() {
        for code in [
            ErrorCode::FlagNotFound,
            ErrorCode::BadRequest,
            ErrorCode::ParseError,
            ErrorCode::InvalidContext,
        ] {
            assert!(!FlagshipError::new(code, "failure").is_retryable());
        }
        for code in [
            ErrorCode::NetworkError,
            ErrorCode::TimeoutError,
            ErrorCode::ProviderNotReady,
            ErrorCode::General,
        ] {
            assert!(FlagshipError::new(code, "failure").is_retryable());
        }

        let error = FlagshipError::new(ErrorCode::General, "unauthorized")
            .with_status(401)
            .with_retryable(false);
        assert!(!error.is_retryable());
        assert_eq!(error.status(), Some(401));
    }

    #[test]
    fn display_is_stable() {
        let error = FlagshipError::new(ErrorCode::NetworkError, "service unavailable")
            .with_server_code("BUSY");
        assert_eq!(error.message(), "service unavailable");
        assert_eq!(error.server_code(), Some("BUSY"));
        assert_eq!(error.to_string(), "NETWORK_ERROR: service unavailable");
        assert_eq!(
            FlagshipError::new(ErrorCode::ParseError, "").to_string(),
            "PARSE_ERROR"
        );
    }

    #[test]
    fn clone_preserves_source_without_formatting_it() {
        let error = FlagshipError::new(ErrorCode::NetworkError, "request failed")
            .with_source(io::Error::other("secret"));
        let cloned = error.clone();

        assert_eq!(
            cloned.source().map(ToString::to_string).as_deref(),
            Some("secret")
        );
        assert_eq!(cloned.to_string(), "NETWORK_ERROR: request failed");
        assert!(format!("{cloned:?}").contains("<redacted>"));
        assert!(!format!("{cloned:?}").contains("secret"));
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

        assert_eq!(error.code(), ErrorCode::NetworkError);
        assert_eq!(error.status(), None);
        assert!(matches!(source, Some(attached_error) if attached_error.url().is_none()));
        assert!(!error.to_string().contains("secret"));
        assert!(!format!("{error:?}").contains("secret"));
    }

    #[test]
    fn errors_without_sources_report_none() {
        assert!(
            FlagshipError::new(ErrorCode::General, "failure")
                .source()
                .is_none()
        );
        assert!(ConfigError::new("invalid endpoint").source().is_none());
    }

    #[test]
    fn config_error_formats_message_but_not_source() {
        let error =
            ConfigError::new("endpoint must use HTTPS").with_source(io::Error::other("secret"));
        let cloned = error.clone();

        assert_eq!(cloned.message(), "endpoint must use HTTPS");
        assert_eq!(
            cloned.to_string(),
            "invalid Flagship configuration: endpoint must use HTTPS"
        );
        assert_eq!(
            cloned.source().map(ToString::to_string).as_deref(),
            Some("secret")
        );
        assert!(format!("{cloned:?}").contains("<redacted>"));
        assert!(!format!("{cloned:?}").contains("secret"));
    }

    #[test]
    fn public_errors_are_send_sync_and_static() {
        fn assert_bounds<T: Send + Sync + 'static>() {}

        assert_bounds::<ErrorCode>();
        assert_bounds::<FlagshipError>();
        assert_bounds::<ConfigError>();
    }
}
