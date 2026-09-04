//! Bounded framing for the XIAO hardware-in-the-loop serial fixture.

use embedded_sdk_provisioning::{MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, MAX_TRANSPORT_FRAME_BYTES};
use zeroize::Zeroize;

/// Bytes outside a frame before its payload and integrity value.
pub const SERIAL_FRAME_HEADER_BYTES: usize = 8;
/// Bytes added to a payload by the complete serial frame.
pub const SERIAL_FRAME_OVERHEAD_BYTES: usize = SERIAL_FRAME_HEADER_BYTES + 4;

const MAGIC: [u8; 4] = [0, b'P', b'R', b'V'];
const VERSION: u8 = 1;
const REQUEST_KIND: u8 = 0;
const RESPONSE_KIND: u8 = 1;

const _: () = assert!(MAX_REQUEST_BYTES + SERIAL_FRAME_OVERHEAD_BYTES <= MAX_TRANSPORT_FRAME_BYTES);

/// Direction encoded into a fixture serial frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialFrameKind {
    /// Request sent from the fixture to the device.
    Request,
    /// Response sent from the device to the fixture.
    Response,
}

impl SerialFrameKind {
    const fn encoded(self) -> u8 {
        match self {
            Self::Request => REQUEST_KIND,
            Self::Response => RESPONSE_KIND,
        }
    }

    const fn decode(value: u8) -> Option<Self> {
        match value {
            REQUEST_KIND => Some(Self::Request),
            RESPONSE_KIND => Some(Self::Response),
            _ => None,
        }
    }

    const fn max_payload(self) -> usize {
        match self {
            Self::Request => MAX_REQUEST_BYTES,
            Self::Response => MAX_RESPONSE_BYTES,
        }
    }
}

/// One complete frame borrowing the decoder's bounded storage.
///
/// This type deliberately implements neither `Debug` nor `Display` because a
/// request payload can contain credentials.
pub struct SerialFrame<'a> {
    kind: SerialFrameKind,
    payload: &'a [u8],
}

impl<'a> SerialFrame<'a> {
    /// Returns whether this is a request or response frame.
    #[must_use]
    pub const fn kind(&self) -> SerialFrameKind {
        self.kind
    }

    /// Returns the complete CBOR envelope carried by this frame.
    #[must_use]
    pub const fn payload(&self) -> &'a [u8] {
        self.payload
    }
}

/// Redacted framing failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SerialFrameError {
    /// The frame version or direction is not supported.
    UnsupportedHeader,
    /// The declared payload exceeds the bound for its direction.
    PayloadTooLarge,
    /// The destination cannot hold the complete encoded frame.
    OutputTooSmall,
    /// The complete frame exceeds the fixed transport budget.
    FrameTooLarge,
    /// The received integrity value does not match the frame bytes.
    Integrity,
    /// A complete frame must be consumed and cleared before more input.
    FramePending,
}

/// Incremental, resynchronizing decoder for a mixed diagnostic/frame stream.
///
/// Ordinary printable diagnostics are ignored until the NUL-prefixed magic is
/// found. Call [`Self::clear`] after consuming a returned frame or after an
/// inter-byte timeout. Clearing and dropping the decoder erase its entire
/// credential-bearing buffer.
pub struct SerialFrameDecoder {
    buffer: [u8; MAX_TRANSPORT_FRAME_BYTES],
    received: usize,
    expected: Option<usize>,
    complete: bool,
}

