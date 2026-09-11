use core::str;

use crate::CodecError;

pub(crate) fn write_segments<'a>(
    output: &'a mut [u8],
    segments: &[&str],
) -> Result<&'a str, CodecError> {
    let required = segments
        .iter()
        .try_fold(0_usize, |length, segment| length.checked_add(segment.len()))
        .ok_or(CodecError::OutputTooSmall {
            required: usize::MAX,
        })?;
    if output.len() < required {
        return Err(CodecError::OutputTooSmall { required });
    }

    let mut offset = 0;
    for segment in segments {
        let end = offset + segment.len();
        output[offset..end].copy_from_slice(segment.as_bytes());
        offset = end;
    }
    str::from_utf8(&output[..required]).map_err(|_| CodecError::InvalidEncoding)
}

pub(crate) const fn decimal_len(mut value: u64) -> usize {
    let mut len = 1;
    while value >= 10 {
        value /= 10;
        len += 1;
    }
    len
}

pub(crate) fn write_decimal(output: &mut [u8], offset: usize, mut value: u64) -> usize {
    let len = decimal_len(value);
    let mut cursor = offset + len;
    while cursor > offset {
        cursor -= 1;
        output[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    len
}

pub(crate) fn parse_decimal_u16(value: &str) -> Option<u16> {
    parse_decimal_u64(value).and_then(|value| u16::try_from(value).ok())
}

pub(crate) fn parse_decimal_u32(value: &str) -> Option<u32> {
    parse_decimal_u64(value).and_then(|value| u32::try_from(value).ok())
}

pub(crate) fn parse_decimal_u64(value: &str) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    value.bytes().try_fold(0_u64, |current, byte| {
        if !byte.is_ascii_digit() {
            return None;
        }
        current
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
    })
}

pub(crate) const fn percent_encoded_len(value: &str) -> Option<usize> {
    let bytes = value.as_bytes();
    let mut index = 0;
    let mut length = 0_usize;
    while index < bytes.len() {
        let width = if is_property_safe(bytes[index]) { 1 } else { 3 };
        match length.checked_add(width) {
            Some(next) => length = next,
            None => return None,
        }
        index += 1;
    }
    Some(length)
}

pub(crate) fn write_percent_encoded(output: &mut [u8], mut offset: usize, value: &str) -> usize {
    let start = offset;
    for byte in value.bytes() {
        if is_property_safe(byte) {
            output[offset] = byte;
            offset += 1;
        } else {
            output[offset] = b'%';
            output[offset + 1] = hex_digit(byte >> 4);
            output[offset + 2] = hex_digit(byte & 0x0f);
            offset += 3;
        }
    }
    offset - start
}

pub(crate) fn validate_percent_encoded(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || hex_value(bytes[index + 1]).is_none()
                || hex_value(bytes[index + 2]).is_none()
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

pub(crate) fn percent_decoded_len(value: &str) -> Option<usize> {
    if !validate_percent_encoded(value) {
        return None;
    }
    let mut length = 0;
    let mut index = 0;
    let bytes = value.as_bytes();
    while index < bytes.len() {
        index += if bytes[index] == b'%' { 3 } else { 1 };
        length += 1;
    }
    Some(length)
}

pub(crate) fn decode_percent<'a>(value: &str, output: &'a mut [u8]) -> Result<&'a str, CodecError> {
    let required = percent_decoded_len(value).ok_or(CodecError::InvalidPercentEncoding)?;
    if output.len() < required {
        return Err(CodecError::OutputTooSmall { required });
    }

    let bytes = value.as_bytes();
    let mut input = 0;
    let mut written = 0;
    while input < bytes.len() {
        if bytes[input] == b'%' {
            let high = hex_value(bytes[input + 1]).ok_or(CodecError::InvalidPercentEncoding)?;
            let low = hex_value(bytes[input + 2]).ok_or(CodecError::InvalidPercentEncoding)?;
            output[written] = high << 4 | low;
            input += 3;
        } else {
            output[written] = bytes[input];
            input += 1;
        }
        written += 1;
    }
    str::from_utf8(&output[..written]).map_err(|_| CodecError::InvalidEncoding)
}

pub(crate) fn query_value<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|entry| {
        let (key, value) = entry.split_once('=')?;
        (key == name).then_some(value)
    })
}

const fn is_property_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' | b'$'
        )
}

const fn hex_digit(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        _ => b'A' + value - 10,
    }
}

const fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_matches_azure_property_examples() {
        let mut output = [0; 64];
        let written = write_percent_encoded(&mut output, 0, "application/json; charset=utf-8");
        assert_eq!(
            str::from_utf8(&output[..written]).unwrap(),
            "application%2Fjson%3B%20charset%3Dutf-8"
        );

        let mut decoded = [0; 32];
        assert_eq!(
            decode_percent("a%20string", &mut decoded).unwrap(),
            "a string"
        );
    }

    #[test]
    fn decode_rejects_invalid_or_too_small_output_without_panicking() {
        let mut output = [0; 2];
        assert_eq!(
            decode_percent("bad%2", &mut output),
            Err(CodecError::InvalidPercentEncoding)
        );
        assert_eq!(
            decode_percent("three", &mut output),
            Err(CodecError::OutputTooSmall { required: 5 })
        );
    }
}
