//! Validated configuration for the Flagship HTTP client.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

pub use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client, Url};

use crate::endpoint::resolve_evaluation_endpoint;
use crate::error::ConfigError;

const USER_AGENT: &str = concat!("cloudflare-flagship-rust/", env!("CARGO_PKG_VERSION"));

/// Default base URL for the Flagship API.
pub const FLAGSHIP_DEFAULT_BASE_URL: &str = "https://api.cloudflare.com";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_RETRIES: u32 = 1;
pub(crate) const MAX_RETRIES: u32 = 10;
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(1);
pub(crate) const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Error returned by a [`FlagshipClientOptionsBuilder::header_factory`] future.
pub type HeaderFactoryError = Box<dyn Error + Send + Sync + 'static>;

type HeaderFuture =
    Pin<Box<dyn Future<Output = Result<HeaderMap, HeaderFactoryError>> + Send + 'static>>;

#[derive(Clone)]
pub(crate) struct HeaderFactory {
    inner: Arc<dyn Fn() -> HeaderFuture + Send + Sync + 'static>,
}

impl HeaderFactory {
    fn new<F, Fut>(factory: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<HeaderMap, HeaderFactoryError>> + Send + 'static,
    {
        Self {
            inner: Arc::new(move || Box::pin(factory())),
        }
    }

    pub(crate) fn headers(&self) -> HeaderFuture {
        (self.inner)()
    }
}

impl fmt::Debug for HeaderFactory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.pad("HeaderFactory(<configured>)")
    }
}

/// Request timeout behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TimeoutPolicy {
    /// Limit each complete attempt, including the header factory and HTTP I/O,
    /// to this non-zero duration.
    Duration(Duration),
    /// No SDK deadline. Caller cancellation and an injected [`Client`]'s own
    /// timeouts still apply.
    Disabled,
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self::Duration(DEFAULT_TIMEOUT)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct AppEndpoint {
    app_id: String,
    account_id: String,
    base_url: Url,
}

impl AppEndpoint {
    pub(crate) fn app_id(&self) -> &str {
        &self.app_id
    }

    pub(crate) fn account_id(&self) -> &str {
        &self.account_id
    }

    pub(crate) fn base_url(&self) -> &Url {
        &self.base_url
    }
}

impl fmt::Debug for AppEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppEndpoint")
            .field("app_id", &"<configured>")
            .field("account_id", &"<configured>")
            .field("base_url", &"<configured>")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ExplicitEndpoint {
    endpoint: Url,
    account_id: Option<String>,
}

impl ExplicitEndpoint {
    pub(crate) fn endpoint(&self) -> &Url {
        &self.endpoint
    }
}

impl fmt::Debug for ExplicitEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExplicitEndpoint")
            .field("endpoint", &"<configured>")
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "<configured>"),
            )
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FlagshipEndpoint {
    App(AppEndpoint),
    Explicit(ExplicitEndpoint),
}

impl FlagshipEndpoint {
    pub(crate) fn account_id(&self) -> Option<&str> {
        match self {
            Self::App(endpoint) => Some(&endpoint.account_id),
            Self::Explicit(endpoint) => endpoint.account_id.as_deref(),
        }
    }
}

/// Validated options for a Flagship HTTP client.
///
/// Build with [`FlagshipClientOptions::builder`]; construction does not need a
/// Tokio runtime, but executing requests does. `Debug` output redacts
/// identifiers, tokens, header values, and URLs.
#[derive(Clone)]
pub struct FlagshipClientOptions {
    endpoint: FlagshipEndpoint,
    evaluation_url: Url,
    bearer_header: Option<HeaderValue>,
    static_headers: HeaderMap,
    header_factory: Option<HeaderFactory>,
    http_client: Client,
    timeout: TimeoutPolicy,
    retries: u32,
    retry_delay: Duration,
}

impl FlagshipClientOptions {
    /// Starts a client-options builder.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cloudflare_flagship::config::FlagshipClientOptions;
    ///
    /// let options = FlagshipClientOptions::builder()
    ///     .app_id("app-123")
    ///     .account_id("account-456")
    ///     .auth_token("secret-token")
    ///     .build()?;
    /// # Ok::<(), cloudflare_flagship::error::ConfigError>(())
    /// ```
    #[must_use]
    pub fn builder() -> FlagshipClientOptionsBuilder {
        FlagshipClientOptionsBuilder::default()
    }

