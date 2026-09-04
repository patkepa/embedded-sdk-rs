use core::{fmt, str};

use embedded_sdk_mqtt::{ConfigError as MqttConfigError, MAX_TOPIC_LEN, TopicFilter, TopicName};

use crate::{
    CodecError,
    encode::{decimal_len, query_value, validate_percent_encoded, write_decimal},
};

const REQUEST_PREFIX: &str = "$iothub/methods/POST/";
const RESPONSE_PREFIX: &str = "$iothub/methods/res/";
const REQUEST_ID_PARAMETER: &str = "$rid";
const RESPONSE_SEPARATOR: &str = "/?$rid=";

/// Largest direct-method request identifier that can always be echoed in an
/// MQTT response topic with any `u16` application status.
pub const MAX_METHOD_REQUEST_ID_LEN: usize =
    MAX_TOPIC_LEN - RESPONSE_PREFIX.len() - 5 - RESPONSE_SEPARATOR.len();

/// Owned direct-method correlation identifier for crossing receive-buffer boundaries.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct MethodRequestId {
    bytes: [u8; MAX_METHOD_REQUEST_ID_LEN],
    len: u16,
}

impl MethodRequestId {
    /// Validates and copies the encoded request identifier.
    pub fn new(value: &str) -> Result<Self, CodecError> {
        if value.is_empty() {
            return Err(CodecError::MissingRequestId);
        }
        if !validate_percent_encoded(value) {
            return Err(CodecError::InvalidPercentEncoding);
        }
        if value.len() > MAX_METHOD_REQUEST_ID_LEN {
            return Err(CodecError::Mqtt(MqttConfigError::TooLong));
        }
        let mut bytes = [0; MAX_METHOD_REQUEST_ID_LEN];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self {
            bytes,
            len: value.len() as u16,
        })
    }

    /// Returns the encoded identifier exactly as IoT Hub supplied it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }
}

impl TryFrom<&str> for MethodRequestId {
    type Error = CodecError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Debug for MethodRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("MethodRequestId")
            .field(&self.as_str())
            .finish()
    }
}

impl fmt::Display for MethodRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Borrowed Azure IoT Hub direct-method invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectMethodRequest<'a> {
    method_name: &'a str,
    request_id: &'a str,
    payload: &'a [u8],
}

impl<'a> DirectMethodRequest<'a> {
    /// Returns the method name from the MQTT topic.
    #[must_use]
    pub const fn method_name(self) -> &'a str {
        self.method_name
    }

    /// Returns the encoded request identifier to echo in the response topic.
    #[must_use]
    pub const fn request_id(self) -> &'a str {
        self.request_id
    }

    /// Copies the correlation identifier so it can outlive the MQTT receive buffer.
    pub fn owned_request_id(self) -> Result<MethodRequestId, CodecError> {
        MethodRequestId::new(self.request_id)
    }

    /// Returns the JSON or empty request payload.
    #[must_use]
    pub const fn payload(self) -> &'a [u8] {
        self.payload
    }
}

/// Returns the IoT Hub direct-method request subscription.
pub fn direct_method_filter() -> Result<TopicFilter, CodecError> {
    Ok(TopicFilter::new("$iothub/methods/POST/#")?)
}

/// Parses a direct-method request topic and borrows its payload.
pub fn parse_direct_method<'a>(
    topic: &'a str,
    payload: &'a [u8],
) -> Result<DirectMethodRequest<'a>, CodecError> {
    let operation = topic
        .strip_prefix(REQUEST_PREFIX)
        .ok_or(CodecError::UnexpectedTopic)?;
    let (method_name, query) = operation
        .split_once("/?")
        .ok_or(CodecError::UnexpectedTopic)?;
    if method_name.is_empty() || method_name.contains('/') {
        return Err(CodecError::UnexpectedTopic);
    }
    if !validate_percent_encoded(method_name) {
        return Err(CodecError::InvalidPercentEncoding);
    }
    let request_id = query_value(query, REQUEST_ID_PARAMETER)
        .filter(|value| !value.is_empty())
        .ok_or(CodecError::MissingRequestId)?;
    if !validate_percent_encoded(request_id) {
        return Err(CodecError::InvalidPercentEncoding);
    }
    Ok(DirectMethodRequest {
        method_name,
        request_id,
        payload,
    })
}

