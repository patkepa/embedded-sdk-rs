use core::{fmt, str};

use embedded_sdk_mqtt::{ConfigError as MqttConfigError, MAX_TOPIC_LEN, TopicName};

use crate::encode::{
    decode_percent, percent_encoded_len, validate_percent_encoded, write_percent_encoded,
};

/// Error returned by Azure IoT topic encoders and parsers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CodecError {
    /// A caller-owned output buffer cannot contain the complete value.
    OutputTooSmall {
        /// Exact number of bytes required when it can be calculated.
        required: usize,
    },
    /// A property name was empty or reserved for an Azure system property.
    InvalidPropertyName,
    /// A percent-encoded component was incomplete or malformed.
    InvalidPercentEncoding,
    /// Decoded bytes were not valid UTF-8.
    InvalidEncoding,
    /// The topic did not match the expected Azure IoT operation.
    UnexpectedTopic,
    /// An operation topic did not contain a usable request identifier.
    MissingRequestId,
    /// A service response status was missing or malformed.
    InvalidStatus,
    /// A desired or reported property version was malformed.
    InvalidVersion,
    /// The portable MQTT topic boundary rejected the complete encoded topic.
    Mqtt(MqttConfigError),
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutputTooSmall { required } => {
                write!(formatter, "Azure IoT output requires {required} bytes")
            }
            Self::InvalidPropertyName => formatter.write_str("invalid Azure IoT property name"),
            Self::InvalidPercentEncoding => {
                formatter.write_str("invalid Azure IoT percent encoding")
            }
            Self::InvalidEncoding => formatter.write_str("invalid Azure IoT text encoding"),
            Self::UnexpectedTopic => formatter.write_str("unexpected Azure IoT topic"),
            Self::MissingRequestId => formatter.write_str("Azure IoT request ID is missing"),
            Self::InvalidStatus => formatter.write_str("invalid Azure IoT response status"),
            Self::InvalidVersion => formatter.write_str("invalid Azure IoT property version"),
            Self::Mqtt(error) => write!(formatter, "invalid Azure IoT MQTT topic: {error}"),
        }
    }
}

impl core::error::Error for CodecError {}

impl From<MqttConfigError> for CodecError {
    fn from(value: MqttConfigError) -> Self {
        Self::Mqtt(value)
    }
}

/// One borrowed application property for a device-to-cloud message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageProperty<'a> {
    name: &'a str,
    value: &'a str,
}

impl<'a> MessageProperty<'a> {
    /// Validates a non-system application property.
    pub fn new(name: &'a str, value: &'a str) -> Result<Self, CodecError> {
        if name.is_empty() || name.starts_with('$') || name.contains('\0') {
            return Err(CodecError::InvalidPropertyName);
        }
        Ok(Self { name, value })
    }

    /// Returns the decoded property name.
    #[must_use]
    pub const fn name(self) -> &'a str {
        self.name
    }

    /// Returns the decoded property value.
    #[must_use]
    pub const fn value(self) -> &'a str {
        self.value
    }
}

/// Borrowed encoded property bag from an inbound Azure MQTT topic.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PropertyBag<'a> {
    encoded: &'a str,
}

impl<'a> PropertyBag<'a> {
    pub(crate) fn parse(encoded: &'a str) -> Result<Self, CodecError> {
        if encoded.starts_with('&') || encoded.ends_with('&') || encoded.contains("&&") {
            return Err(CodecError::InvalidPropertyName);
        }
        let bag = Self { encoded };
        for property in bag.iter() {
            property?;
        }
        Ok(bag)
    }

    /// Returns whether the message has no properties.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.encoded.is_empty()
    }

    /// Iterates through encoded properties without allocating.
    #[must_use]
    pub const fn iter(self) -> PropertyIter<'a> {
        PropertyIter {
            remaining: self.encoded,
        }
    }
}

/// One encoded inbound property.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodedProperty<'a> {
    name: &'a str,
    value: Option<&'a str>,
}

impl<'a> EncodedProperty<'a> {
    /// Returns the encoded name exactly as delivered by IoT Hub.
    #[must_use]
    pub const fn encoded_name(self) -> &'a str {
        self.name
    }

    /// Returns the encoded value, distinguishing absent and empty values.
    #[must_use]
    pub const fn encoded_value(self) -> Option<&'a str> {
        self.value
    }

    /// Decodes the property name into caller-owned storage.
    pub fn decode_name(self, output: &mut [u8]) -> Result<&str, CodecError> {
        decode_percent(self.name, output)
    }

    /// Decodes the property value into caller-owned storage.
    pub fn decode_value(self, output: &mut [u8]) -> Result<Option<&str>, CodecError> {
        self.value
            .map(|value| decode_percent(value, output))
            .transpose()
    }
}

/// Iterator over an encoded inbound Azure property bag.
#[derive(Clone, Copy, Debug)]
pub struct PropertyIter<'a> {
    remaining: &'a str,
}

