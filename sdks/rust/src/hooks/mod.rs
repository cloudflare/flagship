//! Flagship-specific `OpenFeature` hooks.
//!
//! [`LoggingHook`] is Cloudflare Flagship's structured, privacy-conscious hook. It is distinct
//! from `open_feature::LoggingHook`, which formats through the `log` crate and can optionally
//! include evaluation context data.
//!
//! [`TelemetryHook`] instead exposes owned customer context and values to an explicitly registered
//! callback. In `open-feature` 0.3, [`open_feature::HookHints`] has no public non-default
//! constructor, mutator, or accessor, so applications cannot attach readable hints and hooks cannot
//! forward them. Telemetry also omits duration because hook stages have no stable evaluation ID and
//! their temporary contexts cannot be correlated safely.

mod logging;
mod telemetry;

pub use logging::{LoggingEvent, LoggingHook, LoggingStage};
pub use telemetry::{TelemetryEvent, TelemetryEventType, TelemetryHook};
