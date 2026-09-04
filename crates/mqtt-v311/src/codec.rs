use core::str;

use embedded_sdk_mqtt::QoS;

use crate::{ConnectReturnCode, Error};

pub(crate) enum ControlPacket {
    Connack {
        session_present: bool,
        code: ConnectReturnCode,
    },
    Puback(u16),
    Suback {
        packet_id: u16,
        granted_qos: QoS,
    },
    Pingresp,
    Disconnect,
}

pub(crate) struct PublishPacket<'a> {
    pub(crate) topic: &'a str,
    pub(crate) payload: &'a [u8],
    pub(crate) qos: QoS,
    pub(crate) packet_id: Option<u16>,
    pub(crate) retained: bool,
}

pub(crate) fn encode_connect(
    client_id: &str,
    keep_alive: u16,
    clean_session: bool,
    credentials: Option<(&str, &[u8])>,
    output: &mut [u8],
) -> Result<usize, Error<core::convert::Infallible>> {
    let payload_len = string_len(client_id)?
        .checked_add(match credentials {
            Some((username, password)) => string_len(username)?
                .checked_add(binary_len(password)?)
                .ok_or(Error::Capacity)?,
            None => 0,
        })
        .ok_or(Error::Capacity)?;
    let remaining = 10_usize.checked_add(payload_len).ok_or(Error::Capacity)?;
    let mut writer = Writer::packet(output, 0x10, remaining)?;
    writer.bytes(b"\0\x04MQTT\x04")?;
    let mut flags = u8::from(clean_session) << 1;
    if credentials.is_some() {
        flags |= 0xc0;
    }
    writer.byte(flags)?;
    writer.u16(keep_alive)?;
    writer.string(client_id)?;
    if let Some((username, password)) = credentials {
        writer.string(username)?;
        writer.binary(password)?;
    }
    Ok(writer.len())
}

pub(crate) fn encode_publish(
    topic: &str,
    payload: &[u8],
    qos: QoS,
    packet_id: Option<u16>,
    duplicate: bool,
    output: &mut [u8],
) -> Result<usize, Error<core::convert::Infallible>> {
    let packet_id_len = usize::from(matches!(qos, QoS::AtLeastOnce)) * 2;
    if matches!(qos, QoS::AtLeastOnce) != packet_id.is_some() {
        return Err(Error::InvalidRequest);
    }
    let remaining = string_len(topic)?
        .checked_add(packet_id_len)
        .and_then(|value| value.checked_add(payload.len()))
        .ok_or(Error::Capacity)?;
    let header = 0x30 | (qos_bits(qos) << 1) | (u8::from(duplicate) << 3);
    let mut writer = Writer::packet(output, header, remaining)?;
    writer.string(topic)?;
    if let Some(packet_id) = packet_id {
        writer.packet_id(packet_id)?;
    }
    writer.bytes(payload)?;
    Ok(writer.len())
}

pub(crate) fn encode_subscribe(
    filter: &str,
    qos: QoS,
    packet_id: u16,
    output: &mut [u8],
) -> Result<usize, Error<core::convert::Infallible>> {
    let remaining = 2_usize
        .checked_add(string_len(filter)?)
        .and_then(|value| value.checked_add(1))
        .ok_or(Error::Capacity)?;
    let mut writer = Writer::packet(output, 0x82, remaining)?;
    writer.packet_id(packet_id)?;
    writer.string(filter)?;
    writer.byte(qos_bits(qos))?;
    Ok(writer.len())
}

pub(crate) fn encode_puback(
    packet_id: u16,
    output: &mut [u8],
) -> Result<usize, Error<core::convert::Infallible>> {
    let mut writer = Writer::packet(output, 0x40, 2)?;
    writer.packet_id(packet_id)?;
    Ok(writer.len())
}

pub(crate) fn encode_ping(output: &mut [u8]) -> Result<usize, Error<core::convert::Infallible>> {
    let writer = Writer::packet(output, 0xc0, 0)?;
    Ok(writer.len())
}

pub(crate) fn encode_disconnect(
    output: &mut [u8],
) -> Result<usize, Error<core::convert::Infallible>> {
    let writer = Writer::packet(output, 0xe0, 0)?;
    Ok(writer.len())
}

