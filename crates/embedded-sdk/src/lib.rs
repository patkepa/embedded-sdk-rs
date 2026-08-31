#![no_std]
#![forbid(unsafe_code)]
#![doc = "Convenience facade for the portable embedded SDK crates."]

/// Portable Bluetooth Low Energy identity and lifecycle contracts.
pub use embedded_sdk_bluetooth as bluetooth;
/// Configuration versioning and validation.
pub use embedded_sdk_config as config;
/// Hardware identity and capability types.
pub use embedded_sdk_core as core;
/// Portable link and IP configuration state.
pub use embedded_sdk_networking as networking;
/// Service lifecycle and health primitives.
pub use embedded_sdk_runtime as runtime;
/// Persistent key-value and raw flash storage contracts.
pub use embedded_sdk_storage as storage;
/// Backend-independent telemetry events.
pub use embedded_sdk_telemetry as telemetry;
/// Portable Wi-Fi configuration, discovery, and connection contracts.
pub use embedded_sdk_wifi as wifi;