/// Builds the response topic for a direct-method invocation.
pub fn direct_method_response_topic(
    request_id: &str,
    status: u16,
    output: &mut [u8],
) -> Result<TopicName, CodecError> {
    if request_id.is_empty() {
        return Err(CodecError::MissingRequestId);
    }
    if !validate_percent_encoded(request_id) {
        return Err(CodecError::InvalidPercentEncoding);
    }
    let status_len = decimal_len(u64::from(status));
    let required = RESPONSE_PREFIX.len() + status_len + RESPONSE_SEPARATOR.len() + request_id.len();
    if required > MAX_TOPIC_LEN {
        return Err(CodecError::Mqtt(MqttConfigError::TooLong));
    }
    if output.len() < required {
        return Err(CodecError::OutputTooSmall { required });
    }

    output[..RESPONSE_PREFIX.len()].copy_from_slice(RESPONSE_PREFIX.as_bytes());
    let mut offset = RESPONSE_PREFIX.len();
    offset += write_decimal(output, offset, u64::from(status));
    output[offset..offset + RESPONSE_SEPARATOR.len()]
        .copy_from_slice(RESPONSE_SEPARATOR.as_bytes());
    offset += RESPONSE_SEPARATOR.len();
    output[offset..offset + request_id.len()].copy_from_slice(request_id.as_bytes());
    let topic = str::from_utf8(&output[..required]).map_err(|_| CodecError::InvalidEncoding)?;
    Ok(TopicName::new(topic)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_and_echoes_request_id_in_response() {
        let request = parse_direct_method(
            "$iothub/methods/POST/reboot/?$rid=req%201",
            br#"{"delay":1}"#,
        )
        .unwrap();
        assert_eq!(request.method_name(), "reboot");
        assert_eq!(request.request_id(), "req%201");
        assert_eq!(request.owned_request_id().unwrap().as_str(), "req%201");
        assert_eq!(request.payload(), br#"{"delay":1}"#);

        let mut output = [0; 64];
        assert_eq!(
            direct_method_response_topic(request.request_id(), 204, &mut output)
                .unwrap()
                .as_str(),
            "$iothub/methods/res/204/?$rid=req%201"
        );
    }

    #[test]
    fn rejects_missing_request_id_and_small_output() {
        assert_eq!(
            parse_direct_method("$iothub/methods/POST/reboot/", b""),
            Err(CodecError::UnexpectedTopic)
        );
        let mut output = [0; 4];
        assert!(matches!(
            direct_method_response_topic("1", 500, &mut output),
            Err(CodecError::OutputTooSmall { .. })
        ));
        assert_eq!(
            parse_direct_method("$iothub/methods/POST/reboot/?$rid=bad%2", b""),
            Err(CodecError::InvalidPercentEncoding)
        );
        assert_eq!(
            parse_direct_method("$iothub/methods/POST/a/b/?$rid=1", b""),
            Err(CodecError::UnexpectedTopic)
        );

        let oversized = [b'a'; MAX_METHOD_REQUEST_ID_LEN + 1];
        assert_eq!(
            MethodRequestId::new(str::from_utf8(&oversized).unwrap()),
            Err(CodecError::Mqtt(MqttConfigError::TooLong))
        );

        let maximum = [b'a'; MAX_METHOD_REQUEST_ID_LEN];
        let request_id = MethodRequestId::new(str::from_utf8(&maximum).unwrap()).unwrap();
        let mut maximum_topic = [0; MAX_TOPIC_LEN];
        assert_eq!(
            direct_method_response_topic(request_id.as_str(), u16::MAX, &mut maximum_topic)
                .unwrap()
                .len(),
            MAX_TOPIC_LEN
        );
    }
}