pub(crate) fn decode_control_packet(
    input: &[u8],
) -> Result<ControlPacket, Error<core::convert::Infallible>> {
    let frame = Frame::parse(input)?;
    match frame.header >> 4 {
        2 => decode_connack(frame),
        4 => decode_puback(frame),
        9 => decode_suback(frame),
        13 if frame.header == 0xd0 && frame.body.is_empty() => Ok(ControlPacket::Pingresp),
        14 if frame.header == 0xe0 && frame.body.is_empty() => Ok(ControlPacket::Disconnect),
        _ => Err(Error::UnexpectedPacket),
    }
}

pub(crate) fn decode_publish_packet(
    input: &[u8],
) -> Result<PublishPacket<'_>, Error<core::convert::Infallible>> {
    let frame = Frame::parse(input)?;
    if frame.header >> 4 != 3 {
        return Err(Error::UnexpectedPacket);
    }
    decode_publish(frame)
}

fn decode_connack(frame: Frame<'_>) -> Result<ControlPacket, Error<core::convert::Infallible>> {
    if frame.header != 0x20 || frame.body.len() != 2 || frame.body[0] > 1 {
        return Err(Error::MalformedPacket);
    }
    let code = ConnectReturnCode::from_wire(frame.body[1]).ok_or(Error::MalformedPacket)?;
    let session_present = frame.body[0] == 1;
    if session_present && code != ConnectReturnCode::Accepted {
        return Err(Error::MalformedPacket);
    }
    Ok(ControlPacket::Connack {
        session_present,
        code,
    })
}

fn decode_publish(frame: Frame<'_>) -> Result<PublishPacket<'_>, Error<core::convert::Infallible>> {
    let qos = match (frame.header >> 1) & 0x03 {
        0 => QoS::AtMostOnce,
        1 => QoS::AtLeastOnce,
        2 => return Err(Error::UnsupportedQos),
        _ => return Err(Error::MalformedPacket),
    };
    let mut cursor = 0;
    let topic = read_string(frame.body, &mut cursor)?;
    if topic.is_empty() || topic.contains(['\0', '+', '#']) {
        return Err(Error::MalformedPacket);
    }
    let packet_id = if qos == QoS::AtLeastOnce {
        Some(read_packet_id(frame.body, &mut cursor)?)
    } else {
        None
    };
    Ok(PublishPacket {
        topic,
        payload: &frame.body[cursor..],
        qos,
        packet_id,
        retained: frame.header & 1 != 0,
    })
}

fn decode_puback(frame: Frame<'_>) -> Result<ControlPacket, Error<core::convert::Infallible>> {
    if frame.header != 0x40 || frame.body.len() != 2 {
        return Err(Error::MalformedPacket);
    }
    let mut cursor = 0;
    Ok(ControlPacket::Puback(read_packet_id(
        frame.body,
        &mut cursor,
    )?))
}

fn decode_suback(frame: Frame<'_>) -> Result<ControlPacket, Error<core::convert::Infallible>> {
    if frame.header != 0x90 || frame.body.len() != 3 {
        return Err(Error::MalformedPacket);
    }
    let mut cursor = 0;
    let packet_id = read_packet_id(frame.body, &mut cursor)?;
    let granted_qos = match frame.body[cursor] {
        0 => QoS::AtMostOnce,
        1 => QoS::AtLeastOnce,
        2 => return Err(Error::UnsupportedQos),
        0x80 => return Err(Error::SubscriptionRejected),
        _ => return Err(Error::MalformedPacket),
    };
    Ok(ControlPacket::Suback {
        packet_id,
        granted_qos,
    })
}

fn read_string<'a>(
    input: &'a [u8],
    cursor: &mut usize,
) -> Result<&'a str, Error<core::convert::Infallible>> {
    let bytes = read_binary(input, cursor)?;
    str::from_utf8(bytes).map_err(|_| Error::MalformedPacket)
}

fn read_binary<'a>(
    input: &'a [u8],
    cursor: &mut usize,
) -> Result<&'a [u8], Error<core::convert::Infallible>> {
    let length_end = cursor.checked_add(2).ok_or(Error::MalformedPacket)?;
    let length_bytes = input
        .get(*cursor..length_end)
        .ok_or(Error::MalformedPacket)?;
    let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
    let end = length_end
        .checked_add(length)
        .ok_or(Error::MalformedPacket)?;
    let value = input.get(length_end..end).ok_or(Error::MalformedPacket)?;
    *cursor = end;
    Ok(value)
}

