use core::{fmt, str};

use embedded_sdk_mqtt::{
    BrokerHostname, BrokerPort, ClientId, Config as MqttConfig, ConfigError as MqttConfigError,
    TopicFilter, TopicName,
};

use crate::{CodecError, MessageProperty, properties::write_telemetry_topic};

/// MQTT API version validated by this provider implementation.
pub const IOT_HUB_API_VERSION: &str = "2021-04-12";
/// Azure IoT Hub MQTT port.
pub const MQTT_TLS_PORT: u16 = 8883;
/// Maximum Azure IoT Hub device identifier length.
pub const MAX_DEVICE_ID_LEN: usize = 128;
/// Maximum keepalive accepted by IoT Hub for a direct MQTT client.
pub const MAX_KEEP_ALIVE_SECONDS: u16 = 1177;

/// Error returned while constructing Azure IoT Hub configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// The device identifier was empty, too long, or contained a forbidden byte.
    InvalidDeviceId,
    /// The keepalive was zero or exceeded the IoT Hub maximum.
    InvalidKeepAlive,
    /// The portable MQTT boundary rejected the translated configuration.
    Mqtt(MqttConfigError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDeviceId => formatter.write_str("invalid Azure IoT device identifier"),
            Self::InvalidKeepAlive => formatter.write_str("invalid Azure IoT keepalive"),
            Self::Mqtt(error) => write!(formatter, "invalid Azure IoT MQTT configuration: {error}"),
        }
    }
}

impl core::error::Error for ConfigError {}

impl From<MqttConfigError> for ConfigError {
    fn from(value: MqttConfigError) -> Self {
        Self::Mqtt(value)
    }
}

/// Fully qualified Azure IoT Hub device endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HubHostname(pub(crate) BrokerHostname);

impl HubHostname {
    /// Validates a fully qualified DNS hostname.
    pub fn new(value: &str) -> Result<Self, ConfigError> {
        Ok(Self(BrokerHostname::new(value)?))
    }

    /// Returns the endpoint hostname.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Bounded Azure IoT Hub device identity.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct DeviceId {
    bytes: [u8; MAX_DEVICE_ID_LEN],
    len: u8,
}

impl DeviceId {
    /// Validates and copies an Azure IoT Hub device identifier.
    pub fn new(value: &str) -> Result<Self, ConfigError> {
        if value.is_empty()
            || value.len() > MAX_DEVICE_ID_LEN
            || !value.bytes().all(is_device_id_byte)
        {
            return Err(ConfigError::InvalidDeviceId);
        }

        let mut bytes = [0; MAX_DEVICE_ID_LEN];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self {
            bytes,
            len: value.len() as u8,
        })
    }

    /// Returns the validated identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }

    /// Returns the identifier length in bytes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns whether the identifier is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DeviceId")
            .field(&self.as_str())
            .finish()
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Validated non-secret IoT Hub connection configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HubConfig {
    device_id: DeviceId,
    mqtt: MqttConfig,
}

impl HubConfig {
    /// Creates a persistent MQTT 3.1.1 IoT Hub session configuration.
    pub fn new(
        hostname: HubHostname,
        device_id: DeviceId,
        keep_alive_seconds: u16,
        maximum_packet_size: u32,
    ) -> Result<Self, ConfigError> {
        if keep_alive_seconds == 0 || keep_alive_seconds > MAX_KEEP_ALIVE_SECONDS {
            return Err(ConfigError::InvalidKeepAlive);
        }

        let mqtt = MqttConfig::new_v311(
            hostname.0,
            BrokerPort::new(MQTT_TLS_PORT)?,
            ClientId::new(device_id.as_str())?,
            keep_alive_seconds,
            false,
            maximum_packet_size,
        )?;
        Ok(Self { device_id, mqtt })
    }

    /// Returns the device identity.
    #[must_use]
    pub const fn device_id(&self) -> &DeviceId {
        &self.device_id
    }

    /// Returns the translated MQTT 3.1.1 configuration.
    #[must_use]
    pub const fn mqtt(&self) -> &MqttConfig {
        &self.mqtt
    }

