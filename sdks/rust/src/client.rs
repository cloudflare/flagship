//! Native asynchronous Flagship HTTP client.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use open_feature::EvaluationContext;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode, Url};
use serde::Serialize;
use serde_json::Value;

use crate::config::{FlagshipClientOptions, HeaderFactory, TimeoutPolicy};
use crate::context::{NormalizedContext, normalize_context};
use crate::endpoint::resolve_evaluation_endpoint;
use crate::error::{ConfigError, ErrorCode, FlagshipError};
use crate::retry::RetryPolicy;
use crate::types::FlagshipEvaluationResponse;

const EVALUATION_BODY_LIMIT: usize = 1024 * 1024;
const ERROR_BODY_LIMIT: usize = 64 * 1024;
const ERROR_MESSAGE_LIMIT: usize = 512;
const SERVER_CODE_LIMIT: usize = 64;

/// An asynchronous client for the Flagship remote-evaluation API.
///
/// The client is cheap to clone and may be shared between concurrent tasks. Each
/// evaluation owns its request state, invokes the configured header factory once
/// per attempt, and rebuilds its request for every retry. The timeout applies to
/// each attempt independently, not to the complete sequence of attempts and
/// retry delays. Dropping the evaluation future cancels in-progress SDK work.
///
/// Flag-not-found, bad-request, parse, and invalid-context failures are terminal.
/// Network, timeout, general, provider-not-ready, and unknown failures use the
/// configured retry policy. Evaluation response bodies are limited to 1 MiB;
/// error bodies are truncated to 64 KiB without replacing their HTTP status.
///
/// # Examples
///
/// ```no_run
/// use cloudflare_flagship::client::FlagshipClient;
/// use cloudflare_flagship::config::FlagshipClientOptions;
/// use open_feature::EvaluationContext;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let options = FlagshipClientOptions::builder()
///     .app_id("app-123")
///     .account_id("account-456")
///     .auth_token("secret-token")
///     .build()?;
/// let client = FlagshipClient::new(options)?;
/// let response = client
///     .evaluate(
///         "dark-mode",
///         &EvaluationContext::default().with_targeting_key("user-123"),
///     )
///     .await?;
///
/// println!("{} = {}", response.flag_key, response.value);
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct FlagshipClient {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    endpoint: Url,
    http_client: reqwest::Client,
    base_headers: HeaderMap,
    header_factory: Option<HeaderFactory>,
    timeout: TimeoutPolicy,
    retries: u32,
    retry_delay: Duration,
}

impl FlagshipClient {
    /// Constructs a client from validated options.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if the validated endpoint cannot be resolved.
    pub fn new(options: FlagshipClientOptions) -> Result<Self, ConfigError> {
        let endpoint = resolve_evaluation_endpoint(options.endpoint())?;
        let mut base_headers = HeaderMap::new();
        if let Some(bearer) = options.bearer_header() {
            base_headers.insert(AUTHORIZATION, bearer.clone());
        }
        merge_headers(&mut base_headers, options.static_headers());
        let http_client = options.http_client().clone();
        let header_factory = options.header_factory().cloned();
        let timeout = options.timeout();
        let retries = options.retries();
        let retry_delay = options.retry_delay();
        drop(options);

        Ok(Self {
            inner: Arc::new(ClientInner {
                endpoint,
                http_client,
                base_headers,
                header_factory,
                timeout,
                retries,
                retry_delay,
            }),
        })
    }