fn read_packet_id(
    input: &[u8],
    cursor: &mut usize,
) -> Result<u16, Error<core::convert::Infallible>> {
    let end = cursor.checked_add(2).ok_or(Error::MalformedPacket)?;
    let bytes = input.get(*cursor..end).ok_or(Error::MalformedPacket)?;
    let value = u16::from_be_bytes([bytes[0], bytes[1]]);
    if value == 0 {
        return Err(Error::MalformedPacket);
    }
    *cursor = end;
    Ok(value)
}

struct Frame<'a> {
    header: u8,
    body: &'a [u8],
}

impl<'a> Frame<'a> {
    fn parse(input: &'a [u8]) -> Result<Self, Error<core::convert::Infallible>> {
        let header = *input.first().ok_or(Error::MalformedPacket)?;
        let (remaining, header_len) = decode_remaining_length(input)?;
        let total = header_len
            .checked_add(remaining)
            .ok_or(Error::MalformedPacket)?;
        if total != input.len() {
            return Err(Error::MalformedPacket);
        }
        Ok(Self {
            header,
            body: &input[header_len..],
        })
    }
}

pub(crate) fn decode_remaining_length(
    input: &[u8],
) -> Result<(usize, usize), Error<core::convert::Infallible>> {
    let mut multiplier = 1_usize;
    let mut value = 0_usize;
    for index in 1..=4 {
        let byte = *input.get(index).ok_or(Error::MalformedPacket)?;
        value = value
            .checked_add(usize::from(byte & 0x7f) * multiplier)
            .ok_or(Error::MalformedPacket)?;
        if byte & 0x80 == 0 {
            if index > 1 && byte == 0 {
                return Err(Error::MalformedPacket);
            }
            return Ok((value, index + 1));
        }
        multiplier = multiplier.checked_mul(128).ok_or(Error::MalformedPacket)?;
    }
    Err(Error::MalformedPacket)
}

fn string_len(value: &str) -> Result<usize, Error<core::convert::Infallible>> {
    if value.is_empty() || value.contains('\0') || value.len() > usize::from(u16::MAX) {
        return Err(Error::InvalidRequest);
    }
    Ok(value.len() + 2)
}

fn binary_len(value: &[u8]) -> Result<usize, Error<core::convert::Infallible>> {
    if value.len() > usize::from(u16::MAX) {
        return Err(Error::InvalidRequest);
    }
    Ok(value.len() + 2)
}

const fn qos_bits(qos: QoS) -> u8 {
    match qos {
        QoS::AtMostOnce => 0,
        QoS::AtLeastOnce => 1,
    }
}

struct Writer<'a> {
    output: &'a mut [u8],
    cursor: usize,
}

impl<'a> Writer<'a> {
    fn packet(
        output: &'a mut [u8],
        header: u8,
        remaining: usize,
    ) -> Result<Self, Error<core::convert::Infallible>> {
        let header_len = 1 + remaining_length_len(remaining)?;
        let total = header_len.checked_add(remaining).ok_or(Error::Capacity)?;
        if output.len() < total {
            return Err(Error::Capacity);
        }
        let mut writer = Self { output, cursor: 0 };
        writer.byte(header)?;
        writer.remaining_length(remaining)?;
        Ok(writer)
    }

    const fn len(&self) -> usize {
        self.cursor
    }

    fn byte(&mut self, value: u8) -> Result<(), Error<core::convert::Infallible>> {
        let output = self.output.get_mut(self.cursor).ok_or(Error::Capacity)?;
        *output = value;
        self.cursor += 1;
        Ok(())
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), Error<core::convert::Infallible>> {
        let end = self
            .cursor
            .checked_add(value.len())
            .ok_or(Error::Capacity)?;
        let output = self
            .output
            .get_mut(self.cursor..end)
            .ok_or(Error::Capacity)?;
        output.copy_from_slice(value);
        self.cursor = end;
        Ok(())
    }

    fn u16(&mut self, value: u16) -> Result<(), Error<core::convert::Infallible>> {
        self.bytes(&value.to_be_bytes())
    }

