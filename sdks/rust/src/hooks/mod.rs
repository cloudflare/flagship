//! Flagship-specific `OpenFeature` hooks.
//!
//! [`LoggingHook`] emits lifecycle events that omit evaluation context and values.
//!
//! [`TelemetryHook`] passes owned evaluation context and resolved values to your callback, so its
//! events may contain sensitive customer data.

mod logging;
mod telemetry;

pub use logging::{LoggingEvent, LoggingEventKind, LoggingHook};
pub use telemetry::{TelemetryEvent, TelemetryEventKind, TelemetryHook};
