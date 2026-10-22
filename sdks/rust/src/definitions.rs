//! Flag-definition fetching and immutable snapshots for local evaluation.
//!
//! The fetcher reuses the shared client's headers, per-attempt timeout, retry,
//! and HTTP-status behavior. Responses below 400 are parsed as definitions,
//! including non-200 2xx and final 3xx responses, except that a 304 is accepted
//! only when the caller supplied a prior `ETag`. HTTP response errors retain
//! their category and status but discard response-body, server-code, and source
//! details because definitions may contain sensitive configuration.
//!
//! Snapshots retain each flag as an owned [`RawValue`]. A flag is materialized
//! as a [`Value`] only on first lookup, then its success or sanitized error is
//! cached in a [`OnceLock`]. Concurrent first lookups of the same flag may block
//! briefly on that synchronous parse; lookups never hold a lock across an await.
//! The returned value reference lives as long as its snapshot, so providers can
//! share immutable generations through [`std::sync::Arc`] without copying flag
//! values. Invalid UTF-8 anywhere or invalid Unicode top-level field names or
//! flag keys reject the snapshot; invalid strings or keys nested inside one raw
//! flag remain isolated to that flag. The response is never lossily converted.
//!
//! `serde_json`'s default recursion bound is intentionally retained for stack
//! safety. It permits 127 nested JSON containers and rejects the 128th. Because
//! a logical condition level contains multiple JSON objects and arrays, deeply
//! nested conditions can reach this structural bound before the evaluator's
//! separate 100-condition semantic cap. This module deliberately does not use
//! `unbounded_depth` or stack-growing deserialization.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use reqwest::header::{ETAG, HeaderMap, HeaderValue, IF_NONE_MATCH};
use reqwest::{Method, StatusCode, Url};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::client::{FlagshipClient, RawRequest, RawResponse};
use crate::error::{ErrorCode, FlagshipError};

/// Definitions are substantially larger than individual evaluations, but remain
/// bounded so an unexpected response cannot consume arbitrary process memory.
const DEFINITIONS_BODY_LIMIT: usize = 16 * 1024 * 1024;
const ERROR_KEY_LIMIT: usize = 128;

/// A cheap-to-clone fetcher for one provider-supplied definitions endpoint.
#[derive(Clone)]
pub(crate) struct DefinitionsFetcher {
    client: FlagshipClient,
    endpoint: Url,
}

impl DefinitionsFetcher {
    pub(crate) fn new(client: FlagshipClient, endpoint: Url) -> Self {
        Self { client, endpoint }
    }

    /// Fetches a complete snapshot, conditionally when a prior validated `ETag` exists.
    ///
    /// A 304 produces [`DefinitionsFetchResult::NotModified`] only when
    /// `prior_etag` is `Some`; headers injected by client configuration do not
    /// establish provider snapshot state. Every other status below 400 is
    /// parsed, including non-200 2xx and final 3xx responses.
    ///
    /// # Errors
    ///
    /// Returns [`FlagshipError`] for header-factory, transport, timeout, HTTP,
    /// body-limit, envelope, and JSON failures. HTTP response body details are
    /// redacted after the shared retry/status pipeline completes.
    ///
    /// # Panics
    ///
    /// Request execution requires a Tokio runtime with I/O enabled. Configured
    /// timeouts and non-zero retry delays additionally require Tokio time; Tokio
    /// panics when those runtime drivers are unavailable.
    pub(crate) async fn fetch(
        &self,
        prior_etag: Option<&HeaderValue>,
    ) -> Result<DefinitionsFetchResult, FlagshipError> {
        let prior_etag = prior_etag.filter(|value| !value.as_bytes().is_empty());
        let mut extra_headers = HeaderMap::new();
        if let Some(etag) = prior_etag {
            extra_headers.insert(IF_NONE_MATCH, etag.clone());
        }
        let request = RawRequest::new(
            Method::GET,
            self.endpoint.clone(),
            DEFINITIONS_BODY_LIMIT,
            extra_headers,
        );
        let response = self
            .client
            .send_raw(&request)
            .await
            .map_err(redact_http_response_error)?;

        if response.status == StatusCode::NOT_MODIFIED {
            return if prior_etag.is_some() {
                Ok(DefinitionsFetchResult::NotModified)
            } else {
                Err(definitions_parse_error(response.status))
            };
        }

        // Match the shared client's evaluation semantics: every status below
        // 400 reaches the response parser. Thus an empty 204 is a parse error.
        let snapshot = DefinitionsSnapshot::from_response(response)?;

        Ok(DefinitionsFetchResult::Updated(snapshot))
    }
}

