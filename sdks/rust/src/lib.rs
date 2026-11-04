//! Async Rust SDK for [Cloudflare Flagship](https://developers.cloudflare.com/flagship/).
//!
//! [`FlagshipServerProvider`] is an `OpenFeature` provider with remote, cached,
//! and local evaluation. [`FlagshipClient`] evaluates flags directly over HTTP.
//! [`LoggingHook`] and [`TelemetryHook`] report evaluation events.
//!
//! ## Runtime and lifecycle
//!
//! Requires a Tokio runtime. `open-feature` 0.3 has no provider initialization
//! or shutdown callbacks, so local providers should be started with
//! [`FlagshipServerProvider::initialize_provider`] and stopped with
//! [`FlagshipServerProvider::shutdown_provider`]. A disabled flag resolves to
//! the value served by Flagship. See [`provider`] for details.
//!
//! ## TLS
//!
//! The default `rustls` feature uses Rustls with its default crypto provider.
//! Disable default features and enable `native-tls` for platform TLS, or
//! `rustls-no-provider` to install your own Rustls crypto provider.
//! Cloudflare Workers should use the TypeScript SDK and its native binding.

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
pub mod types;

pub use client::FlagshipClient;
pub use config::{
    FLAGSHIP_DEFAULT_BASE_URL, FlagshipClientOptions, FlagshipClientOptionsBuilder,
    HeaderFactoryError, HeaderMap, HeaderValue, TimeoutPolicy,
};
pub use error::{ConfigError, ErrorCode, FlagshipError};
pub use hooks::{
    LoggingEvent, LoggingEventKind, LoggingHook, TelemetryEvent, TelemetryEventKind, TelemetryHook,
};
pub use provider::{FlagshipServerProvider, FlagshipServerProviderBuilder};
pub use types::{EvaluationReason, FlagshipEvaluationResponse};

/// The `open-feature` version used by this crate.
pub use open_feature;
/// The Reqwest version used by this crate.
pub use reqwest;
/// The `serde_json` version used by this crate.
pub use serde_json;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme_doctests {}