    /// Writes the complete MQTT username into caller-owned storage.
    pub fn write_mqtt_username<'a>(&self, output: &'a mut [u8]) -> Result<&'a str, CodecError> {
        crate::encode::write_segments(
            output,
            &[
                self.mqtt.hostname().as_str(),
                "/",
                self.device_id.as_str(),
                "/?api-version=",
                IOT_HUB_API_VERSION,
            ],
        )
    }

    /// Builds the base device-to-cloud telemetry topic without properties.
    pub fn telemetry_topic(&self, scratch: &mut [u8]) -> Result<TopicName, CodecError> {
        write_telemetry_topic(self.device_id.as_str(), None, None, &[], scratch)
    }

    /// Builds a telemetry topic with content metadata and application properties.
    pub fn telemetry_topic_with_properties(
        &self,
        content_type: Option<&str>,
        content_encoding: Option<&str>,
        properties: &[MessageProperty<'_>],
        scratch: &mut [u8],
    ) -> Result<TopicName, CodecError> {
        write_telemetry_topic(
            self.device_id.as_str(),
            content_type,
            content_encoding,
            properties,
            scratch,
        )
    }

    /// Builds the cloud-to-device subscription for this identity.
    pub fn cloud_to_device_filter(&self, scratch: &mut [u8]) -> Result<TopicFilter, CodecError> {
        let filter = crate::encode::write_segments(
            scratch,
            &[
                "devices/",
                self.device_id.as_str(),
                "/messages/devicebound/#",
            ],
        )?;
        Ok(TopicFilter::new(filter)?)
    }
}

const fn is_device_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'%'
                | b'_'
                | b'*'
                | b'?'
                | b'!'
                | b'('
                | b')'
                | b','
                | b':'
                | b'='
                | b'@'
                | b'$'
                | b'\''
        )
}

#[cfg(test)]
mod tests {
    use embedded_sdk_mqtt::{ProtocolVersion, SessionConfig};

    use super::*;

    fn config() -> HubConfig {
        HubConfig::new(
            HubHostname::new("contoso.azure-devices.net").unwrap(),
            DeviceId::new("sensor-01").unwrap(),
            240,
            1024,
        )
        .unwrap()
    }

    #[test]
    fn validates_azure_device_identifiers_at_service_bounds() {
        let maximum = "a".repeat(MAX_DEVICE_ID_LEN);
        assert_eq!(DeviceId::new(&maximum).unwrap().len(), MAX_DEVICE_ID_LEN);
        assert_eq!(
            DeviceId::new(&(maximum + "a")),
            Err(ConfigError::InvalidDeviceId)
        );
        assert_eq!(
            DeviceId::new("bad+device"),
            Err(ConfigError::InvalidDeviceId)
        );
        assert_eq!(
            DeviceId::new("bad#device"),
            Err(ConfigError::InvalidDeviceId)
        );
    }

    #[test]
    fn translates_to_persistent_mqtt_311_on_the_tls_port() {
        let config = config();

        assert_eq!(config.mqtt().protocol_version(), ProtocolVersion::V3_1_1);
        assert_eq!(config.mqtt().port().get(), MQTT_TLS_PORT);
        assert_eq!(
            config.mqtt().session(),
            SessionConfig::V3_1_1(embedded_sdk_mqtt::V311SessionConfig::new(false))
        );
    }

    #[test]
    fn builds_connection_identity_and_device_topics() {
        let config = config();
        let mut scratch = [0; 256];

        assert_eq!(
            config.write_mqtt_username(&mut scratch).unwrap(),
            "contoso.azure-devices.net/sensor-01/?api-version=2021-04-12"
        );
        assert_eq!(
            config.telemetry_topic(&mut scratch).unwrap().as_str(),
            "devices/sensor-01/messages/events/"
        );
        assert_eq!(
            config
                .cloud_to_device_filter(&mut scratch)
                .unwrap()
                .as_str(),
            "devices/sensor-01/messages/devicebound/#"
        );
    }

    #[test]
    fn reports_exact_output_capacity_without_partial_success() {
        let config = config();
        let mut output = [0xaa; 8];

        assert_eq!(
            config.write_mqtt_username(&mut output),
            Err(CodecError::OutputTooSmall { required: 59 })
        );
        assert_eq!(output, [0xaa; 8]);
    }

    #[test]
    fn enforces_iot_hub_keepalive_range() {
        let hostname = HubHostname::new("contoso.azure-devices.net").unwrap();
        let device_id = DeviceId::new("sensor-01").unwrap();

        assert_eq!(
            HubConfig::new(hostname, device_id, 0, 1024),
            Err(ConfigError::InvalidKeepAlive)
        );
        assert_eq!(
            HubConfig::new(hostname, device_id, MAX_KEEP_ALIVE_SECONDS + 1, 1024,),
            Err(ConfigError::InvalidKeepAlive)
        );
    }
}
