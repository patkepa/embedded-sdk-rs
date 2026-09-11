use crate::{CodecError, HubConfig, PropertyBag};

/// Borrowed cloud-to-device message classified from an Azure MQTT publish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CloudToDeviceMessage<'a> {
    payload: &'a [u8],
    properties: PropertyBag<'a>,
}

impl<'a> CloudToDeviceMessage<'a> {
    /// Returns the application payload.
    #[must_use]
    pub const fn payload(self) -> &'a [u8] {
        self.payload
    }

    /// Returns the encoded Azure system and application properties.
    #[must_use]
    pub const fn properties(self) -> PropertyBag<'a> {
        self.properties
    }
}

/// Classifies a device-specific cloud-to-device MQTT publication.
pub fn parse_cloud_to_device<'a>(
    config: &HubConfig,
    topic: &'a str,
    payload: &'a [u8],
) -> Result<CloudToDeviceMessage<'a>, CodecError> {
    let remainder = topic
        .strip_prefix("devices/")
        .and_then(|topic| topic.strip_prefix(config.device_id().as_str()))
        .and_then(|topic| topic.strip_prefix("/messages/devicebound/"))
        .ok_or(CodecError::UnexpectedTopic)?;
    let encoded_properties = remainder.strip_prefix('?').unwrap_or(remainder);
    let properties = PropertyBag::parse(encoded_properties)?;
    Ok(CloudToDeviceMessage {
        payload,
        properties,
    })
}

#[cfg(test)]
mod tests {
    use crate::{DeviceId, HubHostname};

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
    fn parses_device_bound_payload_and_properties() {
        let message = parse_cloud_to_device(
            &config(),
            "devices/sensor-01/messages/devicebound/?command=blink&label=a%20b",
            b"{}",
        )
        .unwrap();
        assert_eq!(message.payload(), b"{}");
        assert_eq!(message.properties().iter().count(), 2);
    }

    #[test]
    fn rejects_another_device_and_malformed_properties() {
        assert_eq!(
            parse_cloud_to_device(&config(), "devices/sensor-02/messages/devicebound/", b"x"),
            Err(CodecError::UnexpectedTopic)
        );
        assert_eq!(
            parse_cloud_to_device(
                &config(),
                "devices/sensor-01/messages/devicebound/?bad=%2",
                b"x"
            ),
            Err(CodecError::InvalidPercentEncoding)
        );
    }
}