    /// Returns the resolved evaluation endpoint without a request query.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        self.inner.endpoint.as_str()
    }

    /// Returns the configured number of attempts after the initial attempt.
    #[must_use]
    pub fn retries(&self) -> u32 {
        self.inner.retries
    }

    /// Returns the configured fixed delay between retry attempts.
    #[must_use]
    pub fn retry_delay(&self) -> Duration {
        self.inner.retry_delay
    }

    /// Evaluates `flag_key` using an `OpenFeature` evaluation context.
    ///
    /// Primitive-only contexts use a GET query. Contexts containing nulls,
    /// arrays, or objects use a JSON POST body. The explicit `flag_key` always
    /// controls the requested flag, even when the context contains `flagKey`.
    ///
    /// # Errors
    ///
    /// Returns [`FlagshipError`] when context conversion, dynamic headers,
    /// transport, HTTP status handling, or response parsing fails.
    ///
    /// # Panics
    ///
    /// Request execution requires a Tokio runtime with I/O enabled. Configured
    /// timeouts and non-zero retry delays additionally require Tokio time. Tokio
    /// panics if those drivers are unavailable.
    pub async fn evaluate(
        &self,
        flag_key: &str,
        context: &EvaluationContext,
    ) -> Result<FlagshipEvaluationResponse, FlagshipError> {
        let context = normalize_context(context)?;

        self.evaluate_normalized(flag_key, &context).await
    }

    /// Evaluates an already normalized context for provider and cache paths.
    pub(crate) async fn evaluate_normalized(
        &self,
        flag_key: &str,
        context: &NormalizedContext,
    ) -> Result<FlagshipEvaluationResponse, FlagshipError> {
        let request = EvaluationRequest::build(&self.inner.endpoint, flag_key, context)?;
        let response = self.send_raw(&request.raw).await?;

        parse_evaluation_response(response.status, &response.body)
    }

    /// Sends a replayable request through shared headers, timeout, and retry logic.
    ///
    /// Statuses below 400 are returned to the caller. Bodies for statuses at or
    /// above 400 are truncated to a small diagnostic limit, converted to a
    /// status-preserving error, and retried according to their category.
    /// Successful oversized bodies produce a terminal parse error.
    pub(crate) async fn send_raw(
        &self,
        request: &RawRequest,
    ) -> Result<RawResponse, FlagshipError> {
        let policy = RetryPolicy::new(self.inner.retries, self.inner.retry_delay);

        policy.run(|_| self.execute_attempt(request)).await
    }

    async fn execute_attempt(&self, request: &RawRequest) -> Result<RawResponse, FlagshipError> {
        let attempt = self.execute_attempt_inner(request);

        match self.inner.timeout {
            TimeoutPolicy::Duration(duration) => tokio::time::timeout(duration, attempt)
                .await
                .map_err(|source| {
                    FlagshipError::new(ErrorCode::TimeoutError, "request timed out")
                        .with_source(source)
                })?,
            TimeoutPolicy::Disabled => attempt.await,
        }
    }

    async fn execute_attempt_inner(
        &self,
        request: &RawRequest,
    ) -> Result<RawResponse, FlagshipError> {
        let headers = self.request_headers(request).await?;
        let mut builder = self
            .inner
            .http_client
            .request(request.method.clone(), request.url.clone())
            .headers(headers);

        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }

        let response = builder.send().await.map_err(FlagshipError::from_reqwest)?;
        let response = read_response(response, request.body_limit).await?;

        if let Some(code) = status_error_code(response.status) {
            return Err(http_error(code, response.status, &response.body));
        }

        Ok(response)
    }

    async fn request_headers(&self, request: &RawRequest) -> Result<HeaderMap, FlagshipError> {
        let dynamic = match &self.inner.header_factory {
            Some(factory) => factory.headers().await.map_err(|source| {
                FlagshipError::new(ErrorCode::General, "header factory failed").with_source(source)
            })?,
            None => HeaderMap::new(),
        };

        let mut headers = self.inner.base_headers.clone();
        merge_headers(&mut headers, &dynamic);
        if request.body.is_some() && !headers.contains_key(CONTENT_TYPE) {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        merge_headers(&mut headers, &request.extra_headers);
        mark_sensitive(&mut headers);

        Ok(headers)
    }
}

impl TryFrom<FlagshipClientOptions> for FlagshipClient {
    type Error = ConfigError;

    fn try_from(options: FlagshipClientOptions) -> Result<Self, Self::Error> {
        Self::new(options)
    }
}

impl fmt::Debug for FlagshipClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FlagshipClient")
            .field("endpoint", &"<configured>")
            .field("retries", &self.retries())
            .field("retry_delay", &self.retry_delay())
            .field("timeout", &self.inner.timeout)
            .finish_non_exhaustive()
    }
}

struct EvaluationRequest {
    raw: RawRequest,
}

impl EvaluationRequest {
    fn build(
        endpoint: &Url,
        flag_key: &str,
        context: &NormalizedContext,
    ) -> Result<Self, FlagshipError> {
        if context.requires_post() {
            let body = serde_json::to_vec(&PostBody {
                flag_key,
                context: context.values(),
            })
            .map_err(|source| {
                FlagshipError::new(
                    ErrorCode::InvalidContext,
                    "failed to serialize evaluation context",
                )
                .with_source(source)
            })?;

            return Ok(Self {
                raw: RawRequest::new(
                    Method::POST,
                    endpoint.clone(),
                    EVALUATION_BODY_LIMIT,
                    HeaderMap::new(),
                )
                .with_body(body),
            });
        }

        let mut url = endpoint.clone();
        url.query_pairs_mut()
            .extend_pairs(context.query_pairs(flag_key)?);

        Ok(Self {
            raw: RawRequest::new(Method::GET, url, EVALUATION_BODY_LIMIT, HeaderMap::new()),
        })
    }
}

impl fmt::Debug for EvaluationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EvaluationRequest")
            .field("method", &self.raw.method)
            .field("url", &"<redacted>")
            .field("body", &self.raw.body.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// A replayable internal HTTP request used by evaluation and definitions fetches.
pub(crate) struct RawRequest {
    method: Method,
    url: Url,
    body: Option<Vec<u8>>,
    body_limit: usize,
    extra_headers: HeaderMap,
}

impl RawRequest {
    /// Creates a request with a caller-selected successful-response body limit.
    pub(crate) fn new(
        method: Method,
        url: Url,
        body_limit: usize,
        extra_headers: HeaderMap,
    ) -> Self {
        Self {
            method,
            url,
            body: None,
            body_limit,
            extra_headers,
        }
    }

    /// Adds a replayable body. JSON content type is supplied when no source does.
    pub(crate) fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);

        self
    }
}

/// A fully buffered internal HTTP response.
pub(crate) struct RawResponse {
    pub(crate) status: StatusCode,
    pub(crate) headers: HeaderMap,
    pub(crate) body: Vec<u8>,
}

impl fmt::Debug for RawResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RawResponse")
            .field("status", &self.status)
            .field("header_count", &self.headers.len())
            .field("body_length", &self.body.len())
            .finish()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PostBody<'a> {
    flag_key: &'a str,
    context: &'a std::collections::BTreeMap<String, Value>,
}

