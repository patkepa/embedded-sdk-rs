#![no_std]
#![forbid(unsafe_code)]
#![doc = "Convenience facade for the portable embedded SDK crates."]

/// Configuration versioning and validation.
pub use embedded_sdk_config as config;
/// Hardware identity and capability types.
pub use embedded_sdk_core as core;
/// Service lifecycle and health primitives.
pub use embedded_sdk_runtime as runtime;
/// Backend-independent telemetry events.
pub use embedded_sdk_telemetry as telemetry;
