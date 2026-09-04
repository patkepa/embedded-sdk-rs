#![no_std]
#![forbid(unsafe_code)]
#![doc = "Bounded Azure IoT Hub device protocol contracts and MQTT topic codecs."]

#[cfg(test)]
extern crate std;

mod c2d;
mod client;
mod config;
mod encode;
mod methods;
mod properties;
mod requests;
mod sas;
mod twin;

pub use c2d::{CloudToDeviceMessage, parse_cloud_to_device};
pub use client::{
    HubCapabilities, HubClient, HubError, HubEvent, HubSession, HubSessionError, HubSessionEvent,
    OutboundOperation, SessionDisposition, Subscription, SubscriptionKind, TwinOperation,
};
pub use config::{
    ConfigError, DeviceId, HubConfig, HubHostname, IOT_HUB_API_VERSION, MAX_DEVICE_ID_LEN,
    MAX_KEEP_ALIVE_SECONDS, MQTT_TLS_PORT,
};
pub use methods::{
    DirectMethodRequest, MAX_METHOD_REQUEST_ID_LEN, MethodRequestId, direct_method_filter,
    direct_method_response_topic, parse_direct_method,
};
pub use properties::{CodecError, EncodedProperty, MessageProperty, PropertyBag, PropertyIter};
pub use requests::{RequestId, RequestIdGenerator};
pub use sas::{
    MAX_SAS_TOKEN_LEN, MAX_SYMMETRIC_KEY_LEN, SasError, SasToken, SymmetricKey, generate_device_sas,
};
pub use twin::{
    DesiredPropertiesPatch, TwinResponse, desired_properties_filter,
    parse_desired_properties_patch, parse_twin_response, reported_properties_topic, twin_get_topic,
    twin_response_filter,
};