impl SerialFrameDecoder {
    /// Creates an empty decoder searching for the frame magic.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: [0; MAX_TRANSPORT_FRAME_BYTES],
            received: 0,
            expected: None,
            complete: false,
        }
    }

    /// Returns whether a partial frame is waiting for more bytes.
    #[must_use]
    pub const fn is_receiving(&self) -> bool {
        self.received != 0 && !self.complete
    }

    /// Pushes one stream byte and returns a complete verified frame when ready.
    pub fn push(&mut self, byte: u8) -> Result<Option<SerialFrame<'_>>, SerialFrameError> {
        if self.complete {
            return Err(SerialFrameError::FramePending);
        }

        if self.received < MAGIC.len() {
            self.seek_magic(byte);
            return Ok(None);
        }

        self.buffer[self.received] = byte;
        self.received += 1;

        if self.received == SERIAL_FRAME_HEADER_BYTES {
            let kind = match self.header_kind() {
                Ok(kind) => kind,
                Err(error) => {
                    self.clear_and_seed(byte);
                    return Err(error);
                }
            };
            let payload_len = usize::from(u16::from_be_bytes([self.buffer[6], self.buffer[7]]));
            if payload_len > kind.max_payload() {
                self.clear_and_seed(byte);
                return Err(SerialFrameError::PayloadTooLarge);
            }
            let expected = SERIAL_FRAME_OVERHEAD_BYTES
                .checked_add(payload_len)
                .ok_or(SerialFrameError::FrameTooLarge)?;
            if expected > MAX_TRANSPORT_FRAME_BYTES {
                self.clear_and_seed(byte);
                return Err(SerialFrameError::FrameTooLarge);
            }
            self.expected = Some(expected);
        }

        let Some(expected) = self.expected else {
            return Ok(None);
        };
        if self.received < expected {
            return Ok(None);
        }

        let crc_offset = expected - 4;
        let expected_crc = u32::from_be_bytes([
            self.buffer[crc_offset],
            self.buffer[crc_offset + 1],
            self.buffer[crc_offset + 2],
            self.buffer[crc_offset + 3],
        ]);
        if crc32(&self.buffer[..crc_offset]) != expected_crc {
            self.clear_and_seed(byte);
            return Err(SerialFrameError::Integrity);
        }

        self.complete = true;
        let kind = self.header_kind()?;
        Ok(Some(SerialFrame {
            kind,
            payload: &self.buffer[SERIAL_FRAME_HEADER_BYTES..crc_offset],
        }))
    }

    /// Erases buffered bytes and resumes searching for a new frame.
    pub fn clear(&mut self) {
        self.buffer.zeroize();
        self.received = 0;
        self.expected = None;
        self.complete = false;
    }

    fn seek_magic(&mut self, byte: u8) {
        if byte == MAGIC[self.received] {
            self.buffer[self.received] = byte;
            self.received += 1;
        } else if byte == MAGIC[0] {
            self.buffer[..self.received].zeroize();
            self.buffer[0] = byte;
            self.received = 1;
        } else {
            self.buffer[..self.received].zeroize();
            self.received = 0;
        }
    }

    fn header_kind(&self) -> Result<SerialFrameKind, SerialFrameError> {
        if self.buffer[4] != VERSION {
            return Err(SerialFrameError::UnsupportedHeader);
        }
        SerialFrameKind::decode(self.buffer[5]).ok_or(SerialFrameError::UnsupportedHeader)
    }

    fn clear_and_seed(&mut self, byte: u8) {
        self.clear();
        if byte == MAGIC[0] {
            self.buffer[0] = byte;
            self.received = 1;
        }
    }
}

