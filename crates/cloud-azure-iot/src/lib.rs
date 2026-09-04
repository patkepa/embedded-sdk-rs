#![no_std]
#![forbid(unsafe_code)]
#![doc = "Bounded Azure IoT Hub device protocol contracts and MQTT topic codecs."]

#[cfg(test)]
extern crate std;

mod c2d;
mod c2d_queue;
mod client;
mod config;
mod credentials;
mod encode;
mod method_queue;
mod methods;
mod properties;
mod requests;
mod sas;
mod telemetry_queue;
mod twin;

pub use c2d::{CloudToDeviceMessage, parse_cloud_to_device};
pub use c2d_queue::{
    CloudToDeviceQueue, CloudToDeviceQueueError, MAX_C2D_FIELD_CAPACITY, QueuedCloudToDevice,
};
pub use client::{
    HubCapabilities, HubClient, HubError, HubEvent, HubSession, HubSessionError, HubSessionEvent,
    InboundRejection, OutboundOperation, SessionDisposition, Subscription, SubscriptionKind,
    TwinOperation,
};
pub use config::{
    ConfigError, DeviceId, HubConfig, HubHostname, IOT_HUB_API_VERSION, MAX_DEVICE_ID_LEN,
    MAX_KEEP_ALIVE_SECONDS, MQTT_TLS_PORT,
};
pub use credentials::{
    DeviceSasProvider, MAX_BASE64_SYMMETRIC_KEY_LEN, SasCredentialError, SasCredentialProvider,
    SasKeySource, SasProviderConfigError,
};
pub use method_queue::{
    DIRECT_METHOD_OVERLOAD_STATUS, DIRECT_METHOD_TIMEOUT_STATUS, DirectMethodDispatch,
    DirectMethodQueue, DirectMethodQueueError, MAX_METHOD_FIELD_CAPACITY, QueuedDirectMethod,
};
pub use methods::{
    DirectMethodRequest, MAX_METHOD_REQUEST_ID_LEN, MethodRequestId, direct_method_filter,
    direct_method_response_topic, parse_direct_method,
};
pub use properties::{CodecError, EncodedProperty, MessageProperty, PropertyBag, PropertyIter};
pub use requests::{RequestId, RequestIdGenerator};
pub use sas::{
    MAX_SAS_TOKEN_LEN, MAX_SYMMETRIC_KEY_LEN, SasError, SasPassword, SasToken, SymmetricKey,
    generate_device_sas,
};
pub use telemetry_queue::{
    MAX_TELEMETRY_PAYLOAD_CAPACITY, QueuedTelemetry, TelemetryDispatch, TelemetryQueue,
    TelemetryQueueError, TelemetryToken,
};
pub use twin::{
    DesiredPropertiesPatch, TwinResponse, desired_properties_filter,
    parse_desired_properties_patch, parse_twin_response, reported_properties_topic, twin_get_topic,
    twin_response_filter,
};