    /// Returns the configured account ID, if any.
    #[must_use]
    pub fn account_id(&self) -> Option<&str> {
        self.endpoint.account_id()
    }

    pub(crate) fn endpoint(&self) -> &FlagshipEndpoint {
        &self.endpoint
    }

    pub(crate) fn evaluation_url(&self) -> &Url {
        &self.evaluation_url
    }

    /// Returns the request timeout policy.
    #[must_use]
    pub fn timeout(&self) -> TimeoutPolicy {
        self.timeout
    }

    /// Returns the number of retry attempts (capped at 10).
    #[must_use]
    pub fn retries(&self) -> u32 {
        self.retries
    }

    /// Returns the fixed delay between retry attempts (capped at 30 seconds).
    #[must_use]
    pub fn retry_delay(&self) -> Duration {
        self.retry_delay
    }

    pub(crate) fn bearer_header(&self) -> Option<&HeaderValue> {
        self.bearer_header.as_ref()
    }

    pub(crate) fn static_headers(&self) -> &HeaderMap {
        &self.static_headers
    }

    pub(crate) fn header_factory(&self) -> Option<&HeaderFactory> {
        self.header_factory.as_ref()
    }

    pub(crate) fn http_client(&self) -> &Client {
        &self.http_client
    }
}

impl fmt::Debug for FlagshipClientOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlagshipClientOptions")
            .field("endpoint", &self.endpoint)
            .field("evaluation_url", &"<configured>")
            .field(
                "bearer_header",
                &self.bearer_header.as_ref().map(|_| "<redacted>"),
            )
            .field("static_header_count", &self.static_headers.len())
            .field("header_factory", &self.header_factory)
            .field("http_client", &"<configured>")
            .field("timeout", &self.timeout)
            .field("retries", &self.retries)
            .field("retry_delay", &self.retry_delay)
            .finish()
    }
}

/// Builder for [`FlagshipClientOptions`].
///
/// Supply either `app_id` plus `account_id`, or `endpoint`. `base_url` only
/// applies to application mode. For [`Self::timeout`] and
/// [`Self::disable_timeout`], the last call wins.
#[derive(Clone, Default)]
pub struct FlagshipClientOptionsBuilder {
    app_id: Option<String>,
    account_id: Option<String>,
    endpoint: Option<String>,
    base_url: Option<String>,
    auth_token: Option<String>,
    static_headers: HeaderMap,
    header_factory: Option<HeaderFactory>,
    http_client: Option<Client>,
    timeout: Option<TimeoutPolicy>,
    retries: Option<u32>,
    retry_delay: Option<Duration>,
}

impl FlagshipClientOptionsBuilder {
    /// Sets the application ID.
    ///
    /// Rejects empty values, surrounding whitespace, control characters, and
    /// `.`/`..` path segments. Interior spaces and slashes are preserved.
    #[must_use]
    pub fn app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = Some(app_id.into());

