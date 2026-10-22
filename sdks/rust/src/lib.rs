//! Native asynchronous Rust SDK for Cloudflare Flagship.
//!
//! The crate provides [`FlagshipClient`] for direct remote evaluation and
//! [`FlagshipServerProvider`] for `OpenFeature`. The provider supports remote
//! evaluation, an optional fixed-TTL/LRU response cache, and local evaluation
//! backed by periodically refreshed definitions. [`LoggingHook`] and
//! [`TelemetryHook`] expose structured evaluation events.
//!
//! ## Runtime and lifecycle
//!
//! Network operations use Reqwest on Tokio. A Tokio runtime with I/O is required;
//! configured timeouts, retries, and local refreshes also require Tokio time.
//! Dropping a direct evaluation future cancels SDK work. Local providers should be
//! started with [`FlagshipServerProvider::initialize_provider`] when startup
//! errors must be observed and stopped with
//! [`FlagshipServerProvider::shutdown_provider`]. The last provider clone also
//! aborts its refresh task.
//!
//! `open-feature` 0.3 cannot return provider initialization errors or invoke a
//! provider shutdown method. It also cannot represent a null value or an array at
//! the root of `StructValue`. A disabled flag is a successful evaluation and
//! returns the value served by Flagship. See [`provider`] for the complete
//! compatibility behavior.
//!
//! ## TLS and targets
//!
//! The default `rustls` feature uses Rustls with an installed default crypto
//! provider. Select `native-tls` instead to use the platform TLS backend, or use
//! `rustls-no-provider` when the application installs its own Rustls provider.
//! WebAssembly and Cloudflare Workers bindings are out of scope; Workers should
//! use the TypeScript SDK and its native binding.

#[cfg(not(any(
    feature = "native-tls",
    feature = "rustls",
    feature = "rustls-no-provider",
)))]
compile_error!(
    "cloudflare-flagship requires one TLS transport feature: `rustls` (default), `native-tls`, or `rustls-no-provider`"
);

mod cache;
pub mod client;
pub mod config;
mod context;
mod definitions;
mod endpoint;
pub mod error;
pub mod hooks;
mod local;
pub mod provider;
mod retry;
pub mod types;

pub use client::FlagshipClient;
pub use config::{
    FLAGSHIP_DEFAULT_BASE_URL, FlagshipClientOptions, FlagshipClientOptionsBuilder,
    FlagshipEndpoint, HeaderFactory, HeaderFactoryError, HeaderMap, HeaderValue, TimeoutPolicy,
};
pub use error::{ConfigError, ErrorCode, FlagshipError, UnknownErrorCode};
pub use hooks::{
    LoggingEvent, LoggingHook, LoggingStage, TelemetryEvent, TelemetryEventType, TelemetryHook,
};
pub use provider::{FlagshipServerProvider, FlagshipServerProviderBuilder};
pub use types::{EvaluationReason, FlagshipEvaluationResponse};

/// The compatible `open-feature` version used by this crate.
pub use open_feature;
/// The compatible Reqwest version used by this crate.
pub use reqwest;
/// The compatible Reqwest client type used by this crate.
pub use reqwest::Client;
/// The compatible `serde_json` version used by this crate.
pub use serde_json;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme_doctests {}