impl fmt::Debug for DefinitionsFetcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DefinitionsFetcher")
            .field("client", &self.client)
            .field("endpoint", &"<configured>")
            .finish()
    }
}

/// The result of a conditional definitions fetch.
pub(crate) enum DefinitionsFetchResult {
    /// A response supplied a replacement snapshot.
    Updated(DefinitionsSnapshot),
    /// The server returned 304; the provider must retain its prior snapshot and `ETag`.
    NotModified,
}

impl fmt::Debug for DefinitionsFetchResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Updated(snapshot) => formatter.debug_tuple("Updated").field(snapshot).finish(),
            Self::NotModified => formatter.write_str("NotModified"),
        }
    }
}

struct FlagEntry {
    raw: Box<RawValue>,
    parsed: OnceLock<Result<Value, FlagshipError>>,
}

impl FlagEntry {
    fn new(raw: Box<RawValue>) -> Self {
        Self {
            raw,
            parsed: OnceLock::new(),
        }
    }

    fn get(&self, key: &str) -> Result<&Value, FlagshipError> {
        let parsed = self.parsed.get_or_init(|| {
            serde_json::from_str(self.raw.get()).map_err(|_| malformed_flag_error(key))
        });

        parsed.as_ref().map_err(Clone::clone)
    }
}

/// An immutable definitions generation with independently parsed flag entries.
pub(crate) struct DefinitionsSnapshot {
    flags: BTreeMap<String, FlagEntry>,
    etag: Option<HeaderValue>,
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used when the local provider is wired up")
)]
impl DefinitionsSnapshot {
    fn from_response(response: RawResponse) -> Result<Self, FlagshipError> {
        let RawResponse {
            status,
            headers,
            body,
        } = response;
        let etag = headers
            .get(ETAG)
            .filter(|value| !value.as_bytes().is_empty())
            .cloned();
        let body = std::str::from_utf8(&body).map_err(|_| definitions_parse_error(status))?;
        let envelope: DefinitionsEnvelope =
            serde_json::from_str(body).map_err(|_| definitions_parse_error(status))?;
        let flags = envelope
            .flags
            .into_iter()
            .map(|(key, raw)| (key, FlagEntry::new(raw)))
            .collect();

        Ok(Self { flags, etag })
    }

    pub(crate) fn len(&self) -> usize {
        self.flags.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.flags.is_empty()
    }

    pub(crate) fn etag(&self) -> Option<&HeaderValue> {
        self.etag.as_ref()
    }

    /// Returns `None` for an absent key and `Some(Err(_))` for a malformed value.
    pub(crate) fn get(&self, key: &str) -> Option<Result<&Value, FlagshipError>> {
        self.flags.get(key).map(|entry| entry.get(key))
    }

    /// Constructs a snapshot from response JSON for provider lifecycle tests.
    #[cfg(test)]
    pub(crate) fn from_json_bytes(body: &[u8]) -> Result<Self, FlagshipError> {
        Self::from_response(RawResponse {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: body.to_vec(),
        })
    }
}

impl fmt::Debug for DefinitionsSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DefinitionsSnapshot")
            .field("flag_count", &self.flags.len())
            .field("has_etag", &self.etag.is_some())
            .finish()
    }
}

#[derive(Deserialize)]
struct DefinitionsEnvelope {
    flags: BTreeMap<String, Box<RawValue>>,
}

fn definitions_parse_error(status: StatusCode) -> FlagshipError {
    FlagshipError::new(
        ErrorCode::ParseError,
        "definitions response JSON could not be parsed",
    )
    .with_status(status.as_u16())
    .with_retryable(false)
}

fn redact_http_response_error(error: FlagshipError) -> FlagshipError {
    let Some(status) = error.status() else {
        return error;
    };
    if status < 400 {
        return error;
    }

    let mut redacted = FlagshipError::new(
        error.code().clone(),
        format!("definitions request failed with HTTP {status}"),
    )
    .with_status(status);
    if let Some(retryable) = error.retryable_override() {
        redacted = redacted.with_retryable(retryable);
    }

    redacted
}

fn malformed_flag_error(key: &str) -> FlagshipError {
    let key = sanitized_key(key);

    FlagshipError::new(
        ErrorCode::ParseError,
        format!("flag definition for key \"{key}\" could not be parsed"),
    )
    .with_retryable(false)
}

