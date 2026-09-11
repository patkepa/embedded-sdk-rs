use core::str;

use embedded_sdk_mqtt::{TopicFilter, TopicName};

use crate::{
    CodecError, RequestId,
    encode::{
        decimal_len, parse_decimal_u16, parse_decimal_u64, query_value, validate_percent_encoded,
        write_decimal,
    },
};

const RESPONSE_PREFIX: &str = "$iothub/twin/res/";
const DESIRED_PREFIX: &str = "$iothub/twin/PATCH/properties/desired/?";

/// Borrowed response to a twin GET or reported-properties PATCH.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TwinResponse<'a> {
    status: u16,
    request_id: &'a str,
    version: Option<u64>,
    payload: &'a [u8],
}

impl<'a> TwinResponse<'a> {
    /// Returns the Azure service status.
    #[must_use]
    pub const fn status(self) -> u16 {
        self.status
    }

    /// Returns the encoded response request identifier.
    #[must_use]
    pub const fn request_id(self) -> &'a str {
        self.request_id
    }

    /// Returns the reported-properties version when supplied by the service.
    #[must_use]
    pub const fn version(self) -> Option<u64> {
        self.version
    }

    /// Returns the JSON or empty response payload.
    #[must_use]
    pub const fn payload(self) -> &'a [u8] {
        self.payload
    }
}

/// Borrowed desired-properties patch and its service version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesiredPropertiesPatch<'a> {
    version: u64,
    payload: &'a [u8],
}

impl<'a> DesiredPropertiesPatch<'a> {
    /// Returns the desired-properties version.
    #[must_use]
    pub const fn version(self) -> u64 {
        self.version
    }

    /// Returns the JSON patch payload.
    #[must_use]
    pub const fn payload(self) -> &'a [u8] {
        self.payload
    }
}

/// Returns the IoT Hub twin response subscription.
pub fn twin_response_filter() -> Result<TopicFilter, CodecError> {
    Ok(TopicFilter::new("$iothub/twin/res/#")?)
}

/// Returns the IoT Hub desired-properties patch subscription.
pub fn desired_properties_filter() -> Result<TopicFilter, CodecError> {
    Ok(TopicFilter::new("$iothub/twin/PATCH/properties/desired/#")?)
}

/// Builds a complete-twin GET topic.
pub fn twin_get_topic(request_id: RequestId, output: &mut [u8]) -> Result<TopicName, CodecError> {
    request_topic("$iothub/twin/GET/?$rid=", request_id, output)
}

/// Builds a reported-properties PATCH topic.
pub fn reported_properties_topic(
    request_id: RequestId,
    output: &mut [u8],
) -> Result<TopicName, CodecError> {
    request_topic(
        "$iothub/twin/PATCH/properties/reported/?$rid=",
        request_id,
        output,
    )
}

/// Parses a twin operation response.
pub fn parse_twin_response<'a>(
    topic: &'a str,
    payload: &'a [u8],
) -> Result<TwinResponse<'a>, CodecError> {
    let operation = topic
        .strip_prefix(RESPONSE_PREFIX)
        .ok_or(CodecError::UnexpectedTopic)?;
    let (status, query) = operation
        .split_once("/?")
        .ok_or(CodecError::UnexpectedTopic)?;
    let status = parse_decimal_u16(status).ok_or(CodecError::InvalidStatus)?;
    let request_id = query_value(query, "$rid")
        .filter(|value| !value.is_empty())
        .ok_or(CodecError::MissingRequestId)?;
    if !validate_percent_encoded(request_id) {
        return Err(CodecError::InvalidPercentEncoding);
    }
    let version = query_value(query, "$version")
        .map(|value| parse_decimal_u64(value).ok_or(CodecError::InvalidVersion))
        .transpose()?;
    Ok(TwinResponse {
        status,
        request_id,
        version,
        payload,
    })
}

/// Parses an online desired-properties patch notification.
pub fn parse_desired_properties_patch<'a>(
    topic: &str,
    payload: &'a [u8],
) -> Result<DesiredPropertiesPatch<'a>, CodecError> {
    let query = topic
        .strip_prefix(DESIRED_PREFIX)
        .ok_or(CodecError::UnexpectedTopic)?;
    let version = query_value(query, "$version")
        .and_then(parse_decimal_u64)
        .ok_or(CodecError::InvalidVersion)?;
    Ok(DesiredPropertiesPatch { version, payload })
}

fn request_topic(
    prefix: &str,
    request_id: RequestId,
    output: &mut [u8],
) -> Result<TopicName, CodecError> {
    let id_len = decimal_len(u64::from(request_id.get()));
    let required = prefix.len() + id_len;
    if output.len() < required {
        return Err(CodecError::OutputTooSmall { required });
    }
    output[..prefix.len()].copy_from_slice(prefix.as_bytes());
    write_decimal(output, prefix.len(), u64::from(request_id.get()));
    let topic = str::from_utf8(&output[..required]).map_err(|_| CodecError::InvalidEncoding)?;
    Ok(TopicName::new(topic)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_correlates_twin_topics() {
        let request_id = RequestId::new(42);
        let mut output = [0; 64];
        assert_eq!(
            twin_get_topic(request_id, &mut output).unwrap().as_str(),
            "$iothub/twin/GET/?$rid=42"
        );
        assert_eq!(
            reported_properties_topic(request_id, &mut output)
                .unwrap()
                .as_str(),
            "$iothub/twin/PATCH/properties/reported/?$rid=42"
        );

        let response =
            parse_twin_response("$iothub/twin/res/204/?$rid=42&$version=6", b"").unwrap();
        assert_eq!(response.status(), 204);
        assert!(request_id.matches(response.request_id()));
        assert_eq!(response.version(), Some(6));
    }

    #[test]
    fn parses_desired_patch_and_rejects_bad_versions() {
        let patch = parse_desired_properties_patch(
            "$iothub/twin/PATCH/properties/desired/?$version=8",
            br#"{"interval":5}"#,
        )
        .unwrap();
        assert_eq!(patch.version(), 8);
        assert_eq!(patch.payload(), br#"{"interval":5}"#);
        assert_eq!(
            parse_desired_properties_patch(
                "$iothub/twin/PATCH/properties/desired/?$version=bad",
                b""
            ),
            Err(CodecError::InvalidVersion)
        );
        assert_eq!(
            parse_twin_response("$iothub/twin/res/200/?$rid=bad%2", b""),
            Err(CodecError::InvalidPercentEncoding)
        );
    }
}