        self
    }

    /// Sets the account ID; required in application mode, optional with
    /// [`Self::endpoint`] (used for local evaluation).
    ///
    /// Validated like [`Self::app_id`].
    #[must_use]
    pub fn account_id(mut self, account_id: impl Into<String>) -> Self {
        self.account_id = Some(account_id.into());

        self
    }

    /// Sets an explicit absolute evaluation endpoint.
    ///
    /// Must be HTTP(S) with a valid host and no credentials, query, fragment,
    /// or control characters.
    #[must_use]
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());

        self
    }

    /// Sets the API base URL for application mode (default
    /// [`FLAGSHIP_DEFAULT_BASE_URL`]).
    ///
    /// Validated like [`Self::endpoint`]; trailing slashes are ignored.
    #[must_use]
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());

        self
    }

    /// Sets the bearer token, unless overridden by a static or dynamic
    /// `Authorization` header.
    ///
    /// An empty token is treated as absent; otherwise it must be visible ASCII
    /// without whitespace.
    #[must_use]
    pub fn auth_token(mut self, auth_token: impl Into<String>) -> Self {
        self.auth_token = Some(auth_token.into());

        self
    }

    /// Replaces the static headers sent on every attempt.
    ///
    /// Values are marked sensitive. A static `Authorization` header replaces
    /// the bearer token; dynamic headers replace static ones with the same name.
    #[must_use]
    pub fn static_headers(mut self, headers: HeaderMap) -> Self {
        self.static_headers = headers;

        self
    }

    /// Sets an async source of dynamic headers, called once per attempt
    /// (including retries), e.g. to refresh short-lived credentials.
    ///
    /// The future runs inside the attempt timeout and must be cancellation
    /// safe. Failures become retryable `GENERAL` errors. Values are marked
    /// sensitive. Dynamic headers override static headers, which override
    /// `auth_token`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cloudflare_flagship::config::{FlagshipClientOptions, HeaderMap};
    ///
    /// let options = FlagshipClientOptions::builder()
    ///     .endpoint("https://example.com/evaluate")
    ///     .header_factory(|| async { Ok(HeaderMap::new()) })
    ///     .build()?;
    /// # Ok::<(), cloudflare_flagship::error::ConfigError>(())
    /// ```
    #[must_use]
    pub fn header_factory<F, Fut>(mut self, factory: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<HeaderMap, HeaderFactoryError>> + Send + 'static,
    {
        self.header_factory = Some(HeaderFactory::new(factory));

        self
    }

    /// Injects the Reqwest client used for HTTP requests.
    ///
    /// Its transport settings and timeouts stay active; the SDK timeout is
    /// applied as an outer deadline. SDK headers override its default headers.
    /// The default client does not follow redirects.
    #[must_use]
    pub fn http_client(mut self, client: Client) -> Self {
        self.http_client = Some(client);

        self
    }

    /// Sets a non-zero timeout for each complete attempt (default 5 seconds).
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(TimeoutPolicy::Duration(timeout));

        self
    }

    /// Disables the SDK timeout. Injected-client timeouts still apply.
    #[must_use]
    pub fn disable_timeout(mut self) -> Self {
        self.timeout = Some(TimeoutPolicy::Disabled);

        self
    }

    /// Sets retry attempts: default 1, capped at 10, `0` disables retries.
    #[must_use]
    pub fn retries(mut self, retries: u32) -> Self {
        self.retries = Some(retries);

        self
    }

    /// Sets the fixed retry delay: default 1 second, capped at 30 seconds,
    /// zero allowed.
    #[must_use]
    pub fn retry_delay(mut self, retry_delay: Duration) -> Self {
        self.retry_delay = Some(retry_delay);

        self
    }

    /// Validates and constructs client options.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when no endpoint mode is selected, modes
    /// conflict, an identifier, URL, token, or timeout is invalid, or the
    /// default Reqwest client cannot be built.
    ///
    /// # Panics
    ///
    /// With only the `rustls-no-provider` feature, Reqwest may panic if no
    /// Rustls crypto provider is installed. Install one first or inject a
    /// client with [`Self::http_client`].
    pub fn build(self) -> Result<FlagshipClientOptions, ConfigError> {
        let endpoint = self.build_endpoint()?;
        let evaluation_url = resolve_evaluation_endpoint(&endpoint);
        let timeout = self.timeout.unwrap_or_default();
        if matches!(timeout, TimeoutPolicy::Duration(duration) if duration.is_zero()) {
            return Err(ConfigError::new(
                "timeout must be greater than zero or disabled",
            ));
        }

        let bearer_header = build_bearer_header(self.auth_token.as_deref())?;
        let mut static_headers = self.static_headers;
        mark_sensitive(&mut static_headers);

        let http_client = match self.http_client {
            Some(client) => client,
            None => Client::builder()
                .user_agent(USER_AGENT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| {
                    ConfigError::new("default HTTP client could not be constructed")
                        .with_source(error.without_url())
                })?,
        };

        Ok(FlagshipClientOptions {
            endpoint,
            evaluation_url,
            bearer_header,
            static_headers,
            header_factory: self.header_factory,
            http_client,
            timeout,
            retries: self.retries.unwrap_or(DEFAULT_RETRIES).min(MAX_RETRIES),
            retry_delay: self
                .retry_delay
                .unwrap_or(DEFAULT_RETRY_DELAY)
                .min(MAX_RETRY_DELAY),
        })
    }

    pub(crate) fn build_endpoint(&self) -> Result<FlagshipEndpoint, ConfigError> {
        if self.endpoint.is_some() && self.app_id.is_some() {
            return Err(ConfigError::new(
                "endpoint and app_id are mutually exclusive",
            ));
        }
        if self.endpoint.is_some() && self.base_url.is_some() {
            return Err(ConfigError::new("base_url cannot be used with endpoint"));
        }

        if let Some(endpoint) = &self.endpoint {
            let endpoint = parse_http_url(endpoint, "endpoint")?;
            let account_id = validate_optional_id(self.account_id.as_deref(), "account_id")?;

            return Ok(FlagshipEndpoint::Explicit(ExplicitEndpoint {
                endpoint,
                account_id,
            }));
        }

        if self.app_id.is_none() && self.account_id.is_none() && self.base_url.is_none() {
            return Err(ConfigError::new(
                "either app_id with account_id or endpoint is required",
            ));
        }

        let app_id = validate_required_id(self.app_id.as_deref(), "app_id")?;
        let account_id = validate_required_id(self.account_id.as_deref(), "account_id")?;
        let base_url = self
            .base_url
            .as_deref()
            .unwrap_or(FLAGSHIP_DEFAULT_BASE_URL);
        let base_url = parse_http_url(base_url, "base_url")?;

        Ok(FlagshipEndpoint::App(AppEndpoint {
            app_id,
            account_id,
            base_url,
        }))
    }
}