    fn packet_id(&mut self, value: u16) -> Result<(), Error<core::convert::Infallible>> {
        if value == 0 {
            return Err(Error::InvalidRequest);
        }
        self.u16(value)
    }

    fn string(&mut self, value: &str) -> Result<(), Error<core::convert::Infallible>> {
        self.binary(value.as_bytes())
    }

    fn binary(&mut self, value: &[u8]) -> Result<(), Error<core::convert::Infallible>> {
        let length = u16::try_from(value.len()).map_err(|_| Error::InvalidRequest)?;
        self.u16(length)?;
        self.bytes(value)
    }

    fn remaining_length(
        &mut self,
        mut value: usize,
    ) -> Result<(), Error<core::convert::Infallible>> {
        loop {
            let mut byte = (value % 128) as u8;
            value /= 128;
            if value != 0 {
                byte |= 0x80;
            }
            self.byte(byte)?;
            if value == 0 {
                return Ok(());
            }
        }
    }
}

fn remaining_length_len(value: usize) -> Result<usize, Error<core::convert::Infallible>> {
    match value {
        0..=127 => Ok(1),
        128..=16_383 => Ok(2),
        16_384..=2_097_151 => Ok(3),
        2_097_152..=268_435_455 => Ok(4),
        _ => Err(Error::Capacity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_uses_mqtt_311_and_persistent_session() {
        let mut output = [0; 128];
        let len = encode_connect(
            "sensor-01",
            240,
            false,
            Some(("hub/sensor", b"token")),
            &mut output,
        )
        .unwrap();
        assert_eq!(&output[2..9], b"\0\x04MQTT\x04");
        assert_eq!(output[9], 0xc0);
        assert_eq!(len, usize::from(output[1]) + 2);
    }

    #[test]
    fn publish_and_subscribe_use_nonzero_packet_ids() {
        let mut output = [0; 128];
        let len = encode_publish(
            "devices/a/messages/events/",
            b"{}",
            QoS::AtLeastOnce,
            Some(42),
            false,
            &mut output,
        )
        .unwrap();
        assert_eq!(output[0], 0x32);
        let topic_end = 2 + 2 + "devices/a/messages/events/".len();
        assert_eq!(&output[topic_end..topic_end + 2], &[0, 42]);
        assert_eq!(len, usize::from(output[1]) + 2);

        let len = encode_subscribe(
            "devices/a/messages/devicebound/#",
            QoS::AtLeastOnce,
            7,
            &mut output,
        )
        .unwrap();
        assert_eq!(output[0], 0x82);
        assert_eq!(&output[2..4], &[0, 7]);
        assert_eq!(len, usize::from(output[1]) + 2);
    }

    #[test]
    fn decodes_service_packets_without_panicking_on_short_frames() {
        assert!(matches!(
            decode_control_packet(&[0x20, 0x02, 0x01, 0x00]).unwrap(),
            ControlPacket::Connack {
                session_present: true,
                code: ConnectReturnCode::Accepted
            }
        ));
        assert!(matches!(
            decode_control_packet(&[0x90, 0x03, 0x00, 0x09, 0x01]).unwrap(),
            ControlPacket::Suback {
                packet_id: 9,
                granted_qos: QoS::AtLeastOnce
            }
        ));
        for malformed in [
            &[0x20, 0x01, 0x00][..],
            &[0x40, 0x02, 0x00, 0x00],
            &[0x90, 0x03, 0x00, 0x01, 0x7f],
            &[0x30, 0x01, 0x00],
            &[0x20, 0x80, 0x00],
        ] {
            let decoded = if malformed[0] >> 4 == 3 {
                decode_publish_packet(malformed).map(|_| ())
            } else {
                decode_control_packet(malformed).map(|_| ())
            };
            assert!(decoded.is_err());
        }
    }

    #[test]
    fn decodes_borrowed_qos1_publication() {
        let packet = [
            0x33, 0x0d, 0x00, 0x07, b'd', b'e', b'v', b'i', b'c', b'e', b's', 0x00, 0x2a, b'{',
            b'}',
        ];
        let publication = decode_publish_packet(&packet).unwrap();
        assert_eq!(publication.topic, "devices");
        assert_eq!(publication.payload, b"{}");
        assert_eq!(publication.qos, QoS::AtLeastOnce);
        assert_eq!(publication.packet_id, Some(42));
        assert!(publication.retained);
    }
}
