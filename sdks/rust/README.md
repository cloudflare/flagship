# cloudflare-flagship

[![crates.io](https://img.shields.io/crates/v/cloudflare-flagship.svg)](https://crates.io/crates/cloudflare-flagship)
[![docs.rs](https://img.shields.io/docsrs/cloudflare-flagship)](https://docs.rs/cloudflare-flagship)
[![license](https://img.shields.io/crates/l/cloudflare-flagship.svg)](LICENSE)

[OpenFeature](https://openfeature.dev)-compliant provider SDK for [Cloudflare Flagship](https://developers.cloudflare.com/flagship/) in async Rust server applications.

> The Rust SDK supports HTTP mode only. The Cloudflare Workers binding mode is exclusive to the TypeScript SDK.

## Installation

```sh
cargo add cloudflare-flagship
```

Requires Rust 1.88+ and a Tokio runtime.

## Quick start

```rust,no_run
use cloudflare_flagship::{
    FlagshipClientOptions, FlagshipServerProvider,
    open_feature::{EvaluationContext, OpenFeature},
};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let options = FlagshipClientOptions::builder()
    .app_id("your-app-id")
    .account_id("your-account-id")
    .auth_token("your-token")
    .build()?;
let provider = FlagshipServerProvider::new(options)?;

let mut api = OpenFeature::default();
api.set_provider(provider).await;

let client = api.create_client();
let context = EvaluationContext::default()
    .with_targeting_key("user-123")
    .with_custom_field("plan", "premium");
let enabled = client
    .get_bool_value("dark-mode", Some(&context), None)
    .await
    .unwrap_or(false);
# let _ = enabled;
# Ok(())
# }
```

See [`examples/server.rs`](examples/server.rs) and [`examples/local.rs`](examples/local.rs) for complete programs.

## Configuration

`FlagshipClientOptions::builder()` accepts either `app_id` plus `account_id` or a full `endpoint` URL.

| Option           | Description                                                                     |
| ---------------- | ------------------------------------------------------------------------------- |
| `app_id`         | Flagship app ID. Mutually exclusive with `endpoint`.                            |
| `account_id`     | Required with `app_id`, and always required for local evaluation.               |
| `base_url`       | Base URL override used with `app_id`; defaults to `https://api.cloudflare.com`. |
| `endpoint`       | Full absolute evaluation endpoint URL.                                          |
| `auth_token`     | Adds `Authorization: Bearer <token>` to each request.                           |
| `static_headers` | Static headers. Explicit `Authorization` overrides `auth_token`.                |
| `header_factory` | Async per-request headers. Values override `static_headers` and `auth_token`.   |
| `http_client`    | Custom `reqwest::Client`.                                                       |
| `timeout`        | Per-attempt timeout; defaults to 5 seconds. `disable_timeout()` removes it.     |
| `retries`        | Retry attempts on transient errors; defaults to 1 and is capped at 10.          |
| `retry_delay`    | Delay between retries; defaults to 1 second and is capped at 30 seconds.        |

`FlagshipServerProvider::builder(options)` adds provider options:

| Option              | Description                                                           |
| ------------------- | --------------------------------------------------------------------- |
| `cache_ttl`         | Cache TTL; enables response caching when greater than zero.           |
| `cache_max_entries` | Maximum cached entries; defaults to 1000.                             |
| `local_evaluation`  | Downloads definitions and evaluates flags in-process; off by default. |
| `refresh_interval`  | Background definitions refresh period in local mode; defaults to 30s. |
| `hook`              | Adds a provider-level OpenFeature hook.                               |

## Local evaluation

With local evaluation, the provider downloads the app's flag definitions, evaluates flags in-process with no network call per evaluation, and refreshes definitions in the background.

OpenFeature 0.3 has no provider initialization or shutdown callbacks, so call `initialize_provider` before registering the provider and `shutdown_provider` when you're done:

```rust,no_run
use std::time::Duration;
use cloudflare_flagship::{FlagshipClientOptions, FlagshipServerProvider, open_feature::OpenFeature};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let options = FlagshipClientOptions::builder()
    .app_id("your-app-id")
    .account_id("your-account-id")
    .auth_token("your-read-token")
    .build()?;
let provider = FlagshipServerProvider::builder(options)
    .local_evaluation(true)
    .refresh_interval(Duration::from_secs(30))
    .build()?;

provider.initialize_provider().await?; // fails if the first definitions fetch fails
let mut api = OpenFeature::default();
api.set_provider(provider.clone()).await;

// ... evaluate flags ...

provider.shutdown_provider().await;
# Ok(())
# }
```

- The auth token needs app **read** permission, not `evaluate`.
- `endpoint`, when set, must end in `/evaluate` so the definitions URL can be derived, and `account_id` must match the account in that URL, because rollout bucketing uses it.
- Incompatible with `cache_ttl`.
- Local evaluations do **not** appear in server-side analytics.
- Later refresh failures mark the provider `STALE` and keep the last good definitions. `last_refresh_error()` returns the most recent fetch failure.

## Response caching

Caching is off by default and enabled by setting `cache_ttl`. Entries are keyed by flag key, flag type, and the full evaluation context, and are LRU-evicted beyond `cache_max_entries`. Cache hits resolve with the `CACHED` reason. Disabled flags, errors, and type mismatches are never cached. `shutdown_provider` clears the cache. Each entry stores the serialized context, so large contexts use more memory.

## Hooks

`LoggingHook` reports evaluation stages without context or values; strings are passed raw, so escape them for your log format. `TelemetryHook` includes the evaluation context and resolved value, so treat its events as sensitive.

```rust,no_run
use cloudflare_flagship::{FlagshipClientOptions, FlagshipServerProvider, LoggingHook};

# fn build(options: FlagshipClientOptions) -> Result<(), Box<dyn std::error::Error>> {
let provider = FlagshipServerProvider::builder(options)
    .hook(LoggingHook::new(|event| eprintln!("{:?}: {:?}", event.kind, event.flag_key)))
    .build()?;
# let _ = provider;
# Ok(())
# }
```

## Direct client

`FlagshipClient` evaluates flags without OpenFeature and returns the raw JSON value, variant, reason, and metadata:

```rust,no_run
use cloudflare_flagship::{FlagshipClient, FlagshipClientOptions, open_feature::EvaluationContext};

# async fn run(options: FlagshipClientOptions) -> Result<(), Box<dyn std::error::Error>> {
let client = FlagshipClient::new(options);
let context = EvaluationContext::default().with_targeting_key("user-123");
let response = client.evaluate("dark-mode", &context).await?;
println!("{} ({})", response.value, response.reason);
# Ok(())
# }
```

## Evaluation context

Primitive-only context is sent as URL query parameters, so it can appear in request logs. Context containing structs, arrays, or nulls uses a JSON POST request. Context over [Flagship's limits](https://developers.cloudflare.com/flagship/reference/limits/) returns `INVALID_CONTEXT` before any request is made.

## Error handling

The lower-level `FlagshipClient` returns `FlagshipError` with an `ErrorCode`, HTTP status, and server error code when available. The server's message is kept for 400 and 404 responses. Provider errors use the matching OpenFeature error code with a fixed message, so server error text never reaches OpenFeature logs.

| Error code           | Cause                                                                |
| -------------------- | -------------------------------------------------------------------- |
| `FLAG_NOT_FOUND`     | Flag key does not exist (HTTP 404 or missing from local definitions) |
| `BAD_REQUEST`        | Evaluation request was invalid (HTTP 400)                            |
| `INVALID_CONTEXT`    | Evaluation context is unsupported or over Flagship's limits          |
| `NETWORK_ERROR`      | Network request failed                                               |
| `TIMEOUT_ERROR`      | Request timed out                                                    |
| `PARSE_ERROR`        | API response or flag definition was invalid                          |
| `PROVIDER_NOT_READY` | Local evaluation used before initialization or after shutdown        |
| `GENERAL`            | Any other transient or unexpected failure                            |

Network errors, timeouts, and HTTP 408, 425, 429, and 5xx responses are retried. Other HTTP errors, including 400, 401, 403, and 404, are not, and neither is a server-side flag configuration error. The default HTTP client does not follow redirects.

## Differences from other SDKs

These come from limits of the `open-feature` 0.3 crate:

- A disabled flag resolves to the value served by Flagship, not your default, because providers never receive the default. Check for the `Disabled` reason if you need to fall back.
- Flag values that are JSON `null` or arrays at the root return `TYPE_MISMATCH`. Use `FlagshipClient` for arbitrary JSON.
- `EvaluationError` does not implement `std::error::Error`, so it needs converting before you can use `?`.

## TLS

The default `rustls` feature uses Rustls with its default crypto provider. For platform TLS, or to install your own Rustls provider before building a client:

```sh
cargo add cloudflare-flagship --no-default-features --features native-tls
cargo add cloudflare-flagship --no-default-features --features rustls-no-provider
```

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
```

## License

[Apache-2.0](LICENSE)