impl fmt::Debug for FlagshipClientOptionsBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlagshipClientOptionsBuilder")
            .field("app_id", &self.app_id.as_ref().map(|_| "<configured>"))
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "<configured>"),
            )
            .field("endpoint", &self.endpoint.as_ref().map(|_| "<configured>"))
            .field("base_url", &self.base_url.as_ref().map(|_| "<configured>"))
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "<redacted>"),
            )
            .field("static_header_count", &self.static_headers.len())
            .field("header_factory", &self.header_factory)
            .field(
                "http_client",
                &self.http_client.as_ref().map(|_| "<configured>"),
            )
            .field("timeout", &self.timeout)
            .field("retries", &self.retries)
            .field("retry_delay", &self.retry_delay)
            .finish()
    }
}

fn validate_required_id(value: Option<&str>, name: &str) -> Result<String, ConfigError> {
    let Some(value) = value else {
        return Err(ConfigError::new(format!("{name} is required")));
    };

    validate_id(value, name).map(str::to_owned)
}

fn validate_optional_id(value: Option<&str>, name: &str) -> Result<Option<String>, ConfigError> {
    value
        .map(|value| validate_id(value, name).map(str::to_owned))
        .transpose()
}

fn validate_id<'a>(value: &'a str, name: &str) -> Result<&'a str, ConfigError> {
    if value.is_empty() {
        return Err(ConfigError::new(format!("{name} must not be empty")));
    }
    if value.trim() != value {
        return Err(ConfigError::new(format!(
            "{name} must not have leading or trailing whitespace"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(ConfigError::new(format!(
            "{name} must not contain control characters"
        )));
    }
    if value
        .split('/')
        .any(|segment| matches!(segment, "." | ".."))
    {
        return Err(ConfigError::new(format!(
            "{name} must not contain dot path segments"
        )));
    }

    Ok(value)
}

fn build_bearer_header(token: Option<&str>) -> Result<Option<HeaderValue>, ConfigError> {
    let Some(token) = token.filter(|token| !token.is_empty()) else {
        return Ok(None);
    };

    if !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(ConfigError::new(
            "auth_token must contain only visible ASCII characters without whitespace",
        ));
    }

    let mut value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
        ConfigError::new("auth_token could not be encoded as an Authorization header")
    })?;
    value.set_sensitive(true);

    Ok(Some(value))
}

fn mark_sensitive(headers: &mut HeaderMap) {
    for value in headers.values_mut() {
        value.set_sensitive(true);
    }
}

