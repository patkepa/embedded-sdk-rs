#![no_std]
#![forbid(unsafe_code)]
#![doc = "Convenience facade for the portable embedded SDK crates."]

/// Portable Bluetooth Low Energy identity and lifecycle contracts.
pub use embedded_sdk_bluetooth as bluetooth;
/// Portable cloud lifecycle contracts and opt-in provider APIs.
pub mod cloud {
    /// Provider-independent cloud lifecycle and capability contracts.
    pub use embedded_sdk_cloud_core as core;

    /// Azure IoT Hub device configuration and protocol contracts.
    #[cfg(feature = "azure-iot")]
    pub use embedded_sdk_cloud_azure_iot as azure_iot;
}
/// Configuration versioning and validation.
pub use embedded_sdk_config as config;
/// Hardware identity and capability types.
pub use embedded_sdk_core as core;
/// Portable MQTT configuration and lifecycle contracts.
pub use embedded_sdk_mqtt as mqtt;
/// Portable link and IP configuration state.
pub use embedded_sdk_networking as networking;
/// Service lifecycle and health primitives.
pub use embedded_sdk_runtime as runtime;
/// Trusted-time, randomness, credential-lifetime, and secret-storage contracts.
pub use embedded_sdk_security as security;
/// Persistent key-value and raw flash storage contracts.
pub use embedded_sdk_storage as storage;
/// Backend-independent telemetry events.
pub use embedded_sdk_telemetry as telemetry;
/// Portable Wi-Fi configuration, discovery, and connection contracts.
pub use embedded_sdk_wifi as wifi;