impl<'a> Iterator for PropertyIter<'a> {
    type Item = Result<EncodedProperty<'a>, CodecError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        let (entry, remaining) = self
            .remaining
            .split_once('&')
            .unwrap_or((self.remaining, ""));
        self.remaining = remaining;
        let (name, value) = match entry.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (entry, None),
        };
        if name.is_empty()
            || !validate_percent_encoded(name)
            || value.is_some_and(|value| !validate_percent_encoded(value))
        {
            return Some(Err(if name.is_empty() {
                CodecError::InvalidPropertyName
            } else {
                CodecError::InvalidPercentEncoding
            }));
        }
        Some(Ok(EncodedProperty { name, value }))
    }
}

pub(crate) fn write_telemetry_topic(
    device_id: &str,
    content_type: Option<&str>,
    content_encoding: Option<&str>,
    properties: &[MessageProperty<'_>],
    output: &mut [u8],
) -> Result<TopicName, CodecError> {
    const PREFIX: &str = "devices/";
    const SUFFIX: &str = "/messages/events/";

    let mut required = PREFIX.len() + device_id.len() + SUFFIX.len();
    let mut count = 0_usize;
    for (name, value) in [("$.ct", content_type), ("$.ce", content_encoding)] {
        if let Some(value) = value {
            required = add_property_len(required, count, name, value)?;
            count += 1;
        }
    }
    for property in properties {
        required = add_property_len(required, count, property.name, property.value)?;
        count += 1;
    }
    if required > MAX_TOPIC_LEN {
        return Err(CodecError::Mqtt(MqttConfigError::TooLong));
    }
    if output.len() < required {
        return Err(CodecError::OutputTooSmall { required });
    }

    let mut offset = 0;
    for segment in [PREFIX, device_id, SUFFIX] {
        output[offset..offset + segment.len()].copy_from_slice(segment.as_bytes());
        offset += segment.len();
    }
    let mut written = 0_usize;
    for (name, value) in [("$.ct", content_type), ("$.ce", content_encoding)] {
        if let Some(value) = value {
            offset += write_property(output, offset, written, name, value);
            written += 1;
        }
    }
    for property in properties {
        offset += write_property(output, offset, written, property.name, property.value);
        written += 1;
    }
    let topic = str::from_utf8(&output[..offset]).map_err(|_| CodecError::InvalidEncoding)?;
    Ok(TopicName::new(topic)?)
}

fn add_property_len(
    current: usize,
    previous_count: usize,
    name: &str,
    value: &str,
) -> Result<usize, CodecError> {
    let name = percent_encoded_len(name).ok_or(CodecError::OutputTooSmall {
        required: usize::MAX,
    })?;
    let value = percent_encoded_len(value).ok_or(CodecError::OutputTooSmall {
        required: usize::MAX,
    })?;
    current
        .checked_add(usize::from(previous_count != 0))
        .and_then(|length| length.checked_add(name))
        .and_then(|length| length.checked_add(1))
        .and_then(|length| length.checked_add(value))
        .ok_or(CodecError::OutputTooSmall {
            required: usize::MAX,
        })
}

fn write_property(
    output: &mut [u8],
    mut offset: usize,
    previous_count: usize,
    name: &str,
    value: &str,
) -> usize {
    let start = offset;
    if previous_count != 0 {
        output[offset] = b'&';
        offset += 1;
    }
    offset += write_percent_encoded(output, offset, name);
    output[offset] = b'=';
    offset += 1;
    offset += write_percent_encoded(output, offset, value);
    offset - start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_bag_preserves_absent_empty_and_encoded_values() {
        let bag = PropertyBag::parse("null&empty=&message=a%20string").unwrap();
        let mut properties = bag.iter();
        assert_eq!(properties.next().unwrap().unwrap().encoded_value(), None);
        assert_eq!(
            properties.next().unwrap().unwrap().encoded_value(),
            Some("")
        );
        let message = properties.next().unwrap().unwrap();
        let mut output = [0; 16];
        assert_eq!(message.decode_value(&mut output).unwrap(), Some("a string"));
        assert!(properties.next().is_none());
    }

    #[test]
    fn rejects_reserved_application_property_names() {
        assert_eq!(
            MessageProperty::new("$.ct", "text/plain"),
            Err(CodecError::InvalidPropertyName)
        );
    }

    #[test]
    fn rejects_empty_property_entries_and_oversized_topics() {
        for encoded in ["&name=value", "name=value&", "one=1&&two=2"] {
            assert_eq!(
                PropertyBag::parse(encoded),
                Err(CodecError::InvalidPropertyName)
            );
        }

        let property = MessageProperty::new("name", "x").unwrap();
        let properties = [property; 40];
        let mut output = [0; 512];
        assert_eq!(
            write_telemetry_topic("sensor", None, None, &properties, &mut output),
            Err(CodecError::Mqtt(MqttConfigError::TooLong))
        );
    }
}
