# Cloudflare Flagship Rust SDK

Native asynchronous feature evaluation for [Cloudflare Flagship](https://developers.cloudflare.com/flagship/), with an [OpenFeature](https://openfeature.dev/) 0.3 provider and a direct HTTP client.

## Install

```toml
[dependencies]
cloudflare-flagship = "0.6.0"
```

The default TLS backend is Rustls. See [TLS features](#tls-features) before changing defaults.

## Remote evaluation with OpenFeature

```rust,no_run
use cloudflare_flagship::{
    FlagshipClientOptions, FlagshipServerProvider,
    open_feature::{EvaluationContext, OpenFeature},
};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let options = FlagshipClientOptions::builder()
    .app_id("app-123")
    .account_id("account-456")
    .auth_token("secret-token")
    .build()?;
let provider = FlagshipServerProvider::new(options)?;

let mut api = OpenFeature::default();
api.set_provider(provider).await;
let client = api.create_named_client("checkout");
let context = EvaluationContext::default().with_targeting_key("user-123");
let enabled = client
    .get_bool_value("new-checkout", Some(&context), None)
    .await
    .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
# let _ = enabled;
# Ok(())
# }
```

## Direct client

Use the native response when OpenFeature type conversion is unnecessary:

```rust,no_run
use cloudflare_flagship::{FlagshipClient, FlagshipClientOptions, open_feature::EvaluationContext};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let options = FlagshipClientOptions::builder()
    .app_id("app-123")
    .account_id("account-456")
    .auth_token("secret-token")
    .build()?;
let client = FlagshipClient::new(options)?;
let response = client
    .evaluate(
        "new-checkout",
        &EvaluationContext::default().with_targeting_key("user-123"),
    )
    .await?;
println!("{} ({})", response.value, response.reason);
# Ok(())
# }
```

## Local evaluation and lifecycle

Local mode downloads definitions, evaluates in-process, and refreshes them in the background. In application mode, local evaluation requires both `app_id` and `account_id`. With an explicit endpoint, provide `account_id` as well so rollout hashing has an account identity, and make the endpoint path end in `/evaluate`; the SDK derives `/definitions` by replacing that final path segment. Initialize explicitly so the first fetch can return an error; OpenFeature 0.3's provider initialization API cannot return one. Shut down explicitly because that API also has no provider shutdown callback.

```rust,no_run
use std::time::Duration;
use cloudflare_flagship::{
    FlagshipClientOptions, FlagshipServerProvider,
    open_feature::OpenFeature,
};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let options = FlagshipClientOptions::builder()
    .app_id("app-123")
    .account_id("account-456")
    .auth_token("secret-token")
    .build()?;
let provider = FlagshipServerProvider::builder(options)
    .local_evaluation(true)
    .refresh_interval(Duration::from_secs(30))
    .build()?;

provider.initialize_provider().await?;
let mut api = OpenFeature::default();
api.set_provider(provider.clone()).await;
let client = api.create_named_client("checkout");
let enabled = client
    .get_bool_value("new-checkout", None, None)
    .await
    .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
# let _ = enabled;
provider.shutdown_provider().await;
# Ok(())
# }
```

Dropping an evaluation future cancels its in-progress SDK work. Dropping the final local-provider clone aborts its refresh task.

If a background refresh fails after initialization, the provider becomes `STALE` but retains the last good definitions snapshot, so local evaluations continue. A later successful refresh replaces the snapshot and returns the provider to `READY`.

## Cache and hooks

The optional remote cache has fixed TTL, bounded LRU capacity, and no singleflight behavior. It cannot be combined with local evaluation. Disabled responses, type mismatches, and errors are never cached. A cache hit retains the response value and variant and reports the `CACHED` evaluation reason.

```rust,no_run
use std::time::Duration;
use cloudflare_flagship::{
    FlagshipClientOptions, FlagshipServerProvider, LoggingHook, TelemetryHook,
};

# fn build() -> Result<(), Box<dyn std::error::Error>> {
let options = FlagshipClientOptions::builder()
    .app_id("app-123")
    .account_id("account-456")
    .build()?;
let logging = LoggingHook::new(|event| {
    eprintln!("{:?}: {}", event.stage(), event.flag_key());
});
let telemetry = TelemetryHook::new(|event| {
    // This event may contain customer context and resolved values.
    let _ = event;
});
let provider = FlagshipServerProvider::builder(options)
    .cache_ttl(Duration::from_secs(15))
    .cache_max_entries(1_000)
    .hook(logging)
    .hook(telemetry)
    .build()?;
# let _ = provider;
# Ok(())
# }
```

Logging events omit context and values. Telemetry explicitly exposes them and should be handled as sensitive data. OpenFeature 0.3 does not provide safely correlatable hook calls or readable hook hints, so telemetry contains neither duration nor hints.

## TLS features

Exactly one TLS strategy is recommended:

| Feature              | Behavior                                                                                                                                     |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `rustls` (default)   | Reqwest Rustls backend with a default crypto provider                                                                                        |
| `native-tls`         | Platform-native TLS; requires `default-features = false`                                                                                     |
| `rustls-no-provider` | Rustls without selecting a crypto provider; requires `default-features = false`; the application must install one before client construction |

For example: `cloudflare-flagship = { version = "0.6.0", default-features = false, features = ["native-tls"] }`. The `rustls-no-provider` configuration likewise requires `default-features = false`:

```toml
cloudflare-flagship = { version = "0.6.0", default-features = false, features = ["rustls-no-provider"] }
```

Cargo features are additive and unified across the dependency graph, so another dependency can enable additional Reqwest TLS features. This crate rejects a build with no TLS transport feature, but does not reject valid feature-unified combinations or providerless Rustls. With `rustls-no-provider`, the application is responsible for installing a Rustls crypto provider before constructing a default client.

## Runtime and compatibility

- Rust 1.88 or newer; Rust 2024 edition.
- Native Tokio runtime and Reqwest transport. Tokio I/O is required for requests; Tokio time is required for SDK timeouts, retry delays, and local refresh.
- WebAssembly and Cloudflare Workers bindings are out of scope. Use the TypeScript SDK's native Workers binding.
- OpenFeature 0.3 cannot represent null or an array at the root of `StructValue`; nested nulls also fail conversion. Use the direct client when arbitrary JSON is required.
- OpenFeature 0.3's `EvaluationError` does not implement `std::error::Error`; adapt it at application error boundaries when using `?`.
- A disabled flag resolves successfully to the value served by Flagship rather than an OpenFeature caller default.

## Requests, retries, and security

The default is one retry with a one-second fixed delay and a five-second timeout **per attempt**, including dynamic-header generation and HTTP I/O. Network, timeout, provider-not-ready, general, and unknown errors are retryable; bad request, missing flag, parse, and invalid-context errors are terminal. Dynamic headers are produced once per attempt. Header precedence is dynamic headers, static headers, bearer token, then injected Reqwest defaults.

Contexts containing only primitive values are sent as GET query parameters. Those values, along with the flag key, can therefore appear in request URLs and proxy or server logs; use the structured POST path where appropriate and treat context data as sensitive regardless.

The SDK enforces Flagship's context limits before transport: 100 top-level attributes, 1–256 characters per top-level key, 10 value-nesting levels, 32 KiB of serialized JSON, and 10,000 visited values.

Do not embed tokens in endpoint URLs or evaluation context. URLs with credentials, query strings, or fragments are rejected. SDK-generated debug output redacts configured URLs and credentials, and request headers are marked sensitive. Telemetry callbacks are an explicit sensitive-data boundary and must avoid blocking or panicking.

## Development

From `sdks/rust`:

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS='-D warnings' cargo doc --all-features --no-deps
cargo package --allow-dirty
```

Inspect package contents with `cargo package --allow-dirty --list`. The packaged crate includes `src/local/cases.json`, which is required by evaluator tests.

Licensed under Apache-2.0. See [`LICENSE`](LICENSE).