impl Default for SerialFrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SerialFrameDecoder {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Encodes one CBOR envelope into a complete fixture serial frame.
pub fn encode_serial_frame(
    kind: SerialFrameKind,
    payload: &[u8],
    output: &mut [u8],
) -> Result<usize, SerialFrameError> {
    if payload.len() > kind.max_payload() {
        return Err(SerialFrameError::PayloadTooLarge);
    }
    let len = SERIAL_FRAME_OVERHEAD_BYTES
        .checked_add(payload.len())
        .ok_or(SerialFrameError::FrameTooLarge)?;
    if len > MAX_TRANSPORT_FRAME_BYTES {
        return Err(SerialFrameError::FrameTooLarge);
    }
    if output.len() < len {
        return Err(SerialFrameError::OutputTooSmall);
    }

    output[..4].copy_from_slice(&MAGIC);
    output[4] = VERSION;
    output[5] = kind.encoded();
    output[6..8].copy_from_slice(&(payload.len() as u16).to_be_bytes());
    output[SERIAL_FRAME_HEADER_BYTES..SERIAL_FRAME_HEADER_BYTES + payload.len()]
        .copy_from_slice(payload);
    let crc_offset = len - 4;
    let crc = crc32(&output[..crc_offset]);
    output[crc_offset..len].copy_from_slice(&crc.to_be_bytes());
    Ok(len)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use embedded_sdk_provisioning::{
        MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, MAX_TRANSPORT_FRAME_BYTES,
    };

    use super::{
        SERIAL_FRAME_OVERHEAD_BYTES, SerialFrameDecoder, SerialFrameError, SerialFrameKind,
        encode_serial_frame,
    };

    fn decode<'a>(decoder: &'a mut SerialFrameDecoder, encoded: &[u8]) -> super::SerialFrame<'a> {
        let (&last, prefix) = encoded.split_last().expect("nonempty encoded frame");
        for &byte in prefix {
            assert!(decoder.push(byte).unwrap().is_none());
        }
        decoder.push(last).unwrap().expect("complete frame")
    }

    #[test]
    fn maximum_request_round_trips_inside_transport_budget() {
        let payload = [0xa5; MAX_REQUEST_BYTES];
        let mut encoded = [0; MAX_TRANSPORT_FRAME_BYTES];
        let len = encode_serial_frame(SerialFrameKind::Request, &payload, &mut encoded).unwrap();
        assert_eq!(len, MAX_REQUEST_BYTES + SERIAL_FRAME_OVERHEAD_BYTES);

        let mut decoder = SerialFrameDecoder::new();
        let frame = decode(&mut decoder, &encoded[..len]);
        assert_eq!(frame.kind(), SerialFrameKind::Request);
        assert_eq!(frame.payload(), payload);
    }

    #[test]
    fn response_bound_is_direction_specific() {
        let payload = [0x5a; MAX_RESPONSE_BYTES];
        let mut encoded = [0; MAX_TRANSPORT_FRAME_BYTES];
        let len = encode_serial_frame(SerialFrameKind::Response, &payload, &mut encoded).unwrap();
        let mut decoder = SerialFrameDecoder::new();
        let frame = decode(&mut decoder, &encoded[..len]);
        assert_eq!(frame.kind(), SerialFrameKind::Response);
        assert_eq!(frame.payload(), payload);

        decoder.clear();
        assert_eq!(
            encode_serial_frame(
                SerialFrameKind::Response,
                &[0; MAX_RESPONSE_BYTES + 1],
                &mut encoded,
            ),
            Err(SerialFrameError::PayloadTooLarge)
        );
    }

    #[test]
    fn printable_diagnostics_are_ignored_before_magic() {
        let mut encoded = [0; 64];
        let len = encode_serial_frame(SerialFrameKind::Request, b"request", &mut encoded).unwrap();
        let mut decoder = SerialFrameDecoder::new();
        for &byte in b"embedded-sdk boot\r\n" {
            assert!(decoder.push(byte).unwrap().is_none());
        }
        let frame = decode(&mut decoder, &encoded[..len]);
        assert_eq!(frame.payload(), b"request");
    }

    #[test]
    fn corrupted_frame_is_rejected_and_next_frame_resynchronizes() {
        let mut corrupted = [0; 64];
        let corrupted_len =
            encode_serial_frame(SerialFrameKind::Request, b"secret", &mut corrupted).unwrap();
        corrupted[8] ^= 1;

        let mut valid = [0; 64];
        let valid_len = encode_serial_frame(SerialFrameKind::Request, b"next", &mut valid).unwrap();
        let mut decoder = SerialFrameDecoder::new();
        let mut error = None;
        for &byte in &corrupted[..corrupted_len] {
            if let Err(actual) = decoder.push(byte) {
                error = Some(actual);
            }
        }
        assert_eq!(error, Some(SerialFrameError::Integrity));
        let frame = decode(&mut decoder, &valid[..valid_len]);
        assert_eq!(frame.payload(), b"next");
    }

    #[test]
    fn complete_frame_must_be_cleared_before_reuse() {
        let mut encoded = [0; 64];
        let len = encode_serial_frame(SerialFrameKind::Request, b"request", &mut encoded).unwrap();
        let mut decoder = SerialFrameDecoder::new();
        let _ = decode(&mut decoder, &encoded[..len]);
        assert!(matches!(
            decoder.push(0),
            Err(SerialFrameError::FramePending)
        ));
        decoder.clear();
        assert!(decoder.push(0).unwrap().is_none());
    }

    #[test]
    fn oversized_declared_response_resynchronizes() {
        let oversized = MAX_RESPONSE_BYTES + 1;
        let header = [
            0,
            b'P',
            b'R',
            b'V',
            1,
            1,
            (oversized >> 8) as u8,
            oversized as u8,
        ];
        let mut decoder = SerialFrameDecoder::new();
        for &byte in &header[..header.len() - 1] {
            assert!(decoder.push(byte).unwrap().is_none());
        }
        assert!(matches!(
            decoder.push(header[header.len() - 1]),
            Err(SerialFrameError::PayloadTooLarge)
        ));

        let mut valid = [0; 64];
        let valid_len = encode_serial_frame(SerialFrameKind::Request, b"ok", &mut valid).unwrap();
        let frame = decode(&mut decoder, &valid[..valid_len]);
        assert_eq!(frame.payload(), b"ok");
    }

    #[test]
    fn every_truncated_prefix_remains_incomplete_and_clearable() {
        let mut encoded = [0; 64];
        let len = encode_serial_frame(
            SerialFrameKind::Request,
            b"credential-bearing-request",
            &mut encoded,
        )
        .unwrap();

        for prefix_len in 0..len {
            let mut decoder = SerialFrameDecoder::new();
            for &byte in &encoded[..prefix_len] {
                assert!(decoder.push(byte).unwrap().is_none());
            }
            decoder.clear();
            assert!(!decoder.is_receiving());
        }
    }

    #[test]
    fn deterministic_arbitrary_streams_remain_bounded() {
        let mut decoder = SerialFrameDecoder::new();
        let mut state = 0x6d2b_79f5_u32;
        for _ in 0..65_536 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let result = decoder.push(state as u8);
            if let Ok(Some(_)) = result {
                decoder.clear();
            }
        }
        decoder.clear();
        assert!(!decoder.is_receiving());
    }
}