fn merge_headers(destination: &mut HeaderMap, source: &HeaderMap) {
    for name in source.keys() {
        destination.remove(name);
        for value in source.get_all(name) {
            destination.append(name, value.clone());
        }
    }
}

fn mark_sensitive(headers: &mut HeaderMap) {
    for value in headers.values_mut() {
        value.set_sensitive(true);
    }
}

async fn read_response(
    mut response: reqwest::Response,
    success_limit: usize,
) -> Result<RawResponse, FlagshipError> {
    let status = response.status();
    let headers = response.headers().clone();
    let is_error = status_error_code(status).is_some();
    let limit = if is_error {
        ERROR_BODY_LIMIT
    } else {
        success_limit
    };

    if !is_error
        && response
            .content_length()
            .is_some_and(|length| usize::try_from(length).map_or(true, |length| length > limit))
    {
        return Err(body_too_large(status, limit));
    }

    let mut body = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or(0)
            .min(limit),
    );

    while let Some(chunk) = response.chunk().await.map_err(|source| {
        let error = FlagshipError::from_reqwest(source);

        error.with_status(status.as_u16())
    })? {
        let remaining = limit.saturating_sub(body.len());
        if chunk.len() > remaining {
            if is_error {
                body.extend_from_slice(&chunk[..remaining]);
                break;
            }

            return Err(body_too_large(status, limit));
        }

        body.extend_from_slice(&chunk);
    }

    Ok(RawResponse {
        status,
        headers,
        body,
    })
}

fn body_too_large(status: StatusCode, limit: usize) -> FlagshipError {
    FlagshipError::new(
        ErrorCode::ParseError,
        format!("response body exceeded the {limit}-byte safety limit"),
    )
    .with_status(status.as_u16())
    .with_retryable(false)
}

fn parse_evaluation_response(
    status: StatusCode,
    body: &[u8],
) -> Result<FlagshipEvaluationResponse, FlagshipError> {
    if let Some(code) = status_error_code(status) {
        return Err(http_error(code, status, body));
    }

    serde_json::from_slice(body).map_err(|source| {
        FlagshipError::new(ErrorCode::ParseError, "response JSON could not be parsed")
            .with_status(status.as_u16())
            .with_source(source)
    })
}

fn status_error_code(status: StatusCode) -> Option<ErrorCode> {
    match status.as_u16() {
        404 => Some(ErrorCode::FlagNotFound),
        400 => Some(ErrorCode::BadRequest),
        401..=999 => Some(ErrorCode::General),
        _ => None,
    }
}

fn http_error(code: ErrorCode, status: StatusCode, body: &[u8]) -> FlagshipError {
    let details = parse_error_body(body);
    let message = if matches!(code, ErrorCode::FlagNotFound | ErrorCode::BadRequest) {
        details.message.unwrap_or_else(|| status_line(status))
    } else {
        format!("HTTP {}: {}", status.as_u16(), status_text(status))
    };

    let mut error = FlagshipError::new(code, message).with_status(status.as_u16());
    if let Some(server_code) = details.server_code {
        error = error.with_server_code(server_code);
    }

    error
}

#[derive(Default)]
struct ErrorBody {
    message: Option<String>,
    server_code: Option<String>,
}

fn parse_error_body(body: &[u8]) -> ErrorBody {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return ErrorBody {
            message: trimmed_body(body).map(|message| sanitize_message(&message)),
            server_code: None,
        };
    };
    let Value::Object(object) = value else {
        return ErrorBody {
            message: go_style_value(&value).map(|message| sanitize_message(&message)),
            server_code: None,
        };
    };

    let message = object
        .get("errorDetails")
        .and_then(go_style_value)
        .or_else(|| string_field(&object, "errorMessage"))
        .or_else(|| string_field(&object, "error"))
        .or_else(|| first_error_field(&object, "message"))
        .or_else(|| trimmed_body(body))
        .map(|message| sanitize_message(&message));
    let server_code = string_field(&object, "errorCode").and_then(sanitize_server_code);

    ErrorBody {
        message,
        server_code,
    }
}

fn string_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(str::to_owned)
}

fn first_error_field(object: &serde_json::Map<String, Value>, field: &str) -> Option<String> {
    let value = object
        .get("errors")?
        .as_array()?
        .first()?
        .as_object()?
        .get(field)?;

    go_style_value(value)
}

fn go_style_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(value).ok(),
    }
}

fn trimmed_body(body: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();

    (!text.is_empty()).then(|| text.to_owned())
}

fn sanitize_message(message: &str) -> String {
    let mut sanitized = String::new();
    let mut segment_lengths = Vec::new();
    let mut truncated = false;

    for character in message.chars() {
        let escaped = if should_escape_message_character(character) {
            character.escape_unicode().to_string()
        } else {
            character.to_string()
        };
        if sanitized.len() + escaped.len() > ERROR_MESSAGE_LIMIT {
            truncated = true;
            break;
        }

        segment_lengths.push(escaped.len());
        sanitized.push_str(&escaped);
    }

    if truncated {
        const MARKER: &str = "…";

        while sanitized.len() + MARKER.len() > ERROR_MESSAGE_LIMIT {
            let Some(segment_length) = segment_lengths.pop() else {
                break;
            };
            sanitized.truncate(sanitized.len() - segment_length);
        }
        sanitized.push_str(MARKER);
    }

    sanitized
}