fn sanitized_key(key: &str) -> String {
    const TRUNCATION_MARKER: &str = "…";
    const _: () = assert!(TRUNCATION_MARKER.len() <= ERROR_KEY_LIMIT);

    let mut sanitized = String::new();
    let mut segment_lengths = Vec::new();
    for character in key.chars() {
        let escaped: String = character.escape_default().collect();
        if sanitized.len() + escaped.len() > ERROR_KEY_LIMIT {
            while sanitized.len() + TRUNCATION_MARKER.len() > ERROR_KEY_LIMIT {
                let Some(segment_length) = segment_lengths.pop() else {
                    return TRUNCATION_MARKER.to_owned();
                };
                sanitized.truncate(sanitized.len() - segment_length);
            }
            sanitized.push_str(TRUNCATION_MARKER);
            break;
        }
        segment_lengths.push(escaped.len());
        sanitized.push_str(&escaped);
    }

    sanitized
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;

    fn response(body: &[u8], etag: Option<HeaderValue>) -> RawResponse {
        let mut headers = HeaderMap::new();
        if let Some(etag) = etag {
            headers.insert(ETAG, etag);
        }

        RawResponse {
            status: StatusCode::OK,
            headers,
            body: body.to_vec(),
        }
    }

    fn snapshot(body: &[u8]) -> Result<DefinitionsSnapshot, FlagshipError> {
        DefinitionsSnapshot::from_json_bytes(body)
    }

    #[test]
    fn envelope_requires_an_object_flags_map_and_rejects_trailing_data() {
        for body in [
            b"not json".as_slice(),
            b"null".as_slice(),
            b"[]".as_slice(),
            b"{}".as_slice(),
            br#"{"flags":null}"#.as_slice(),
            br#"{"flags":[]}"#.as_slice(),
            br#"{"flags":"map"}"#.as_slice(),
            br#"{"flags":{}} trailing"#.as_slice(),
        ] {
            let error = snapshot(body).expect_err("invalid envelope");

            assert_eq!(error.code(), &ErrorCode::ParseError);
            assert_eq!(error.status(), Some(200));
            assert!(!error.is_retryable());
            assert_eq!(
                error.message(),
                "definitions response JSON could not be parsed"
            );
        }
    }

    #[test]
    fn envelope_rejects_invalid_utf8_and_invalid_json_key_strings() {
        for body in [
            b"{\"flags\":{\"\xff\":{}}}".as_slice(),
            b"{\"flags\":{\"bad\":\"\xff\"}}".as_slice(),
            b"{\"ignored\":\"\xff\",\"flags\":{}}".as_slice(),
            br#"{"flags":{"\ud800":{}}}"#.as_slice(),
        ] {
            let error = snapshot(body).expect_err("invalid JSON text");

            assert_eq!(error.code(), &ErrorCode::ParseError);
            assert_eq!(error.status(), Some(200));
            assert_eq!(
                error.message(),
                "definitions response JSON could not be parsed"
            );
        }
    }

    #[test]
    fn invalid_nested_key_is_isolated_to_its_raw_flag() {
        let snapshot = snapshot(br#"{"flags":{"bad":{"\ud800":true},"good":{"enabled":true}}}"#)
            .expect("raw nested strings remain lazy");

        let error = snapshot
            .get("bad")
            .expect("bad flag present")
            .expect_err("lone surrogate is invalid Unicode");
        assert_eq!(error.code(), &ErrorCode::ParseError);
        assert_eq!(
            snapshot
                .get("good")
                .expect("good flag present")
                .expect("sibling remains valid"),
            &json!({"enabled": true})
        );
    }

    #[test]
    fn extra_envelope_fields_are_allowed_and_duplicate_flag_keys_use_last_value() {
        let snapshot = snapshot(
            br#"{"version":7,"flags":{"same":{"value":1},"same":{"value":2}},"future":true}"#,
        )
        .expect("valid envelope");

        assert_eq!(snapshot.len(), 1);
        assert!(!snapshot.is_empty());
        assert_eq!(
            snapshot.get("same").expect("present").expect("valid"),
            &json!({"value": 2})
        );
    }

    #[test]
    fn lookups_are_lazy_cached_and_isolated() {
        let deep = format!("{}0{}", "[".repeat(256), "]".repeat(256));
        let body = format!(r#"{{"flags":{{"good":{{"enabled":true}},"bad":{deep}}}}}"#);
        let snapshot = snapshot(body.as_bytes()).expect("raw envelope remains valid");
        assert!(snapshot.flags["good"].parsed.get().is_none());
        assert!(snapshot.flags["bad"].parsed.get().is_none());

        let first = snapshot
            .get("good")
            .expect("good present")
            .expect("good valid");
        let second = snapshot
            .get("good")
            .expect("good present")
            .expect("good cached");
        assert!(std::ptr::eq(first, second));
        assert_eq!(first, &json!({"enabled": true}));
        let cached_success = snapshot.flags["good"]
            .parsed
            .get()
            .expect("success cached")
            .as_ref()
            .expect("cached success");
        assert!(std::ptr::eq(first, cached_success));

        let first_error = snapshot
            .get("bad")
            .expect("bad present")
            .expect_err("too deep");
        let second_error = snapshot
            .get("bad")
            .expect("bad present")
            .expect_err("cached error");
        assert_eq!(first_error.code(), &ErrorCode::ParseError);
        assert_eq!(first_error.message(), second_error.message());
        let cached_error = snapshot.flags["bad"]
            .parsed
            .get()
            .expect("error cached")
            .as_ref()
            .expect_err("cached error");
        let cached_error_again = snapshot.flags["bad"]
            .parsed
            .get()
            .expect("error remains cached")
            .as_ref()
            .expect_err("cached error");
        assert!(std::ptr::eq(cached_error, cached_error_again));
        assert_eq!(cached_error.message(), first_error.message());
        assert!(snapshot.get("missing").is_none());
        assert_eq!(
            snapshot
                .get("good")
                .expect("good present")
                .expect("isolated"),
            first
        );
    }

    #[test]
    fn malformed_flag_errors_bound_and_escape_the_key_without_body_or_source() {
        let key = format!("private\n\u{202e}\\\"{}", "x".repeat(1_000));
        let deep = format!("{}0{}", "[".repeat(256), "]".repeat(256));

        // Build through RawValue because Value itself cannot represent the deliberately
        // over-deep entry used to exercise the lazy parse failure.
        let body = format!(
            "{{\"flags\":{{{}:{deep}}}}}",
            serde_json::to_string(&key).expect("encoded key")
        );
        let snapshot = snapshot(body.as_bytes()).expect("raw envelope");
        let error = snapshot.get(&key).expect("present").expect_err("too deep");
        let rendered = error.to_string();

        assert_eq!(error.code(), &ErrorCode::ParseError);
        assert!(error.message().len() < 256);
        assert!(rendered.contains(r#"private\n\u{202e}\\\""#));
        assert!(rendered.contains('…'));
        assert!(!rendered.contains(&deep));
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn sanitized_keys_use_exact_bounds_and_unambiguous_complete_escapes() {
        let fitting = "a".repeat(ERROR_KEY_LIMIT);
        let overflowing = "a".repeat(ERROR_KEY_LIMIT + 1);

        assert_eq!(sanitized_key(&fitting), fitting);
        assert_eq!(sanitized_key(&overflowing), format!("{}…", "a".repeat(125)));
        assert_eq!(sanitized_key("literal...text"), "literal...text");
        assert_eq!(
            sanitized_key("line\nquote\"slash\\"),
            r#"line\nquote\"slash\\"#
        );
        assert_eq!(sanitized_key("bidi\u{202e}"), r"bidi\u{202e}");

        let near_escape_boundary = format!("{}\u{202e}", "a".repeat(121));
        let sanitized = sanitized_key(&near_escape_boundary);
        assert_eq!(sanitized, format!("{}…", "a".repeat(121)));
        assert!(!sanitized.ends_with("\\u{202…"));
        assert!(sanitized.len() <= ERROR_KEY_LIMIT);
    }

    #[test]
    fn serde_recursion_boundary_is_127_nested_containers() {
        let accepted = format!("{}0{}", "[".repeat(127), "]".repeat(127));
        let rejected = format!("{}0{}", "[".repeat(128), "]".repeat(128));
        let body = format!(r#"{{"flags":{{"accepted":{accepted},"rejected":{rejected}}}}}"#);
        let snapshot = snapshot(body.as_bytes()).expect("raw values isolate recursive parsing");

        assert!(snapshot.get("accepted").expect("present").is_ok());
        let error = snapshot
            .get("rejected")
            .expect("present")
            .expect_err("128th container exceeds serde_json's default bound");
        assert_eq!(error.code(), &ErrorCode::ParseError);
    }

    #[test]
    fn snapshot_is_send_sync_and_debug_only_reports_metadata() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<DefinitionsSnapshot>();
        assert_send_sync::<DefinitionsFetcher>();
        let secret_key = "private-flag-key";
        let secret_value = "private-definition-value";
        let body = format!(r#"{{"flags":{{"{secret_key}":"{secret_value}"}}}}"#);
        let mut headers = HeaderMap::new();
        headers.insert(ETAG, HeaderValue::from_static("private-etag"));
        let snapshot = Arc::new(
            DefinitionsSnapshot::from_response(RawResponse {
                status: StatusCode::OK,
                headers,
                body: body.into_bytes(),
            })
            .expect("snapshot"),
        );
        let debug = format!("{snapshot:?}");

        assert!(debug.contains("flag_count: 1"));
        assert!(debug.contains("has_etag: true"));
        assert!(!debug.contains(secret_key));
        assert!(!debug.contains(secret_value));
        assert!(!debug.contains("private-etag"));

        let threads = (0..8)
            .map(|_| {
                let snapshot = Arc::clone(&snapshot);

                std::thread::spawn(move || snapshot.get(secret_key).expect("present").is_ok())
            })
            .collect::<Vec<_>>();
        for thread in threads {
            assert!(thread.join().expect("lookup thread"));
        }
    }

    #[test]
    fn missing_response_etag_is_preserved_as_none() {
        let without = snapshot(br#"{"flags":{}}"#).expect("snapshot");
        let empty = DefinitionsSnapshot::from_response(response(
            br#"{"flags":{}}"#,
            Some(HeaderValue::from_static("")),
        ))
        .expect("snapshot");
        let exact = HeaderValue::from_static(r#"W/"opaque""#);
        let with =
            DefinitionsSnapshot::from_response(response(br#"{"flags":{}}"#, Some(exact.clone())))
                .expect("snapshot");

        assert!(without.etag().is_none());
        assert!(without.is_empty());
        assert!(empty.etag().is_none());
        assert_eq!(with.etag(), Some(&exact));
    }

    #[test]
    fn non_200_success_statuses_follow_shared_client_parse_semantics() {
        let accepted = DefinitionsSnapshot::from_response(RawResponse {
            status: StatusCode::ACCEPTED,
            headers: HeaderMap::new(),
            body: br#"{"flags":{}}"#.to_vec(),
        })
        .expect("valid 202 body");
        let no_content = DefinitionsSnapshot::from_response(RawResponse {
            status: StatusCode::NO_CONTENT,
            headers: HeaderMap::new(),
            body: Vec::new(),
        })
        .expect_err("empty 204 body");
        let redirect = DefinitionsSnapshot::from_response(RawResponse {
            status: StatusCode::FOUND,
            headers: HeaderMap::new(),
            body: br#"{"flags":{}}"#.to_vec(),
        })
        .expect("valid final 302 body");

        assert_eq!(accepted.len(), 0);
        assert_eq!(redirect.len(), 0);
        assert_eq!(no_content.code(), &ErrorCode::ParseError);
        assert_eq!(no_content.status(), Some(204));
    }

    #[cfg(not(all(
        feature = "rustls-no-provider",
        not(any(feature = "rustls", feature = "native-tls"))
    )))]
    mod network {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc::{self, Receiver};
        use std::thread::JoinHandle;
        use std::time::Duration;

        use crate::config::{FlagshipClientOptions, FlagshipClientOptionsBuilder};

        use super::*;

        struct TestResponse {
            status: u16,
            headers: Vec<(&'static str, String)>,
            body: Vec<u8>,
        }

        fn local_server(
            path: &str,
            responses: Vec<TestResponse>,
        ) -> (Url, Receiver<Vec<u8>>, JoinHandle<()>) {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
            let address = listener.local_addr().expect("server address");
            let (sender, receiver) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                for response in responses {
                    let (mut stream, _) = listener.accept().expect("accept request");
                    sender
                        .send(read_request(&mut stream))
                        .expect("record request");
                    write!(stream, "HTTP/1.1 {} Test\r\n", response.status).expect("status");
                    for (name, value) in response.headers {
                        write!(stream, "{name}: {value}\r\n").expect("header");
                    }
                    write!(
                        stream,
                        "Content-Length: {}\r\nConnection: close\r\n\r\n",
                        response.body.len()
                    )
                    .expect("response headers");
                    stream.write_all(&response.body).expect("response body");
                }
            });
            let endpoint = Url::parse(&format!("http://{address}{path}")).expect("endpoint");

            (endpoint, receiver, thread)
        }

        fn read_request(stream: &mut TcpStream) -> Vec<u8> {
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).expect("read request");
                assert_ne!(count, 0, "request ended before headers");
                request.extend_from_slice(&buffer[..count]);
            }

            request
        }

        fn client_builder(endpoint: &Url) -> FlagshipClientOptionsBuilder {
            let mut client_endpoint = endpoint.clone();
            client_endpoint.set_query(None);
            client_endpoint.set_fragment(None);

            FlagshipClientOptions::builder()
                .endpoint(client_endpoint.as_str())
                .retries(0)
                .disable_timeout()
        }

        fn fetcher(endpoint: Url, builder: FlagshipClientOptionsBuilder) -> DefinitionsFetcher {
            let options = builder.build().expect("options");
            let client = FlagshipClient::new(options).expect("client");

            DefinitionsFetcher::new(client, endpoint)
        }

        #[tokio::test]
        async fn get_uses_supplied_path_without_content_type_and_round_trips_etags() {
            let response_etag = HeaderValue::from_static(r#"W/"response-tag""#);
            let (endpoint, requests, server) = local_server(
                "/private/definitions?generation=7",
                vec![TestResponse {
                    status: 200,
                    headers: vec![("ETag", response_etag.to_str().expect("ASCII").to_owned())],
                    body: br#"{"flags":{"one":{"enabled":true}}}"#.to_vec(),
                }],
            );
            let prior = HeaderValue::from_static(r#""prior-tag""#);
            let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));

            let result = definitions.fetch(Some(&prior)).await.expect("fetch");
            let request = String::from_utf8(requests.recv().expect("request")).expect("UTF-8");
            server.join().expect("server thread");

            assert!(request.starts_with("GET /private/definitions?generation=7 HTTP/1.1"));
            assert!(request.contains("if-none-match: \"prior-tag\""));
            assert!(!request.to_ascii_lowercase().contains("content-type:"));
            let DefinitionsFetchResult::Updated(snapshot) = result else {
                panic!("expected updated snapshot");
            };
            assert_eq!(snapshot.etag(), Some(&response_etag));
            assert_eq!(snapshot.len(), 1);
        }

        #[tokio::test]
        async fn request_etag_has_final_precedence_and_retry_rebuilds_factory_headers() {
            let (endpoint, requests, server) = local_server(
                "/definitions",
                vec![
                    TestResponse {
                        status: 500,
                        headers: vec![],
                        body: Vec::new(),
                    },
                    TestResponse {
                        status: 200,
                        headers: vec![],
                        body: br#"{"flags":{}}"#.to_vec(),
                    },
                ],
            );
            let calls = Arc::new(AtomicUsize::new(0));
            let factory_calls = Arc::clone(&calls);
            let builder = client_builder(&endpoint)
                .retries(1)
                .retry_delay(Duration::ZERO)
                .header_factory(move || {
                    let call = factory_calls.fetch_add(1, Ordering::SeqCst) + 1;

                    async move {
                        let mut headers = HeaderMap::new();
                        headers.insert(IF_NONE_MATCH, HeaderValue::from_static("factory-loser"));
                        headers.insert(
                            "x-attempt",
                            HeaderValue::from_str(&call.to_string()).expect("attempt header"),
                        );

                        Ok(headers)
                    }
                });
            let definitions = fetcher(endpoint, builder);
            let prior = HeaderValue::from_static("request-winner");

            let result = definitions
                .fetch(Some(&prior))
                .await
                .expect("retried fetch");
            let first = String::from_utf8(requests.recv().expect("first request")).expect("UTF-8");
            let second =
                String::from_utf8(requests.recv().expect("second request")).expect("UTF-8");
            server.join().expect("server thread");

            assert!(matches!(result, DefinitionsFetchResult::Updated(_)));
            assert!(first.contains("if-none-match: request-winner"));
            assert!(second.contains("if-none-match: request-winner"));
            assert!(!first.contains("factory-loser"));
            assert!(first.contains("x-attempt: 1"));
            assert!(second.contains("x-attempt: 2"));
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }

        #[tokio::test]
        async fn not_modified_does_not_parse_body_or_replace_snapshot_state() {
            let (endpoint, requests, server) = local_server(
                "/definitions",
                vec![TestResponse {
                    status: 304,
                    headers: vec![("ETag", "unexpected-replacement".to_owned())],
                    body: b"not JSON and must not be parsed".to_vec(),
                }],
            );
            let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));
            let prior = HeaderValue::from_static("retained-by-provider");

            let result = definitions.fetch(Some(&prior)).await.expect("304 result");
            let _request = requests.recv().expect("request");
            server.join().expect("server thread");

            assert!(matches!(result, DefinitionsFetchResult::NotModified));
        }

        #[tokio::test]
        async fn not_modified_without_explicit_prior_etag_is_a_parse_error() {
            let (endpoint, requests, server) = local_server(
                "/definitions",
                vec![TestResponse {
                    status: 304,
                    headers: vec![],
                    body: Vec::new(),
                }],
            );
            let mut static_headers = HeaderMap::new();
            static_headers.insert(IF_NONE_MATCH, HeaderValue::from_static("injected-etag"));
            let definitions = fetcher(
                endpoint.clone(),
                client_builder(&endpoint).static_headers(static_headers),
            );

            let error = definitions
                .fetch(None)
                .await
                .expect_err("304 requires explicit provider state");
            let request = String::from_utf8(requests.recv().expect("request")).expect("UTF-8");
            server.join().expect("server thread");

            assert!(request.contains("if-none-match: injected-etag"));
            assert_eq!(error.code(), &ErrorCode::ParseError);
            assert_eq!(error.status(), Some(304));
            assert!(!error.is_retryable());
        }

        #[tokio::test]
        async fn empty_explicit_prior_etag_is_treated_as_absent() {
            let (endpoint, requests, server) = local_server(
                "/definitions",
                vec![TestResponse {
                    status: 304,
                    headers: vec![],
                    body: Vec::new(),
                }],
            );
            let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));
            let empty = HeaderValue::from_static("");

            let error = definitions
                .fetch(Some(&empty))
                .await
                .expect_err("empty ETag does not establish conditional state");
            let request = String::from_utf8(requests.recv().expect("request")).expect("UTF-8");
            server.join().expect("server thread");

            assert!(!request.to_ascii_lowercase().contains("if-none-match:"));
            assert_eq!(error.code(), &ErrorCode::ParseError);
            assert_eq!(error.status(), Some(304));
            assert!(!error.is_retryable());
        }

        #[tokio::test]
        async fn missing_etag_clears_conditional_state_in_updated_snapshot() {
            let (endpoint, requests, server) = local_server(
                "/definitions",
                vec![TestResponse {
                    status: 200,
                    headers: vec![],
                    body: br#"{"flags":{}}"#.to_vec(),
                }],
            );
            let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));

            let result = definitions.fetch(None).await.expect("fetch");
            let request = String::from_utf8(requests.recv().expect("request")).expect("UTF-8");
            server.join().expect("server thread");
            let DefinitionsFetchResult::Updated(snapshot) = result else {
                panic!("expected update");
            };

            assert!(snapshot.etag().is_none());
            assert!(!request.to_ascii_lowercase().contains("if-none-match:"));
        }

        #[tokio::test]
        async fn empty_response_etag_is_not_sent_on_the_next_fetch() {
            let (endpoint, requests, server) = local_server(
                "/definitions",
                vec![
                    TestResponse {
                        status: 200,
                        headers: vec![("ETag", String::new())],
                        body: br#"{"flags":{}}"#.to_vec(),
                    },
                    TestResponse {
                        status: 200,
                        headers: vec![],
                        body: br#"{"flags":{}}"#.to_vec(),
                    },
                ],
            );
            let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));

            let first = definitions.fetch(None).await.expect("first fetch");
            let DefinitionsFetchResult::Updated(snapshot) = first else {
                panic!("expected update");
            };
            assert!(snapshot.etag().is_none());
            definitions
                .fetch(snapshot.etag())
                .await
                .expect("second fetch");
            let first_request =
                String::from_utf8(requests.recv().expect("first request")).expect("UTF-8");
            let second_request =
                String::from_utf8(requests.recv().expect("second request")).expect("UTF-8");
            server.join().expect("server thread");

            assert!(
                !first_request
                    .to_ascii_lowercase()
                    .contains("if-none-match:")
            );
            assert!(
                !second_request
                    .to_ascii_lowercase()
                    .contains("if-none-match:")
            );
        }

        #[tokio::test]
        async fn http_errors_retain_status_but_redact_body_server_code_and_source() {
            for (status, expected_code) in [
                (401, ErrorCode::General),
                (404, ErrorCode::FlagNotFound),
                (503, ErrorCode::General),
            ] {
                let secret = format!("private-definitions-response-{status}");
                let (endpoint, requests, server) = local_server(
                    "/definitions",
                    vec![TestResponse {
                        status,
                        headers: vec![],
                        body: format!(
                            r#"{{"errorDetails":"{secret}","errorCode":"SECRET_{status}"}}"#
                        )
                        .into_bytes(),
                    }],
                );
                let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));

                let error = definitions.fetch(None).await.expect_err("HTTP error");
                let _request = requests.recv().expect("request");
                server.join().expect("server thread");
                let debug = format!("{error:?}");

                assert_eq!(error.code(), &expected_code);
                assert_eq!(error.status(), Some(status));
                assert_eq!(
                    error.message(),
                    format!("definitions request failed with HTTP {status}")
                );
                assert!(error.server_code().is_none());
                assert!(std::error::Error::source(&error).is_none());
                assert!(!error.to_string().contains(&secret));
                assert!(!debug.contains(&secret));
                assert!(!debug.contains(&format!("SECRET_{status}")));
            }
        }

        #[tokio::test]
        async fn exact_definitions_body_limit_is_accepted() {
            let prefix = br#"{"flags":{},"padding":""#;
            let suffix = br#""}"#;
            let padding_length = DEFINITIONS_BODY_LIMIT - prefix.len() - suffix.len();
            let mut body = Vec::with_capacity(DEFINITIONS_BODY_LIMIT);
            body.extend_from_slice(prefix);
            body.extend(std::iter::repeat_n(b'x', padding_length));
            body.extend_from_slice(suffix);
            assert_eq!(body.len(), DEFINITIONS_BODY_LIMIT);
            let (endpoint, requests, server) = local_server(
                "/definitions",
                vec![TestResponse {
                    status: 200,
                    headers: vec![],
                    body,
                }],
            );
            let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));

            let result = definitions.fetch(None).await.expect("body at limit");
            let _request = requests.recv().expect("request");
            server.join().expect("server thread");

            assert!(matches!(result, DefinitionsFetchResult::Updated(_)));
        }

        #[tokio::test]
        async fn definitions_body_limit_rejects_oversized_content_length() {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
            let address = listener.local_addr().expect("server address");
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept request");
                let _request = read_request(&mut stream);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    DEFINITIONS_BODY_LIMIT + 1
                )
                .expect("response");
            });
            let endpoint = Url::parse(&format!("http://{address}/definitions")).expect("endpoint");
            let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));

            let error = definitions.fetch(None).await.expect_err("oversized body");
            server.join().expect("server thread");

            assert_eq!(error.code(), &ErrorCode::ParseError);
            assert_eq!(error.status(), Some(200));
            assert!(error.message().contains("16777216-byte"));
        }

        #[tokio::test]
        async fn definitions_body_limit_rejects_oversized_chunked_body() {
            const CHUNK_SIZE: usize = 64 * 1024;

            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
            let address = listener.local_addr().expect("server address");
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("accept request");
                let _request = read_request(&mut stream);
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                    )
                    .expect("response headers");
                let chunk = vec![b' '; CHUNK_SIZE];
                for _ in 0..=(DEFINITIONS_BODY_LIMIT / CHUNK_SIZE) {
                    if write!(stream, "{CHUNK_SIZE:x}\r\n").is_err()
                        || stream.write_all(&chunk).is_err()
                        || stream.write_all(b"\r\n").is_err()
                    {
                        return;
                    }
                }
                let _ = stream.write_all(b"0\r\n\r\n");
            });
            let endpoint = Url::parse(&format!("http://{address}/definitions")).expect("endpoint");
            let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));

            let error = definitions.fetch(None).await.expect_err("oversized body");
            server.join().expect("server thread");

            assert_eq!(error.code(), &ErrorCode::ParseError);
            assert_eq!(error.status(), Some(200));
            assert!(error.message().contains("16777216-byte"));
        }

        #[tokio::test(start_paused = true)]
        async fn configured_timeout_covers_definitions_header_factory() {
            let endpoint = Url::parse("http://127.0.0.1:1/definitions").expect("endpoint");
            let builder = FlagshipClientOptions::builder()
                .endpoint("http://127.0.0.1:1/evaluate")
                .retries(0)
                .timeout(Duration::from_secs(2))
                .header_factory(|| async {
                    std::future::pending::<Result<HeaderMap, crate::config::HeaderFactoryError>>()
                        .await
                });
            let definitions = fetcher(endpoint, builder);

            let error = definitions.fetch(None).await.expect_err("timeout");

            assert_eq!(error.code(), &ErrorCode::TimeoutError);
        }

        #[test]
        fn fetch_future_is_send() {
            fn assert_send<T: Send>(_: T) {}

            let endpoint = Url::parse("http://127.0.0.1:1/definitions").expect("endpoint");
            let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));

            assert_send(definitions.fetch(None));
        }

        #[test]
        fn fetcher_debug_redacts_endpoint() {
            let endpoint =
                Url::parse("https://private.example/secret/definitions?token=x").expect("endpoint");
            let definitions = fetcher(endpoint.clone(), client_builder(&endpoint));
            let debug = format!("{definitions:?}");

            assert!(!debug.contains("private.example"));
            assert!(!debug.contains("secret"));
            assert!(!debug.contains("token"));
        }
    }
}