fn parse_http_url(value: &str, name: &str) -> Result<Url, ConfigError> {
    if value.is_empty() {
        return Err(ConfigError::new(format!("{name} must not be empty")));
    }
    if value.chars().any(char::is_control) {
        return Err(ConfigError::new(format!(
            "{name} must not contain control characters"
        )));
    }

    let url = Url::parse(value).map_err(|error| {
        ConfigError::new(format!("{name} must be an absolute HTTP(S) URL")).with_source(error)
    })?;
    if !matches!(url.scheme(), "http" | "https") || !url.has_host() {
        return Err(ConfigError::new(format!(
            "{name} must be an absolute HTTP(S) URL"
        )));
    }
    if has_obviously_invalid_host(&url) {
        return Err(ConfigError::new(format!("{name} contains an invalid host")));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConfigError::new(format!(
            "{name} must not contain credentials"
        )));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(ConfigError::new(format!(
            "{name} must not contain a query or fragment"
        )));
    }

    Ok(url)
}

fn has_obviously_invalid_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    let lowercase_host = host.to_ascii_lowercase();

    host.chars()
        .any(|character| character.is_control() || matches!(character, '{' | '}' | '"' | '`'))
        || lowercase_host.contains("%7b")
        || lowercase_host.contains("%7d")
        || lowercase_host.contains("%22")
        || lowercase_host.contains("%60")
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn app_builder() -> FlagshipClientOptionsBuilder {
        FlagshipClientOptions::builder()
            .app_id("app-123")
            .account_id("account-456")
    }

    fn assert_message<T>(result: Result<T, ConfigError>, expected: &str) {
        let error = result.err().expect("configuration should fail");

        assert_eq!(error.message(), expected);
    }

    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    fn build_valid(builder: FlagshipClientOptionsBuilder) -> FlagshipClientOptions {
        builder.build().expect("valid options")
    }

    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[test]
    fn defaults_match_other_sdks() {
        let options = build_valid(app_builder());
        let FlagshipEndpoint::App(endpoint) = options.endpoint() else {
            panic!("expected app endpoint");
        };

        assert_eq!(endpoint.app_id(), "app-123");
        assert_eq!(options.account_id(), Some("account-456"));
        assert_eq!(endpoint.base_url().as_str(), "https://api.cloudflare.com/");
        assert_eq!(
            options.evaluation_url().as_str(),
            "https://api.cloudflare.com/client/v4/accounts/account-456/flagship/apps/app-123/evaluate"
        );
        assert_eq!(
            options.timeout(),
            TimeoutPolicy::Duration(Duration::from_secs(5))
        );
        assert_eq!(options.retries(), 1);
        assert_eq!(options.retry_delay(), Duration::from_secs(1));
        assert!(options.bearer_header().is_none());
        assert!(options.static_headers().is_empty());
        assert!(options.header_factory().is_none());
        let _ = options.http_client();
    }

    #[test]
    fn explicit_endpoint_supports_optional_account_id() {
        let without_account = FlagshipClientOptions::builder()
            .endpoint("http://localhost:8787/evaluate")
            .build_endpoint()
            .expect("valid explicit endpoint");
        let with_account = FlagshipClientOptions::builder()
            .endpoint("https://example.com/evaluate")
            .account_id("account-for-local-mode")
            .build_endpoint()
            .expect("valid explicit endpoint with account");

        assert_eq!(without_account.account_id(), None);
        assert_eq!(with_account.account_id(), Some("account-for-local-mode"));
        let FlagshipEndpoint::Explicit(endpoint) = with_account else {
            panic!("expected explicit endpoint");
        };
        assert_eq!(endpoint.endpoint().as_str(), "https://example.com/evaluate");
    }

    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[test]
    fn timeout_last_call_wins() {
        let custom = build_valid(
            app_builder()
                .disable_timeout()
                .timeout(Duration::from_millis(250)),
        );
        let disabled = app_builder()
            .timeout(Duration::from_millis(250))
            .disable_timeout()
            .build()
            .expect("disabled timeout");

        assert_eq!(
            custom.timeout(),
            TimeoutPolicy::Duration(Duration::from_millis(250))
        );
        assert_eq!(disabled.timeout(), TimeoutPolicy::Disabled);
    }

    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[test]
    fn retries_and_delay_are_capped_and_accept_explicit_zero() {
        let capped = build_valid(
            app_builder()
                .retries(100)
                .retry_delay(Duration::from_secs(100)),
        );
        let zero = app_builder()
            .retries(0)
            .retry_delay(Duration::ZERO)
            .build()
            .expect("zero retry policy is valid");

        assert_eq!(capped.retries(), 10);
        assert_eq!(capped.retry_delay(), Duration::from_secs(30));
        assert_eq!(zero.retries(), 0);
        assert_eq!(zero.retry_delay(), Duration::ZERO);
    }

    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[test]
    fn bearer_and_static_headers_are_sensitive() {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_static("Basic static-secret"),
        );
        headers.append(
            reqwest::header::HeaderName::from_static("x-secret"),
            HeaderValue::from_static("first-secret"),
        );
        headers.append(
            reqwest::header::HeaderName::from_static("x-secret"),
            HeaderValue::from_static("second-secret"),
        );
        let options = build_valid(
            app_builder()
                .auth_token("token-secret")
                .static_headers(headers),
        );

        assert!(
            options
                .bearer_header()
                .is_some_and(HeaderValue::is_sensitive)
        );
        assert!(
            options
                .static_headers()
                .values()
                .all(HeaderValue::is_sensitive)
        );
    }

    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[test]
    fn empty_auth_token_is_absent() {
        let options = build_valid(app_builder().auth_token(""));

        assert!(options.bearer_header().is_none());
    }

    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    #[test]
    fn debug_does_not_expose_secrets() {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_static("Basic static-secret"),
        );
        let builder = app_builder()
            .app_id("app-secret")
            .account_id("account-secret")
            .base_url("https://url-secret.example/api")
            .auth_token("token-secret")
            .static_headers(headers)
            .header_factory(|| async { Ok(HeaderMap::new()) });
        let builder_debug = format!("{builder:?}");
        let options = build_valid(builder);
        let options_debug = format!("{options:?}");
        let explicit = FlagshipClientOptions::builder()
            .endpoint("https://url-secret.example/evaluate")
            .account_id("account-secret")
            .build_endpoint()
            .expect("valid explicit endpoint");
        let explicit_debug = format!("{explicit:?}");

        for output in [&builder_debug, &options_debug, &explicit_debug] {
            assert!(!output.contains("app-secret"));
            assert!(!output.contains("account-secret"));
            assert!(!output.contains("url-secret"));
            assert!(!output.contains("token-secret"));
            assert!(!output.contains("static-secret"));
        }
    }

    #[test]
    fn invalid_configuration_display_does_not_expose_secrets() {
        let error = app_builder()
            .auth_token("display secret")
            .build()
            .expect_err("invalid token");
        assert!(!error.to_string().contains("display secret"));
        assert!(!format!("{error:?}").contains("display secret"));
    }

    #[tokio::test]
    async fn header_factory_is_async_send_sync_and_returns_boxed_errors() {
        fn assert_send_sync<T: Send + Sync + 'static>(_: &T) {}

        let calls = Arc::new(AtomicUsize::new(0));
        let factory_calls = Arc::clone(&calls);
        let builder = app_builder().header_factory(move || {
            let factory_calls = Arc::clone(&factory_calls);

            async move {
                tokio::task::yield_now().await;
                factory_calls.fetch_add(1, Ordering::SeqCst);

                Err(
                    Box::new(io::Error::other("rotating credential unavailable"))
                        as HeaderFactoryError,
                )
            }
        });
        let factory = builder.header_factory.expect("factory configured");

        assert_send_sync(&factory);
        let error = factory
            .headers()
            .await
            .expect_err("factory error should propagate");
        assert_eq!(error.to_string(), "rotating credential unavailable");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn public_configuration_types_are_send_sync() {
        fn assert_send_sync<T: Send + Sync + 'static>() {}

        assert_send_sync::<HeaderFactory>();
        assert_send_sync::<FlagshipClientOptions>();
        assert_send_sync::<FlagshipClientOptionsBuilder>();
    }

    #[test]
    fn rejects_no_mode_and_reports_missing_mode_fields_exactly() {
        assert_message(
            FlagshipClientOptions::builder().build(),
            "either app_id with account_id or endpoint is required",
        );
        assert_message(
            FlagshipClientOptions::builder()
                .account_id("account")
                .build(),
            "app_id is required",
        );
        assert_message(
            FlagshipClientOptions::builder().app_id("app").build(),
            "account_id is required",
        );
    }

    #[test]
    fn identifier_validation_is_strict_but_preserves_interior_characters() {
        let endpoint = FlagshipClientOptions::builder()
            .app_id("team one/apps/main")
            .account_id("account/group")
            .build_endpoint()
            .expect("interior spaces and slashes are preserved");
        let FlagshipEndpoint::App(endpoint) = endpoint else {
            panic!("expected app endpoint");
        };
        assert_eq!(endpoint.app_id(), "team one/apps/main");
        assert_eq!(endpoint.account_id, "account/group");

        let invalid = [
            ("", "app_id must not be empty"),
            (
                " app",
                "app_id must not have leading or trailing whitespace",
            ),
            (
                "app ",
                "app_id must not have leading or trailing whitespace",
            ),
            ("app\nname", "app_id must not contain control characters"),
            ("team/./app", "app_id must not contain dot path segments"),
            ("team/../app", "app_id must not contain dot path segments"),
        ];
        for (app_id, expected) in invalid {
            let result = FlagshipClientOptions::builder()
                .app_id(app_id)
                .account_id("account")
                .build_endpoint();
            assert_message(result, expected);
        }
    }

    #[test]
    fn rejects_conflicting_endpoint_modes_with_exact_messages() {
        assert_message(
            app_builder()
                .endpoint("https://example.com/evaluate")
                .build_endpoint(),
            "endpoint and app_id are mutually exclusive",
        );
        assert_message(
            FlagshipClientOptions::builder()
                .endpoint("https://example.com/evaluate")
                .base_url("https://api.cloudflare.com")
                .build_endpoint(),
            "base_url cannot be used with endpoint",
        );
    }

    #[test]
    fn rejects_invalid_urls_with_exact_messages() {
        let invalid = [
            ("/evaluate", "endpoint must be an absolute HTTP(S) URL"),
            (
                "file:///evaluate",
                "endpoint must be an absolute HTTP(S) URL",
            ),
            (
                "https://user:secret@example.com/evaluate",
                "endpoint must not contain credentials",
            ),
            (
                "https://example.com/evaluate?token=secret",
                "endpoint must not contain a query or fragment",
            ),
            (
                "https://example.com/evaluate#fragment",
                "endpoint must not contain a query or fragment",
            ),
            (
                "https://example.com/eval\nuate",
                "endpoint must not contain control characters",
            ),
            (
                "https://{host}/evaluate",
                "endpoint contains an invalid host",
            ),
            (
                "https://\"host\"/evaluate",
                "endpoint contains an invalid host",
            ),
            (
                "https://`host`/evaluate",
                "endpoint contains an invalid host",
            ),
        ];
        for (endpoint, expected) in invalid {
            let result = FlagshipClientOptions::builder()
                .endpoint(endpoint)
                .build_endpoint();
            assert_message(result, expected);
        }

        assert_message(
            app_builder()
                .base_url("https://example.com?token=secret")
                .build_endpoint(),
            "base_url must not contain a query or fragment",
        );
    }

    #[test]
    fn rejects_invalid_tokens_and_zero_timeout_exactly() {
        for token in [
            " ",
            "has space",
            "has\ttab",
            "line\nbreak",
            "delete\u{7f}",
            "café",
        ] {
            assert_message(
                app_builder().auth_token(token).build(),
                "auth_token must contain only visible ASCII characters without whitespace",
            );
        }

        assert_message(
            app_builder().timeout(Duration::ZERO).build(),
            "timeout must be greater than zero or disabled",
        );
    }

    #[test]
    fn reqwest_types_are_reexported() {
        let _: reqwest::header::HeaderMap = HeaderMap::new();
        let _: Option<reqwest::Client> = None;
    }
}