fn should_escape_message_character(character: char) -> bool {
    if character == '\\' || character.is_control() {
        return true;
    }

    let mut prefixed = String::with_capacity(character.len_utf8() + 1);
    prefixed.push('a');
    prefixed.push(character);
    let mut escaped = prefixed.escape_debug().skip(1);

    escaped.next() == Some('\\') && escaped.next() == Some('u')
}

fn sanitize_server_code(code: String) -> Option<String> {
    if code.is_empty()
        || code.len() > SERVER_CODE_LIMIT
        || !code.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return None;
    }

    Some(code)
}

fn status_text(status: StatusCode) -> String {
    status
        .canonical_reason()
        .map_or_else(|| status.as_u16().to_string(), str::to_owned)
}

fn status_line(status: StatusCode) -> String {
    match status.canonical_reason() {
        Some(reason) => format!("{} {reason}", status.as_u16()),
        None => status.as_u16().to_string(),
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
    use std::error::Error as _;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, Receiver};
    use std::thread::JoinHandle;

    use open_feature::{EvaluationContext, EvaluationContextFieldValue};
    use serde_json::json;

    use super::*;
    use crate::config::{FlagshipClientOptionsBuilder, HeaderMap, HeaderValue};

    fn client_builder(endpoint: &str) -> FlagshipClientOptionsBuilder {
        FlagshipClientOptions::builder()
            .endpoint(endpoint)
            .retries(0)
            .disable_timeout()
    }

    fn build_client(builder: FlagshipClientOptionsBuilder) -> FlagshipClient {
        let options = builder.build().expect("test options");

        FlagshipClient::new(options).expect("test client")
    }

    fn local_server(responses: Vec<(u16, Vec<u8>)>) -> (String, Receiver<Vec<u8>>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = listener.local_addr().expect("local test address");
        let (request_sender, request_receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept test request");
                let request = read_request(&mut stream);
                request_sender.send(request).expect("record test request");

                let reason = StatusCode::from_u16(status)
                    .ok()
                    .and_then(|status| status.canonical_reason())
                    .unwrap_or("Test Status");
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });

        (
            format!("http://{address}/evaluate"),
            request_receiver,
            thread,
        )
    }

    fn stalled_server() -> (String, Receiver<()>, mpsc::Sender<()>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = listener.local_addr().expect("local test address");
        let (accepted_sender, accepted_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let _request = read_request(&mut stream);
            accepted_sender.send(()).expect("signal accepted request");
            release_receiver.recv().expect("release stalled server");
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 32\r\nConnection: close\r\n\r\n{\"flagKey\":\"flag\",\"value\":true}",
            );
        });

        (
            format!("http://{address}/evaluate"),
            accepted_receiver,
            release_sender,
            thread,
        )
    }

    fn chunked_server(status: u16, chunks: Vec<Vec<u8>>) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test server");
        let address = listener.local_addr().expect("local test address");
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let _request = read_request(&mut stream);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Test\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            );
            for chunk in chunks {
                let _ = write!(stream, "{:x}\r\n", chunk.len());
                let _ = stream.write_all(&chunk);
                let _ = stream.write_all(b"\r\n");
            }
            let _ = stream.write_all(b"0\r\n\r\n");
        });

        (format!("http://{address}/evaluate"), thread)
    }

    fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];

        loop {
            let count = stream.read(&mut buffer).expect("read test request");
            assert_ne!(count, 0, "request ended before its body was complete");
            request.extend_from_slice(&buffer[..count]);

            let Some(headers_end) = find_bytes(&request, b"\r\n\r\n") else {
                continue;
            };
            let content_length = request_content_length(&request[..headers_end]);
            if request.len() >= headers_end + 4 + content_length {
                return request;
            }
        }
    }

    fn request_content_length(headers: &[u8]) -> usize {
        String::from_utf8_lossy(headers)
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().expect("numeric content length"))
            })
            .unwrap_or(0)
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    #[tokio::test]
    async fn primitive_context_uses_get_and_requested_flag_wins() {
        let response = br#"{"flagKey":"requested","value":true}"#.to_vec();
        let (endpoint, requests, server) = local_server(vec![(200, response)]);
        let client = build_client(client_builder(&endpoint));
        let context = EvaluationContext::default()
            .with_targeting_key("user one")
            .with_custom_field("flagKey", "context-flag")
            .with_custom_field("enabled", true);

        let result = client
            .evaluate("requested", &context)
            .await
            .expect("successful evaluation");
        let request =
            String::from_utf8(requests.recv().expect("captured request")).expect("UTF-8 request");
        server.join().expect("test server did not panic");

        assert!(request.starts_with("GET /evaluate?"));
        assert!(request.contains("enabled=true"));
        assert!(request.contains("flagKey=requested"));
        assert!(request.contains("targetingKey=user+one"));
        assert!(!request.contains("context-flag"));
        assert!(!request.to_ascii_lowercase().contains("content-type:"));
        assert_eq!(result.variant, "");
        assert_eq!(result.reason, crate::types::EvaluationReason::DEFAULT);
    }

    #[tokio::test]
    async fn structured_context_retries_with_rebuilt_body_and_rotating_headers() {
        let success = br#"{"flagKey":"requested","value":null,"variant":null,"reason":null}"#;
        let (endpoint, requests, server) = local_server(vec![
            (500, br#"{"errorCode":"BUSY"}"#.to_vec()),
            (200, success.to_vec()),
        ]);
        let calls = Arc::new(AtomicUsize::new(0));
        let factory_calls = Arc::clone(&calls);
        let mut static_headers = HeaderMap::new();
        static_headers.insert(AUTHORIZATION, HeaderValue::from_static("Static secret"));
        let builder = client_builder(&endpoint)
            .retries(1)
            .retry_delay(Duration::ZERO)
            .auth_token("bearer-secret")
            .static_headers(static_headers)
            .header_factory(move || {
                let call = factory_calls.fetch_add(1, Ordering::SeqCst) + 1;

                async move {
                    let mut headers = HeaderMap::new();
                    headers.insert(
                        AUTHORIZATION,
                        HeaderValue::from_str(&format!("Dynamic {call}"))?,
                    );

                    Ok(headers)
                }
            });
        let client = build_client(builder);
        let context = EvaluationContext::default()
            .with_custom_field("profile", Arc::new(json!({"plan": "pro"})))
            .with_custom_field("flagKey", "context-flag");

        let result = client
            .evaluate("requested", &context)
            .await
            .expect("retry succeeds");
        let first = String::from_utf8(requests.recv().expect("first request")).expect("UTF-8");
        let second = String::from_utf8(requests.recv().expect("second request")).expect("UTF-8");
        server.join().expect("test server did not panic");

        assert!(first.starts_with("POST /evaluate HTTP/1.1"));
        assert!(
            first
                .to_ascii_lowercase()
                .contains("content-type: application/json")
        );
        assert!(first.contains("authorization: Dynamic 1"));
        assert!(second.contains("authorization: Dynamic 2"));
        assert!(!first.contains("bearer-secret"));
        assert!(!first.contains("Static secret"));
        assert!(first.ends_with(r#"{"flagKey":"requested","context":{"flagKey":"context-flag","profile":{"plan":"pro"}}}"#));
        assert!(second.ends_with(r#"{"flagKey":"requested","context":{"flagKey":"context-flag","profile":{"plan":"pro"}}}"#));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(result.value, Value::Null);
        assert_eq!(result.variant, "");
        assert_eq!(result.reason, crate::types::EvaluationReason::DEFAULT);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_covers_header_factory() {
        let options = FlagshipClientOptions::builder()
            .endpoint("http://127.0.0.1:1/evaluate")
            .retries(0)
            .timeout(Duration::from_secs(2))
            .header_factory(|| async {
                std::future::pending::<Result<HeaderMap, crate::config::HeaderFactoryError>>().await
            })
            .build()
            .expect("test options");
        let client = FlagshipClient::new(options).expect("test client");

        let error = client
            .evaluate("flag", &EvaluationContext::default())
            .await
            .expect_err("header factory should time out");

        assert_eq!(error.code(), &ErrorCode::TimeoutError);
        assert_eq!(error.message(), "request timed out");
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_covers_http_io() {
        let (endpoint, accepted, release, server) = stalled_server();
        let client = build_client(
            FlagshipClientOptions::builder()
                .endpoint(&endpoint)
                .retries(0)
                .timeout(Duration::from_secs(2)),
        );
        let evaluation =
            tokio::spawn(
                async move { client.evaluate("flag", &EvaluationContext::default()).await },
            );

        tokio::task::spawn_blocking(move || accepted.recv())
            .await
            .expect("accept waiter")
            .expect("server accepted request");
        tokio::time::advance(Duration::from_secs(2)).await;
        let error = evaluation
            .await
            .expect("evaluation task")
            .expect_err("HTTP read should time out");
        release.send(()).expect("release server");
        server.join().expect("test server did not panic");

        assert_eq!(error.code(), &ErrorCode::TimeoutError);
    }

    #[tokio::test]
    async fn connection_refused_strips_endpoint_from_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve local port");
        let address = listener.local_addr().expect("reserved address");
        drop(listener);
        let endpoint = format!("http://{address}/private/evaluate");
        let targeting_sentinel = "private-targeting-sentinel";
        let options = FlagshipClientOptions::builder()
            .endpoint(&endpoint)
            .retries(0)
            .disable_timeout()
            .build()
            .expect("test options");
        let client = FlagshipClient::new(options).expect("test client");

        let error = client
            .evaluate(
                "flag",
                &EvaluationContext::default().with_targeting_key(targeting_sentinel),
            )
            .await
            .expect_err("unused port should refuse connection");
        let source = error
            .source()
            .and_then(|source| source.downcast_ref::<reqwest::Error>());

        assert_eq!(error.code(), &ErrorCode::NetworkError);
        assert!(source.is_some_and(|source| source.url().is_none()));
        assert!(!error.to_string().contains(&endpoint));
        assert!(!error.to_string().contains(targeting_sentinel));
        let debug = format!("{error:?}");
        assert!(!debug.contains(&endpoint));
        assert!(!debug.contains(targeting_sentinel));

        let mut source = error.source();
        while let Some(current) = source {
            let rendered = current.to_string();
            assert!(!rendered.contains(&endpoint));
            assert!(!rendered.contains(targeting_sentinel));
            source = current.source();
        }
    }

    #[tokio::test]
    async fn terminal_http_statuses_do_not_retry() {
        for (status, expected) in [(400, ErrorCode::BadRequest), (404, ErrorCode::FlagNotFound)] {
            let (endpoint, _requests, server) = local_server(vec![(status, b"terminal".to_vec())]);
            let calls = Arc::new(AtomicUsize::new(0));
            let factory_calls = Arc::clone(&calls);
            let client = build_client(
                FlagshipClientOptions::builder()
                    .endpoint(&endpoint)
                    .retries(3)
                    .retry_delay(Duration::ZERO)
                    .disable_timeout()
                    .header_factory(move || {
                        factory_calls.fetch_add(1, Ordering::SeqCst);

                        async { Ok(HeaderMap::new()) }
                    }),
            );

            let error = client
                .evaluate("flag", &EvaluationContext::default())
                .await
                .expect_err("terminal status should fail");
            server.join().expect("test server did not panic");

            assert_eq!(error.code(), &expected);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn unauthorized_and_forbidden_statuses_retry_without_exposing_bodies() {
        for status in [401, 403] {
            let private_body = br#"{"error":"private authorization details"}"#.to_vec();
            let (endpoint, requests, server) = local_server(vec![
                (status, private_body.clone()),
                (status, private_body.clone()),
            ]);
            let client = build_client(
                client_builder(&endpoint)
                    .retries(1)
                    .retry_delay(Duration::ZERO),
            );

            let error = client
                .evaluate("flag", &EvaluationContext::default())
                .await
                .expect_err("authorization status should fail after retry");

            assert_eq!(error.code(), &ErrorCode::General);
            assert_eq!(error.status(), Some(status));
            assert!(error.is_retryable());
            assert!(!error.message().contains("private"));
            assert!(!format!("{error:?}").contains("private"));
            assert_eq!(requests.iter().count(), 2);
            server.join().expect("test server did not panic");
        }
    }

    #[tokio::test]
    async fn nonstandard_status_at_or_above_400_is_an_http_error() {
        let (endpoint, _requests, server) = local_server(vec![(600, b"nonstandard".to_vec())]);
        let client = build_client(client_builder(&endpoint));

        let error = client
            .evaluate("flag", &EvaluationContext::default())
            .await
            .expect_err("600 status should fail");
        server.join().expect("test server did not panic");

        assert_eq!(error.code(), &ErrorCode::General);
        assert_eq!(error.status(), Some(600));
    }

    #[tokio::test(start_paused = true)]
    async fn header_factory_failures_retry_once_per_attempt() {
        let calls = Arc::new(AtomicUsize::new(0));
        let factory_calls = Arc::clone(&calls);
        let client = build_client(
            FlagshipClientOptions::builder()
                .endpoint("http://127.0.0.1:1/evaluate")
                .retries(2)
                .retry_delay(Duration::from_secs(3))
                .disable_timeout()
                .header_factory(move || {
                    factory_calls.fetch_add(1, Ordering::SeqCst);

                    async {
                        Err::<HeaderMap, _>(Box::new(std::io::Error::other("factory secret"))
                            as crate::config::HeaderFactoryError)
                    }
                }),
        );

        let error = client
            .evaluate("flag", &EvaluationContext::default())
            .await
            .expect_err("factory always fails");

        assert_eq!(error.code(), &ErrorCode::General);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(!format!("{error:?}").contains("factory secret"));
    }

    #[tokio::test]
    async fn invalid_context_fails_before_headers_or_network() {
        #[derive(Debug)]
        struct Unsupported;

        let calls = Arc::new(AtomicUsize::new(0));
        let factory_calls = Arc::clone(&calls);
        let client = build_client(
            FlagshipClientOptions::builder()
                .endpoint("http://127.0.0.1:1/evaluate")
                .retries(3)
                .disable_timeout()
                .header_factory(move || {
                    factory_calls.fetch_add(1, Ordering::SeqCst);

                    async { Ok(HeaderMap::new()) }
                }),
        );
        let context = EvaluationContext::default().with_custom_field(
            "invalid",
            EvaluationContextFieldValue::new_struct(Unsupported),
        );

        let error = client
            .evaluate("flag", &context)
            .await
            .expect_err("unsupported context should fail");

        assert_eq!(error.code(), &ErrorCode::InvalidContext);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn header_precedence_multivalue_content_type_and_sensitivity() {
        let mut static_headers = HeaderMap::new();
        static_headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Static lower-secret"),
        );
        static_headers.append("x-values", HeaderValue::from_static("static-one"));
        static_headers.append("x-values", HeaderValue::from_static("static-two"));
        static_headers.insert("x-static-only", HeaderValue::from_static("static-kept"));
        let client = build_client(
            FlagshipClientOptions::builder()
                .endpoint("http://127.0.0.1:1/evaluate")
                .auth_token("bearer-lower-secret")
                .static_headers(static_headers)
                .disable_timeout()
                .header_factory(|| async {
                    let mut headers = HeaderMap::new();
                    headers.insert(AUTHORIZATION, HeaderValue::from_static("Dynamic winner"));
                    headers.append("x-values", HeaderValue::from_static("dynamic-one"));
                    headers.append("x-values", HeaderValue::from_static("dynamic-two"));
                    headers.insert("x-dynamic-only", HeaderValue::from_static("dynamic-kept"));
                    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/custom"));

                    Ok(headers)
                }),
        );
        let body_request = RawRequest::new(
            Method::POST,
            Url::parse("http://127.0.0.1:1/evaluate").expect("test URL"),
            16,
            HeaderMap::new(),
        )
        .with_body(Vec::new());

        let headers = client
            .request_headers(&body_request)
            .await
            .expect("dynamic headers");
        let values = headers
            .get_all("x-values")
            .iter()
            .map(|value| value.to_str().expect("ASCII header"))
            .collect::<Vec<_>>();

        assert_eq!(headers[AUTHORIZATION], "Dynamic winner");
        assert_eq!(values, ["dynamic-one", "dynamic-two"]);
        assert_eq!(headers["x-static-only"], "static-kept");
        assert_eq!(headers["x-dynamic-only"], "dynamic-kept");
        assert_eq!(headers[CONTENT_TYPE], "application/custom");
        assert!(headers.values().all(HeaderValue::is_sensitive));
        let debug = format!("{headers:?}");
        assert!(!debug.contains("lower-secret"));
        assert!(!debug.contains("Dynamic winner"));

        let mut extra = HeaderMap::new();
        extra.insert(AUTHORIZATION, HeaderValue::from_static("Extra final"));
        extra.insert(CONTENT_TYPE, HeaderValue::from_static("application/final"));
        let request = RawRequest::new(
            Method::POST,
            Url::parse("http://127.0.0.1:1/evaluate").expect("test URL"),
            16,
            extra,
        )
        .with_body(Vec::new());
        let headers = client
            .request_headers(&request)
            .await
            .expect("extra headers");
        assert_eq!(headers[AUTHORIZATION], "Extra final");
        assert_eq!(headers[CONTENT_TYPE], "application/final");
        assert!(headers.values().all(HeaderValue::is_sensitive));

        let bearer_client = build_client(
            FlagshipClientOptions::builder()
                .endpoint("http://127.0.0.1:1/evaluate")
                .auth_token("bearer-alone")
                .disable_timeout(),
        );
        let bearer_headers = bearer_client
            .request_headers(&RawRequest::new(
                Method::GET,
                Url::parse("http://127.0.0.1:1/evaluate").expect("test URL"),
                16,
                HeaderMap::new(),
            ))
            .await
            .expect("bearer headers");
        assert_eq!(bearer_headers[AUTHORIZATION], "Bearer bearer-alone");
        assert!(bearer_headers[AUTHORIZATION].is_sensitive());
    }

    #[tokio::test]
    async fn oversized_content_length_success_is_terminal_but_error_keeps_status() {
        let (endpoint, _requests, server) = local_server(vec![(200, vec![b'x'; 9])]);
        let client = build_client(client_builder(&endpoint));
        let request = RawRequest::new(
            Method::GET,
            Url::parse(&endpoint).expect("test URL"),
            8,
            HeaderMap::new(),
        );

        let error = client
            .send_raw(&request)
            .await
            .expect_err("successful body exceeds limit");
        server.join().expect("test server did not panic");
        assert_eq!(error.code(), &ErrorCode::ParseError);
        assert_eq!(error.status(), Some(200));
        assert!(!error.is_retryable());
        assert!(error.message().contains("8-byte"));

        let (endpoint, _requests, server) =
            local_server(vec![(400, vec![b'e'; ERROR_BODY_LIMIT + 1024])]);
        let client = build_client(client_builder(&endpoint));
        let error = client
            .evaluate("flag", &EvaluationContext::default())
            .await
            .expect_err("large error response should remain a status error");
        server.join().expect("test server did not panic");
        assert_eq!(error.code(), &ErrorCode::BadRequest);
        assert_eq!(error.status(), Some(400));
        assert!(error.message().len() <= ERROR_MESSAGE_LIMIT);
    }

    #[tokio::test]
    async fn oversized_chunked_success_is_terminal() {
        let (endpoint, server) = chunked_server(200, vec![b"1234".to_vec(), b"56789".to_vec()]);
        let client = build_client(client_builder(&endpoint));
        let request = RawRequest::new(
            Method::GET,
            Url::parse(&endpoint).expect("test URL"),
            8,
            HeaderMap::new(),
        );

        let error = client
            .send_raw(&request)
            .await
            .expect_err("chunked response exceeds limit");
        server.join().expect("test server did not panic");

        assert_eq!(error.code(), &ErrorCode::ParseError);
        assert_eq!(error.status(), Some(200));
        assert!(error.message().contains("8-byte"));
    }

    #[tokio::test]
    async fn dropping_evaluation_cancels_mid_request() {
        let (endpoint, accepted, release, server) = stalled_server();
        let client = build_client(client_builder(&endpoint));
        let task =
            tokio::spawn(
                async move { client.evaluate("flag", &EvaluationContext::default()).await },
            );

        tokio::task::spawn_blocking(move || accepted.recv())
            .await
            .expect("accept waiter")
            .expect("server accepted request");
        task.abort();
        assert!(
            task.await
                .expect_err("evaluation should be cancelled")
                .is_cancelled()
        );
        release.send(()).expect("release server");
        server.join().expect("test server did not panic");
    }

    #[test]
    fn response_errors_include_status_details_and_server_code() {
        let not_found = parse_evaluation_response(
            StatusCode::NOT_FOUND,
            br#"{"errorDetails":"missing","errorCode":"FLAG_UNKNOWN"}"#,
        )
        .expect_err("404 should fail");
        assert_eq!(not_found.code(), &ErrorCode::FlagNotFound);
        assert_eq!(not_found.status(), Some(404));
        assert_eq!(not_found.message(), "missing");
        assert_eq!(not_found.server_code(), Some("FLAG_UNKNOWN"));

        let bad_request = parse_evaluation_response(StatusCode::BAD_REQUEST, b" invalid request ")
            .expect_err("400 should fail");
        assert_eq!(bad_request.code(), &ErrorCode::BadRequest);
        assert_eq!(bad_request.message(), "invalid request");

        let general = parse_evaluation_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            br#"{"errorCode":"BUSY"}"#,
        )
        .expect_err("500 should fail");
        assert_eq!(general.code(), &ErrorCode::General);
        assert_eq!(general.server_code(), Some("BUSY"));

        let unsafe_detail = format!("line\n\u{202e}{}", "é".repeat(600));
        let body = serde_json::to_vec(&json!({
            "errorDetails": unsafe_detail,
            "errorCode": "BAD CODE",
            "errors": [{"code": "GENERIC_MUST_BE_IGNORED"}]
        }))
        .expect("error JSON");
        let sanitized =
            parse_evaluation_response(StatusCode::BAD_REQUEST, &body).expect_err("400 should fail");
        assert!(sanitized.message().len() <= ERROR_MESSAGE_LIMIT);
        assert!(sanitized.message().contains("\\u{a}"));
        assert!(sanitized.message().contains("\\u{202e}"));
        assert!(sanitized.message().ends_with('…'));
        assert!(sanitized.server_code().is_none());
    }

    #[test]
    fn message_sanitization_escapes_invisible_characters_but_preserves_visible_text() {
        let message = concat!(
            "letters \"quotes\" emoji 😀 combining e\u{301} backslash\\ ",
            "\u{0}\n\u{ad}\u{2028}\u{2029}\u{200b}\u{200c}\u{200d}",
            "\u{2060}\u{2062}\u{feff}\u{202e}\u{e0061}"
        );
        let sanitized = sanitize_message(message);

        assert!(sanitized.starts_with("letters \"quotes\" emoji 😀 combining e\u{301} backslash"));
        for escaped in [
            "\\u{5c}",
            "\\u{0}",
            "\\u{a}",
            "\\u{ad}",
            "\\u{2028}",
            "\\u{2029}",
            "\\u{200b}",
            "\\u{200c}",
            "\\u{200d}",
            "\\u{2060}",
            "\\u{2062}",
            "\\u{feff}",
            "\\u{202e}",
            "\\u{e0061}",
        ] {
            assert!(sanitized.contains(escaped), "missing {escaped}");
        }
        for unsafe_character in [
            '\0',
            '\n',
            '\u{ad}',
            '\u{2028}',
            '\u{2029}',
            '\u{200b}',
            '\u{200c}',
            '\u{200d}',
            '\u{2060}',
            '\u{2062}',
            '\u{feff}',
            '\u{202e}',
            '\u{e0061}',
        ] {
            assert!(!sanitized.contains(unsafe_character));
        }

        let truncated = sanitize_message(&"é".repeat(ERROR_MESSAGE_LIMIT));
        assert!(truncated.len() <= ERROR_MESSAGE_LIMIT);
        assert!(truncated.ends_with('…'));
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn malformed_trailing_and_oversized_responses_are_rejected_and_debug_is_redacted() {
        for body in [
            b"not JSON".as_slice(),
            br#"{"flagKey":"flag","value":true} trailing"#.as_slice(),
            br#"{"flagKey":"flag"}"#.as_slice(),
        ] {
            let error =
                parse_evaluation_response(StatusCode::OK, body).expect_err("invalid response");
            assert_eq!(error.code(), &ErrorCode::ParseError);
            assert_eq!(error.status(), Some(200));
        }

        let oversized = body_too_large(StatusCode::OK, 1234);
        assert_eq!(oversized.code(), &ErrorCode::ParseError);
        assert!(!oversized.is_retryable());
        assert!(oversized.message().contains("1234-byte"));

        let request = EvaluationRequest {
            raw: RawRequest::new(
                Method::POST,
                Url::parse("https://secret.example/evaluate?token=secret").expect("test URL"),
                EVALUATION_BODY_LIMIT,
                HeaderMap::new(),
            )
            .with_body(b"private context".to_vec()),
        };
        let debug = format!("{request:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("private context"));
    }

    #[test]
    fn client_and_evaluation_future_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        fn assert_send<T: Send>(_: T) {}

        assert_send_sync::<FlagshipClient>();

        let options = FlagshipClientOptions::builder()
            .endpoint("http://127.0.0.1:1/evaluate")
            .http_client(reqwest::Client::new())
            .build()
            .expect("test options");
        let client = FlagshipClient::new(options).expect("test client");
        let cloned = client.clone();
        assert!(Arc::ptr_eq(&client.inner, &cloned.inner));
        let context = EvaluationContext::default();
        assert_send(client.evaluate("flag", &context));
    }

    #[test]
    fn client_debug_redacts_endpoint_and_token_but_reports_timeout() {
        let options = FlagshipClientOptions::builder()
            .endpoint("https://private-host.example/secret/evaluate")
            .auth_token("private-token")
            .timeout(Duration::from_secs(17))
            .http_client(reqwest::Client::new())
            .build()
            .expect("test options");
        let client = FlagshipClient::new(options).expect("test client");
        let debug = format!("{client:?}");

        assert!(debug.contains("17s"));
        assert!(!debug.contains("private-host"));
        assert!(!debug.contains("secret/evaluate"));
        assert!(!debug.contains("private-token"));
    }
}
