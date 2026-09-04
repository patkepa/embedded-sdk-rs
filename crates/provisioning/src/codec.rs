//! Bounded version-1 CBOR request and response codec.

use embedded_sdk_config::SchemaVersion;
use minicbor::{Decoder, Encoder, encode::write::Cursor};

use crate::{
    Capabilities, CommitDisposition, DeviceState, ErrorKind as ServiceErrorKind, Generation,
    MAX_CANDIDATE_BYTES, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, PROTOCOL_VERSION_MAJOR,
    PROTOCOL_VERSION_MINOR, RecoveryReason, RejectionReason, Request, RequestId, Response,
    ResponseKind, ServiceError, SessionId, Status, TransactionId, TransactionState,
};

const PROTOCOL_MAGIC: u32 = u32::from_be_bytes(*b"PRV1");

const FIELD_MAGIC: u8 = 0;
const FIELD_VERSION_MAJOR: u8 = 1;
const FIELD_VERSION_MINOR: u8 = 2;
const FIELD_KIND: u8 = 3;
const FIELD_REQUEST_ID: u8 = 4;
const FIELD_TRANSACTION_ID: u8 = 5;
const FIELD_PAYLOAD_LENGTH: u8 = 6;
const FIELD_PAYLOAD: u8 = 7;
const FIELD_COUNT_WITHOUT_TRANSACTION: u64 = 7;
const FIELD_COUNT_WITH_TRANSACTION: u64 = 8;

const KIND_CAPABILITIES: u8 = 0;
const KIND_STATUS: u8 = 1;
const KIND_BEGIN: u8 = 2;
const KIND_SUBMIT_CANDIDATE: u8 = 3;
const KIND_VALIDATE: u8 = 4;
const KIND_COMMIT: u8 = 5;
const KIND_ABORT: u8 = 6;
const KIND_FACTORY_RESET: u8 = 7;

const RESPONSE_CAPABILITIES: u8 = 0;
const RESPONSE_STATUS: u8 = 1;
const RESPONSE_TRANSACTION_BEGUN: u8 = 2;
const RESPONSE_CANDIDATE_RECEIVED: u8 = 3;
const RESPONSE_CANDIDATE_VALIDATED: u8 = 4;
const RESPONSE_COMMITTED: u8 = 5;
const RESPONSE_ABORTED: u8 = 6;
const RESPONSE_FACTORY_RESET: u8 = 7;
const RESPONSE_ERROR: u8 = 8;

const STATUS_PAYLOAD_BYTES: usize = 36;
const RESPONSE_PAYLOAD_MAX: usize = STATUS_PAYLOAD_BYTES;

/// Redacted success or failure carried by one response envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireResponse {
    /// A successful provisioning response.
    Success(Response),
    /// A stable redacted service failure.
    Error(ServiceError),
}

impl From<Response> for WireResponse {
    fn from(value: Response) -> Self {
        Self::Success(value)
    }
}

impl From<ServiceError> for WireResponse {
    fn from(value: ServiceError) -> Self {
        Self::Error(value)
    }
}

/// Redacted request encoding or decoding failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CodecError {
    /// The complete request exceeds [`MAX_REQUEST_BYTES`].
    RequestTooLarge,
    /// The complete response exceeds [`MAX_RESPONSE_BYTES`].
    ResponseTooLarge,
    /// The output slice cannot hold the complete encoded request.
    OutputTooSmall,
    /// The CBOR structure or primitive type is invalid.
    Malformed,
    /// The envelope uses an indefinite map, which is not accepted.
    IndefiniteContainer,
    /// A singular envelope field appeared more than once.
    DuplicateField,
    /// A required envelope field is absent.
    MissingField,
    /// The envelope contains a field not defined by this wire version.
    UnknownField,
    /// The protocol magic does not identify a provisioning request.
    InvalidMagic,
    /// The wire major version differs or its minor version is newer.
    UnsupportedVersion,
    /// The operation discriminant is not defined by this wire version.
    InvalidMessageKind,
    /// A request or transaction identifier used the reserved zero value.
    InvalidIdentifier,
    /// The operation payload has an invalid length or representation.
    InvalidPayload,
    /// Bytes remain after the complete envelope.
    TrailingData,
}

/// Decodes one complete bounded version-1 CBOR request.
///
/// The envelope is a definite CBOR map with numeric keys. The payload is a
/// byte string whose length is repeated explicitly in the envelope. Duplicate
/// or unknown fields, indefinite containers, and trailing bytes are rejected.
pub fn decode_request(input: &[u8]) -> Result<Request<'_>, CodecError> {
    if input.len() > MAX_REQUEST_BYTES {
        return Err(CodecError::RequestTooLarge);
    }

    let mut decoder = Decoder::new(input);
    let entries = decoder
        .map()
        .map_err(|_| CodecError::Malformed)?
        .ok_or(CodecError::IndefiniteContainer)?;
    if entries > FIELD_COUNT_WITH_TRANSACTION {
        return Err(CodecError::UnknownField);
    }

    let mut seen = 0_u16;
    let mut magic = None;
    let mut version_major = None;
    let mut version_minor = None;
    let mut kind = None;
    let mut request_id = None;
    let mut transaction_id = None;
    let mut payload_length = None;
    let mut payload = None;

    for _ in 0..entries {
        let field = decoder.u8().map_err(|_| CodecError::Malformed)?;
        if field > FIELD_PAYLOAD {
            return Err(CodecError::UnknownField);
        }
        let mask = 1_u16 << field;
        if seen & mask != 0 {
            return Err(CodecError::DuplicateField);
        }
        seen |= mask;

        match field {
            FIELD_MAGIC => magic = Some(decoder.u32().map_err(|_| CodecError::Malformed)?),
            FIELD_VERSION_MAJOR => {
                version_major = Some(decoder.u16().map_err(|_| CodecError::Malformed)?);
            }
            FIELD_VERSION_MINOR => {
                version_minor = Some(decoder.u16().map_err(|_| CodecError::Malformed)?);
            }
            FIELD_KIND => kind = Some(decoder.u8().map_err(|_| CodecError::Malformed)?),
            FIELD_REQUEST_ID => {
                request_id = Some(decoder.u32().map_err(|_| CodecError::Malformed)?);
            }
            FIELD_TRANSACTION_ID => {
                transaction_id = Some(decoder.u64().map_err(|_| CodecError::Malformed)?);
            }
            FIELD_PAYLOAD_LENGTH => {
                payload_length = Some(decoder.u16().map_err(|_| CodecError::Malformed)?);
            }
            FIELD_PAYLOAD => {
                payload = Some(decoder.bytes().map_err(|_| CodecError::Malformed)?);
            }
            _ => return Err(CodecError::UnknownField),
        }
    }

    if decoder.position() != input.len() {
        return Err(CodecError::TrailingData);
    }
    if magic.ok_or(CodecError::MissingField)? != PROTOCOL_MAGIC {
        return Err(CodecError::InvalidMagic);
    }
    if version_major.ok_or(CodecError::MissingField)? != PROTOCOL_VERSION_MAJOR
        || version_minor.ok_or(CodecError::MissingField)? > PROTOCOL_VERSION_MINOR
    {
        return Err(CodecError::UnsupportedVersion);
    }

    let kind = kind.ok_or(CodecError::MissingField)?;
    let request_id = RequestId::new(request_id.ok_or(CodecError::MissingField)?)
        .ok_or(CodecError::InvalidIdentifier)?;
    let payload = payload.ok_or(CodecError::MissingField)?;
    if usize::from(payload_length.ok_or(CodecError::MissingField)?) != payload.len() {
        return Err(CodecError::InvalidPayload);
    }

    let needs_transaction = matches!(
        kind,
        KIND_BEGIN | KIND_SUBMIT_CANDIDATE | KIND_VALIDATE | KIND_COMMIT | KIND_ABORT
    );
    let transaction_id = match (needs_transaction, transaction_id) {
        (true, Some(value)) => {
            Some(TransactionId::new(value).ok_or(CodecError::InvalidIdentifier)?)
        }
        (true, None) => return Err(CodecError::MissingField),
        (false, None) => None,
        (false, Some(_)) => return Err(CodecError::InvalidPayload),
    };

    match kind {
        KIND_CAPABILITIES if payload.is_empty() => Ok(Request::Capabilities { request_id }),
        KIND_STATUS if payload.is_empty() => Ok(Request::Status { request_id }),
        KIND_BEGIN if payload.len() == 4 => Ok(Request::Begin {
            request_id,
            transaction_id: transaction_id.ok_or(CodecError::MissingField)?,
            schema: SchemaVersion::new(
                u16::from_be_bytes([payload[0], payload[1]]),
                u16::from_be_bytes([payload[2], payload[3]]),
            ),
        }),
        KIND_SUBMIT_CANDIDATE if payload.len() <= MAX_CANDIDATE_BYTES => {
            Ok(Request::SubmitCandidate {
                request_id,
                transaction_id: transaction_id.ok_or(CodecError::MissingField)?,
                encoded: payload,
            })
        }
        KIND_VALIDATE if payload.is_empty() => Ok(Request::Validate {
            request_id,
            transaction_id: transaction_id.ok_or(CodecError::MissingField)?,
        }),
        KIND_COMMIT if payload.is_empty() => Ok(Request::Commit {
            request_id,
            transaction_id: transaction_id.ok_or(CodecError::MissingField)?,
        }),
        KIND_ABORT if payload.is_empty() => Ok(Request::Abort {
            request_id,
            transaction_id: transaction_id.ok_or(CodecError::MissingField)?,
        }),
        KIND_FACTORY_RESET if payload.is_empty() => Ok(Request::FactoryReset { request_id }),
        KIND_CAPABILITIES
        | KIND_STATUS
        | KIND_BEGIN
        | KIND_SUBMIT_CANDIDATE
        | KIND_VALIDATE
        | KIND_COMMIT
        | KIND_ABORT
        | KIND_FACTORY_RESET => Err(CodecError::InvalidPayload),
        _ => Err(CodecError::InvalidMessageKind),
    }
}

/// Encodes one version-1 request into caller-owned bounded storage.
pub fn encode_request(request: &Request<'_>, mut output: &mut [u8]) -> Result<usize, CodecError> {
    if output.len() > MAX_REQUEST_BYTES {
        output = &mut output[..MAX_REQUEST_BYTES];
    }

    let (kind, transaction_id, schema, payload) = match request {
        Request::Capabilities { .. } => (KIND_CAPABILITIES, None, None, &[][..]),
        Request::Status { .. } => (KIND_STATUS, None, None, &[][..]),
        Request::Begin {
            transaction_id,
            schema,
            ..
        } => (KIND_BEGIN, Some(*transaction_id), Some(*schema), &[][..]),
        Request::SubmitCandidate {
            transaction_id,
            encoded,
            ..
        } => {
            if encoded.len() > MAX_CANDIDATE_BYTES {
                return Err(CodecError::RequestTooLarge);
            }
            (KIND_SUBMIT_CANDIDATE, Some(*transaction_id), None, *encoded)
        }
        Request::Validate { transaction_id, .. } => {
            (KIND_VALIDATE, Some(*transaction_id), None, &[][..])
        }
        Request::Commit { transaction_id, .. } => {
            (KIND_COMMIT, Some(*transaction_id), None, &[][..])
        }
        Request::Abort { transaction_id, .. } => (KIND_ABORT, Some(*transaction_id), None, &[][..]),
        Request::FactoryReset { .. } => (KIND_FACTORY_RESET, None, None, &[][..]),
    };
    let schema_payload = schema.map(|schema| {
        let major = schema.major.to_be_bytes();
        let minor = schema.minor.to_be_bytes();
        [major[0], major[1], minor[0], minor[1]]
    });
    let payload = schema_payload.as_ref().map_or(payload, |bytes| &bytes[..]);
    let payload_length = u16::try_from(payload.len()).map_err(|_| CodecError::RequestTooLarge)?;

    let cursor = Cursor::new(output);
    let mut encoder = Encoder::new(cursor);
    encoder
        .map(if transaction_id.is_some() {
            FIELD_COUNT_WITH_TRANSACTION
        } else {
            FIELD_COUNT_WITHOUT_TRANSACTION
        })
        .and_then(|encoder| encoder.u8(FIELD_MAGIC))
        .and_then(|encoder| encoder.u32(PROTOCOL_MAGIC))
        .and_then(|encoder| encoder.u8(FIELD_VERSION_MAJOR))
        .and_then(|encoder| encoder.u16(PROTOCOL_VERSION_MAJOR))
        .and_then(|encoder| encoder.u8(FIELD_VERSION_MINOR))
        .and_then(|encoder| encoder.u16(PROTOCOL_VERSION_MINOR))
        .and_then(|encoder| encoder.u8(FIELD_KIND))
        .and_then(|encoder| encoder.u8(kind))
        .and_then(|encoder| encoder.u8(FIELD_REQUEST_ID))
        .and_then(|encoder| encoder.u32(request.request_id().get()))
        .map_err(|_| CodecError::OutputTooSmall)?;
    if let Some(transaction_id) = transaction_id {
        encoder
            .u8(FIELD_TRANSACTION_ID)
            .and_then(|encoder| encoder.u64(transaction_id.get()))
            .map_err(|_| CodecError::OutputTooSmall)?;
    }
    encoder
        .u8(FIELD_PAYLOAD_LENGTH)
        .and_then(|encoder| encoder.u16(payload_length))
        .and_then(|encoder| encoder.u8(FIELD_PAYLOAD))
        .and_then(|encoder| encoder.bytes(payload))
        .map_err(|_| CodecError::OutputTooSmall)?;

    let len = encoder.writer().position();
    if len > MAX_REQUEST_BYTES {
        Err(CodecError::RequestTooLarge)
    } else {
        Ok(len)
    }
}

/// Encodes one complete redacted success or error response.
pub fn encode_response(response: WireResponse, mut output: &mut [u8]) -> Result<usize, CodecError> {
    if output.len() > MAX_RESPONSE_BYTES {
        output = &mut output[..MAX_RESPONSE_BYTES];
    }
    let mut payload = [0; RESPONSE_PAYLOAD_MAX];
    let (request_id, kind, payload_len) = encode_response_payload(response, &mut payload);

    let cursor = Cursor::new(output);
    let mut encoder = Encoder::new(cursor);
    encoder
        .map(FIELD_COUNT_WITHOUT_TRANSACTION)
        .and_then(|encoder| encoder.u8(FIELD_MAGIC))
        .and_then(|encoder| encoder.u32(PROTOCOL_MAGIC))
        .and_then(|encoder| encoder.u8(FIELD_VERSION_MAJOR))
        .and_then(|encoder| encoder.u16(PROTOCOL_VERSION_MAJOR))
        .and_then(|encoder| encoder.u8(FIELD_VERSION_MINOR))
        .and_then(|encoder| encoder.u16(PROTOCOL_VERSION_MINOR))
        .and_then(|encoder| encoder.u8(FIELD_KIND))
        .and_then(|encoder| encoder.u8(kind))
        .and_then(|encoder| encoder.u8(FIELD_REQUEST_ID))
        .and_then(|encoder| encoder.u32(request_id.get()))
        .and_then(|encoder| encoder.u8(FIELD_PAYLOAD_LENGTH))
        .and_then(|encoder| encoder.u16(payload_len as u16))
        .and_then(|encoder| encoder.u8(FIELD_PAYLOAD))
        .and_then(|encoder| encoder.bytes(&payload[..payload_len]))
        .map_err(|_| CodecError::OutputTooSmall)?;
    let len = encoder.writer().position();
    if len > MAX_RESPONSE_BYTES {
        Err(CodecError::ResponseTooLarge)
    } else {
        Ok(len)
    }
}

/// Decodes one complete redacted success or error response.
pub fn decode_response(input: &[u8]) -> Result<WireResponse, CodecError> {
    if input.len() > MAX_RESPONSE_BYTES {
        return Err(CodecError::ResponseTooLarge);
    }

    let mut decoder = Decoder::new(input);
    let entries = decoder
        .map()
        .map_err(|_| CodecError::Malformed)?
        .ok_or(CodecError::IndefiniteContainer)?;
    if entries > FIELD_COUNT_WITHOUT_TRANSACTION {
        return Err(CodecError::UnknownField);
    }

    let mut seen = 0_u16;
    let mut magic = None;
    let mut version_major = None;
    let mut version_minor = None;
    let mut kind = None;
    let mut request_id = None;
    let mut payload_length = None;
    let mut payload = None;

    for _ in 0..entries {
        let field = decoder.u8().map_err(|_| CodecError::Malformed)?;
        if field > FIELD_PAYLOAD || field == FIELD_TRANSACTION_ID {
            return Err(CodecError::UnknownField);
        }
        let mask = 1_u16 << field;
        if seen & mask != 0 {
            return Err(CodecError::DuplicateField);
        }
        seen |= mask;
        match field {
            FIELD_MAGIC => magic = Some(decoder.u32().map_err(|_| CodecError::Malformed)?),
            FIELD_VERSION_MAJOR => {
                version_major = Some(decoder.u16().map_err(|_| CodecError::Malformed)?);
            }
            FIELD_VERSION_MINOR => {
                version_minor = Some(decoder.u16().map_err(|_| CodecError::Malformed)?);
            }
            FIELD_KIND => kind = Some(decoder.u8().map_err(|_| CodecError::Malformed)?),
            FIELD_REQUEST_ID => {
                request_id = Some(decoder.u32().map_err(|_| CodecError::Malformed)?);
            }
            FIELD_PAYLOAD_LENGTH => {
                payload_length = Some(decoder.u16().map_err(|_| CodecError::Malformed)?);
            }
            FIELD_PAYLOAD => payload = Some(decoder.bytes().map_err(|_| CodecError::Malformed)?),
            _ => return Err(CodecError::UnknownField),
        }
    }

    if decoder.position() != input.len() {
        return Err(CodecError::TrailingData);
    }
    if magic.ok_or(CodecError::MissingField)? != PROTOCOL_MAGIC {
        return Err(CodecError::InvalidMagic);
    }
    if version_major.ok_or(CodecError::MissingField)? != PROTOCOL_VERSION_MAJOR
        || version_minor.ok_or(CodecError::MissingField)? > PROTOCOL_VERSION_MINOR
    {
        return Err(CodecError::UnsupportedVersion);
    }
    let request_id = RequestId::new(request_id.ok_or(CodecError::MissingField)?)
        .ok_or(CodecError::InvalidIdentifier)?;
    let payload = payload.ok_or(CodecError::MissingField)?;
    if usize::from(payload_length.ok_or(CodecError::MissingField)?) != payload.len() {
        return Err(CodecError::InvalidPayload);
    }
    decode_response_payload(kind.ok_or(CodecError::MissingField)?, request_id, payload)
}

fn encode_response_payload(
    response: WireResponse,
    payload: &mut [u8; RESPONSE_PAYLOAD_MAX],
) -> (RequestId, u8, usize) {
    match response {
        WireResponse::Success(Response { request_id, kind }) => match kind {
            ResponseKind::Capabilities(capabilities) => {
                payload[..2].copy_from_slice(&capabilities.protocol_major.to_be_bytes());
                payload[2..4].copy_from_slice(&capabilities.protocol_minor.to_be_bytes());
                payload[4..8].copy_from_slice(&capabilities.max_candidate_bytes.to_be_bytes());
                payload[8] = u8::from(capabilities.reboot_to_apply);
                (request_id, RESPONSE_CAPABILITIES, 9)
            }
            ResponseKind::Status(status) => {
                encode_status(status, payload);
                (request_id, RESPONSE_STATUS, STATUS_PAYLOAD_BYTES)
            }
            ResponseKind::TransactionBegun => (request_id, RESPONSE_TRANSACTION_BEGUN, 0),
            ResponseKind::CandidateReceived => (request_id, RESPONSE_CANDIDATE_RECEIVED, 0),
            ResponseKind::CandidateValidated => (request_id, RESPONSE_CANDIDATE_VALIDATED, 0),
            ResponseKind::Committed(disposition) => {
                let (tag, generation) = match disposition {
                    CommitDisposition::RebootRequired { pending_generation } => {
                        (0, pending_generation)
                    }
                    CommitDisposition::ApplyScheduled { pending_generation } => {
                        (1, pending_generation)
                    }
                };
                payload[0] = tag;
                payload[1..5].copy_from_slice(&generation.get().to_be_bytes());
                (request_id, RESPONSE_COMMITTED, 5)
            }
            ResponseKind::Aborted => (request_id, RESPONSE_ABORTED, 0),
            ResponseKind::FactoryReset => (request_id, RESPONSE_FACTORY_RESET, 0),
        },
        WireResponse::Error(error) => {
            payload[0] = encode_service_error(error.kind);
            (error.request_id, RESPONSE_ERROR, 1)
        }
    }
}

fn decode_response_payload(
    kind: u8,
    request_id: RequestId,
    payload: &[u8],
) -> Result<WireResponse, CodecError> {
    let response_kind = match kind {
        RESPONSE_CAPABILITIES if payload.len() == 9 && payload[8] <= 1 => {
            ResponseKind::Capabilities(Capabilities {
                protocol_major: read_u16(&payload[..2]),
                protocol_minor: read_u16(&payload[2..4]),
                max_candidate_bytes: read_u32(&payload[4..8]),
                reboot_to_apply: payload[8] == 1,
            })
        }
        RESPONSE_STATUS if payload.len() == STATUS_PAYLOAD_BYTES => {
            ResponseKind::Status(decode_status(payload)?)
        }
        RESPONSE_TRANSACTION_BEGUN if payload.is_empty() => ResponseKind::TransactionBegun,
        RESPONSE_CANDIDATE_RECEIVED if payload.is_empty() => ResponseKind::CandidateReceived,
        RESPONSE_CANDIDATE_VALIDATED if payload.is_empty() => ResponseKind::CandidateValidated,
        RESPONSE_COMMITTED if payload.len() == 5 => {
            let generation =
                Generation::new(read_u32(&payload[1..5])).ok_or(CodecError::InvalidIdentifier)?;
            ResponseKind::Committed(match payload[0] {
                0 => CommitDisposition::RebootRequired {
                    pending_generation: generation,
                },
                1 => CommitDisposition::ApplyScheduled {
                    pending_generation: generation,
                },
                _ => return Err(CodecError::InvalidPayload),
            })
        }
        RESPONSE_ABORTED if payload.is_empty() => ResponseKind::Aborted,
        RESPONSE_FACTORY_RESET if payload.is_empty() => ResponseKind::FactoryReset,
        RESPONSE_ERROR if payload.len() == 1 => {
            return Ok(WireResponse::Error(ServiceError {
                request_id,
                kind: decode_service_error(payload[0])?,
            }));
        }
        RESPONSE_CAPABILITIES
        | RESPONSE_STATUS
        | RESPONSE_TRANSACTION_BEGUN
        | RESPONSE_CANDIDATE_RECEIVED
        | RESPONSE_CANDIDATE_VALIDATED
        | RESPONSE_COMMITTED
        | RESPONSE_ABORTED
        | RESPONSE_FACTORY_RESET
        | RESPONSE_ERROR => return Err(CodecError::InvalidPayload),
        _ => return Err(CodecError::InvalidMessageKind),
    };
    Ok(WireResponse::Success(Response {
        request_id,
        kind: response_kind,
    }))
}

fn encode_status(status: Status, output: &mut [u8; STATUS_PAYLOAD_BYTES]) {
    output.fill(0);
    match status.device {
        DeviceState::Unprovisioned => output[0] = 0,
        DeviceState::Provisioned {
            confirmed_generation,
        } => {
            output[0] = 1;
            output[1..5].copy_from_slice(&confirmed_generation.get().to_be_bytes());
        }
        DeviceState::PendingVerification {
            previous_generation,
            pending_generation,
            attempts,
        } => {
            output[0] = 2;
            encode_optional_generation(previous_generation, &mut output[5..9]);
            output[9..13].copy_from_slice(&pending_generation.get().to_be_bytes());
            output[13] = attempts;
        }
        DeviceState::RollbackRequired {
            previous_generation,
            rejected_generation,
            reason,
        } => {
            output[0] = 3;
            encode_optional_generation(previous_generation, &mut output[5..9]);
            output[9..13].copy_from_slice(&rejected_generation.get().to_be_bytes());
            output[14] = encode_rejection_reason(reason);
        }
        DeviceState::RecoveryRequired { reason } => {
            output[0] = 4;
            output[14] = encode_recovery_reason(reason);
        }
        DeviceState::ResetInProgress => output[0] = 5,
    }

    match status.transaction {
        TransactionState::Idle => output[15] = 0,
        TransactionState::Owned {
            session_id,
            transaction_id,
            schema,
        } => {
            output[15] = 1;
            encode_transaction(session_id, transaction_id, output);
            output[32..34].copy_from_slice(&schema.major.to_be_bytes());
            output[34..36].copy_from_slice(&schema.minor.to_be_bytes());
        }
        TransactionState::CandidateReceived {
            session_id,
            transaction_id,
        } => {
            output[15] = 2;
            encode_transaction(session_id, transaction_id, output);
        }
        TransactionState::CandidateValidated {
            session_id,
            transaction_id,
        } => {
            output[15] = 3;
            encode_transaction(session_id, transaction_id, output);
        }
        TransactionState::CommitInProgress {
            session_id,
            transaction_id,
        } => {
            output[15] = 4;
            encode_transaction(session_id, transaction_id, output);
        }
        TransactionState::Committed { pending_generation } => {
            output[15] = 5;
            output[24..32].copy_from_slice(&u64::from(pending_generation.get()).to_be_bytes());
        }
    }
}

fn decode_status(input: &[u8]) -> Result<Status, CodecError> {
    let confirmed = optional_generation(read_u32(&input[1..5]))?;
    let previous = optional_generation(read_u32(&input[5..9]))?;
    let pending = optional_generation(read_u32(&input[9..13]))?;
    let device = match input[0] {
        0 if confirmed.is_none()
            && previous.is_none()
            && pending.is_none()
            && input[13] == 0
            && input[14] == 0 =>
        {
            DeviceState::Unprovisioned
        }
        1 if confirmed.is_some()
            && previous.is_none()
            && pending.is_none()
            && input[13] == 0
            && input[14] == 0 =>
        {
            DeviceState::Provisioned {
                confirmed_generation: confirmed.ok_or(CodecError::InvalidPayload)?,
            }
        }
        2 if confirmed.is_none() && pending.is_some() && input[14] == 0 => {
            DeviceState::PendingVerification {
                previous_generation: previous,
                pending_generation: pending.ok_or(CodecError::InvalidPayload)?,
                attempts: input[13],
            }
        }
        3 if confirmed.is_none() && pending.is_some() && input[13] == 0 => {
            DeviceState::RollbackRequired {
                previous_generation: previous,
                rejected_generation: pending.ok_or(CodecError::InvalidPayload)?,
                reason: decode_rejection_reason(input[14])?,
            }
        }
        4 if confirmed.is_none() && previous.is_none() && pending.is_none() && input[13] == 0 => {
            DeviceState::RecoveryRequired {
                reason: decode_recovery_reason(input[14])?,
            }
        }
        5 if confirmed.is_none()
            && previous.is_none()
            && pending.is_none()
            && input[13] == 0
            && input[14] == 0 =>
        {
            DeviceState::ResetInProgress
        }
        _ => return Err(CodecError::InvalidPayload),
    };

    let session_value = read_u64(&input[16..24]);
    let transaction_value = read_u64(&input[24..32]);
    let schema = SchemaVersion::new(read_u16(&input[32..34]), read_u16(&input[34..36]));
    let transaction = match input[15] {
        0 if session_value == 0 && transaction_value == 0 && schema == SchemaVersion::new(0, 0) => {
            TransactionState::Idle
        }
        1 => TransactionState::Owned {
            session_id: SessionId::new(session_value).ok_or(CodecError::InvalidIdentifier)?,
            transaction_id: TransactionId::new(transaction_value)
                .ok_or(CodecError::InvalidIdentifier)?,
            schema,
        },
        tag @ 2..=4 if schema == SchemaVersion::new(0, 0) => {
            let session_id = SessionId::new(session_value).ok_or(CodecError::InvalidIdentifier)?;
            let transaction_id =
                TransactionId::new(transaction_value).ok_or(CodecError::InvalidIdentifier)?;
            match tag {
                2 => TransactionState::CandidateReceived {
                    session_id,
                    transaction_id,
                },
                3 => TransactionState::CandidateValidated {
                    session_id,
                    transaction_id,
                },
                4 => TransactionState::CommitInProgress {
                    session_id,
                    transaction_id,
                },
                _ => return Err(CodecError::InvalidPayload),
            }
        }
        5 if session_value == 0
            && schema == SchemaVersion::new(0, 0)
            && transaction_value <= u64::from(u32::MAX) =>
        {
            TransactionState::Committed {
                pending_generation: Generation::new(transaction_value as u32)
                    .ok_or(CodecError::InvalidIdentifier)?,
            }
        }
        _ => return Err(CodecError::InvalidPayload),
    };
    Ok(Status {
        device,
        transaction,
    })
}

fn encode_transaction(
    session_id: SessionId,
    transaction_id: TransactionId,
    output: &mut [u8; STATUS_PAYLOAD_BYTES],
) {
    output[16..24].copy_from_slice(&session_id.get().to_be_bytes());
    output[24..32].copy_from_slice(&transaction_id.get().to_be_bytes());
}

fn encode_optional_generation(generation: Option<Generation>, output: &mut [u8]) {
    output.copy_from_slice(&generation.map_or(0, Generation::get).to_be_bytes());
}

fn optional_generation(value: u32) -> Result<Option<Generation>, CodecError> {
    if value == 0 {
        Ok(None)
    } else {
        Generation::new(value)
            .map(Some)
            .ok_or(CodecError::InvalidIdentifier)
    }
}

const fn encode_rejection_reason(reason: RejectionReason) -> u8 {
    match reason {
        RejectionReason::ApplyFailed => 1,
        RejectionReason::WifiUnavailable => 2,
        RejectionReason::NetworkUnavailable => 3,
        RejectionReason::VerificationFailed => 4,
        RejectionReason::AttemptsExhausted => 5,
    }
}

const fn decode_rejection_reason(value: u8) -> Result<RejectionReason, CodecError> {
    match value {
        1 => Ok(RejectionReason::ApplyFailed),
        2 => Ok(RejectionReason::WifiUnavailable),
        3 => Ok(RejectionReason::NetworkUnavailable),
        4 => Ok(RejectionReason::VerificationFailed),
        5 => Ok(RejectionReason::AttemptsExhausted),
        _ => Err(CodecError::InvalidPayload),
    }
}

const fn encode_recovery_reason(reason: RecoveryReason) -> u8 {
    match reason {
        RecoveryReason::CorruptedRecord => 1,
        RecoveryReason::IncompatiblePersistentVersion => 2,
        RecoveryReason::MissingSlot => 3,
        RecoveryReason::GenerationMismatch => 4,
        RecoveryReason::GenerationExhausted => 5,
        RecoveryReason::StorageFailure => 6,
    }
}

const fn decode_recovery_reason(value: u8) -> Result<RecoveryReason, CodecError> {
    match value {
        1 => Ok(RecoveryReason::CorruptedRecord),
        2 => Ok(RecoveryReason::IncompatiblePersistentVersion),
        3 => Ok(RecoveryReason::MissingSlot),
        4 => Ok(RecoveryReason::GenerationMismatch),
        5 => Ok(RecoveryReason::GenerationExhausted),
        6 => Ok(RecoveryReason::StorageFailure),
        _ => Err(CodecError::InvalidPayload),
    }
}

const fn encode_service_error(error: ServiceErrorKind) -> u8 {
    match error {
        ServiceErrorKind::UnsupportedProtocolVersion => 1,
        ServiceErrorKind::UnsupportedProductSchema => 2,
        ServiceErrorKind::InvalidIdentifier => 3,
        ServiceErrorKind::CapacityExceeded => 4,
        ServiceErrorKind::MalformedRequest => 5,
        ServiceErrorKind::Unauthorized => 6,
        ServiceErrorKind::TransactionBusy => 7,
        ServiceErrorKind::TransactionMismatch => 8,
        ServiceErrorKind::InvalidTransition => 9,
        ServiceErrorKind::RequestConflict => 10,
        ServiceErrorKind::CandidateDecode => 11,
        ServiceErrorKind::CandidateValidation => 12,
        ServiceErrorKind::Storage => 13,
        ServiceErrorKind::RecoveryRequired => 14,
    }
}

const fn decode_service_error(value: u8) -> Result<ServiceErrorKind, CodecError> {
    match value {
        1 => Ok(ServiceErrorKind::UnsupportedProtocolVersion),
        2 => Ok(ServiceErrorKind::UnsupportedProductSchema),
        3 => Ok(ServiceErrorKind::InvalidIdentifier),
        4 => Ok(ServiceErrorKind::CapacityExceeded),
        5 => Ok(ServiceErrorKind::MalformedRequest),
        6 => Ok(ServiceErrorKind::Unauthorized),
        7 => Ok(ServiceErrorKind::TransactionBusy),
        8 => Ok(ServiceErrorKind::TransactionMismatch),
        9 => Ok(ServiceErrorKind::InvalidTransition),
        10 => Ok(ServiceErrorKind::RequestConflict),
        11 => Ok(ServiceErrorKind::CandidateDecode),
        12 => Ok(ServiceErrorKind::CandidateValidation),
        13 => Ok(ServiceErrorKind::Storage),
        14 => Ok(ServiceErrorKind::RecoveryRequired),
        _ => Err(CodecError::InvalidPayload),
    }
}

fn read_u16(input: &[u8]) -> u16 {
    u16::from_be_bytes([input[0], input[1]])
}

fn read_u32(input: &[u8]) -> u32 {
    u32::from_be_bytes([input[0], input[1], input[2], input[3]])
}

fn read_u64(input: &[u8]) -> u64 {
    u64::from_be_bytes([
        input[0], input[1], input[2], input[3], input[4], input[5], input[6], input[7],
    ])
}

#[cfg(test)]
mod tests {
    use embedded_sdk_config::SchemaVersion;
    use minicbor::{Encoder, encode::write::Cursor};

    use super::{
        CodecError, FIELD_COUNT_WITHOUT_TRANSACTION, FIELD_KIND, FIELD_MAGIC, FIELD_PAYLOAD,
        FIELD_PAYLOAD_LENGTH, FIELD_REQUEST_ID, FIELD_VERSION_MAJOR, FIELD_VERSION_MINOR,
        KIND_STATUS, PROTOCOL_MAGIC, WireResponse, decode_request, decode_response, encode_request,
        encode_response,
    };
    use crate::{
        Capabilities, CommitDisposition, DeviceState, ErrorKind, Generation, MAX_REQUEST_BYTES,
        MAX_RESPONSE_BYTES, RecoveryReason, RejectionReason, Request, RequestId, Response,
        ResponseKind, ServiceError, SessionId, Status, TransactionId, TransactionState,
    };

    fn request_id(value: u32) -> RequestId {
        RequestId::new(value).unwrap()
    }

    fn transaction_id(value: u64) -> TransactionId {
        TransactionId::new(value).unwrap()
    }

    fn session_id(value: u64) -> SessionId {
        SessionId::new(value).unwrap()
    }

    fn generation(value: u32) -> Generation {
        Generation::new(value).unwrap()
    }

    fn assert_decode_error(input: &[u8], expected: CodecError) {
        match decode_request(input) {
            Err(actual) => assert_eq!(actual, expected),
            Ok(_) => panic!("request unexpectedly decoded"),
        }
    }

    #[test]
    fn round_trips_every_request_shape() {
        let candidates = [
            Request::Capabilities {
                request_id: request_id(1),
            },
            Request::Status {
                request_id: request_id(2),
            },
            Request::Begin {
                request_id: request_id(3),
                transaction_id: transaction_id(10),
                schema: SchemaVersion::new(1, 2),
            },
            Request::SubmitCandidate {
                request_id: request_id(4),
                transaction_id: transaction_id(10),
                encoded: &[9, 8, 7],
            },
            Request::Validate {
                request_id: request_id(5),
                transaction_id: transaction_id(10),
            },
            Request::Commit {
                request_id: request_id(6),
                transaction_id: transaction_id(10),
            },
            Request::Abort {
                request_id: request_id(7),
                transaction_id: transaction_id(10),
            },
            Request::FactoryReset {
                request_id: request_id(8),
            },
        ];
        let mut output = [0; MAX_REQUEST_BYTES];

        for expected in candidates {
            let len = encode_request(&expected, &mut output).unwrap();
            let actual = decode_request(&output[..len]).unwrap();
            assert_eq!(actual.request_id(), expected.request_id());
            assert_eq!(actual.transaction_id(), expected.transaction_id());
            assert_eq!(actual.operation_kind(), expected.operation_kind());
            if let Request::SubmitCandidate { encoded, .. } = actual {
                assert_eq!(encoded, &[9, 8, 7]);
            }
        }
    }

    #[test]
    fn status_has_a_stable_golden_vector() {
        let mut output = [0; 64];
        let len = encode_request(
            &Request::Status {
                request_id: request_id(1),
            },
            &mut output,
        )
        .unwrap();

        assert_eq!(
            &output[..len],
            &[
                0xa7, 0x00, 0x1a, 0x50, 0x52, 0x56, 0x31, 0x01, 0x01, 0x02, 0x00, 0x03, 0x01, 0x04,
                0x01, 0x06, 0x00, 0x07, 0x40,
            ]
        );
    }

    #[test]
    fn rejects_duplicate_fields_and_trailing_data() {
        let mut duplicate = [0; 64];
        let mut cursor = Cursor::new(&mut duplicate[..]);
        let mut encoder = Encoder::new(&mut cursor);
        encoder
            .map(2)
            .unwrap()
            .u8(FIELD_MAGIC)
            .unwrap()
            .u32(PROTOCOL_MAGIC)
            .unwrap()
            .u8(FIELD_MAGIC)
            .unwrap()
            .u32(PROTOCOL_MAGIC)
            .unwrap();
        let len = cursor.position();
        assert_decode_error(&duplicate[..len], CodecError::DuplicateField);

        let mut output = [0; 64];
        let len = encode_request(
            &Request::Status {
                request_id: request_id(1),
            },
            &mut output,
        )
        .unwrap();
        output[len] = 0;
        assert_decode_error(&output[..=len], CodecError::TrailingData);
    }

    #[test]
    fn rejects_zero_identifiers_and_payload_length_mismatch() {
        let mut output = [0; 64];
        let mut cursor = Cursor::new(&mut output[..]);
        let mut encoder = Encoder::new(&mut cursor);
        encoder
            .map(FIELD_COUNT_WITHOUT_TRANSACTION)
            .unwrap()
            .u8(FIELD_MAGIC)
            .unwrap()
            .u32(PROTOCOL_MAGIC)
            .unwrap()
            .u8(FIELD_VERSION_MAJOR)
            .unwrap()
            .u16(crate::PROTOCOL_VERSION_MAJOR)
            .unwrap()
            .u8(FIELD_VERSION_MINOR)
            .unwrap()
            .u16(crate::PROTOCOL_VERSION_MINOR)
            .unwrap()
            .u8(FIELD_KIND)
            .unwrap()
            .u8(KIND_STATUS)
            .unwrap()
            .u8(FIELD_REQUEST_ID)
            .unwrap()
            .u32(0)
            .unwrap()
            .u8(FIELD_PAYLOAD_LENGTH)
            .unwrap()
            .u16(1)
            .unwrap()
            .u8(FIELD_PAYLOAD)
            .unwrap()
            .bytes(&[])
            .unwrap();
        let len = cursor.position();
        assert_decode_error(&output[..len], CodecError::InvalidIdentifier);

        output[14] = 1;
        assert_decode_error(&output[..len], CodecError::InvalidPayload);
    }

    #[test]
    fn round_trips_every_success_and_error_response() {
        let responses = [
            ResponseKind::Capabilities(Capabilities {
                protocol_major: 1,
                protocol_minor: 0,
                max_candidate_bytes: 1024,
                reboot_to_apply: true,
            }),
            ResponseKind::TransactionBegun,
            ResponseKind::CandidateReceived,
            ResponseKind::CandidateValidated,
            ResponseKind::Committed(CommitDisposition::RebootRequired {
                pending_generation: generation(9),
            }),
            ResponseKind::Committed(CommitDisposition::ApplyScheduled {
                pending_generation: generation(10),
            }),
            ResponseKind::Aborted,
            ResponseKind::FactoryReset,
        ];
        let mut output = [0; MAX_RESPONSE_BYTES];
        for kind in responses {
            let expected = WireResponse::Success(Response {
                request_id: request_id(7),
                kind,
            });
            let len = encode_response(expected, &mut output).unwrap();
            assert_eq!(decode_response(&output[..len]).unwrap(), expected);
        }

        let errors = [
            ErrorKind::UnsupportedProtocolVersion,
            ErrorKind::UnsupportedProductSchema,
            ErrorKind::InvalidIdentifier,
            ErrorKind::CapacityExceeded,
            ErrorKind::MalformedRequest,
            ErrorKind::Unauthorized,
            ErrorKind::TransactionBusy,
            ErrorKind::TransactionMismatch,
            ErrorKind::InvalidTransition,
            ErrorKind::RequestConflict,
            ErrorKind::CandidateDecode,
            ErrorKind::CandidateValidation,
            ErrorKind::Storage,
            ErrorKind::RecoveryRequired,
        ];
        for kind in errors {
            let expected = WireResponse::Error(ServiceError {
                request_id: request_id(8),
                kind,
            });
            let len = encode_response(expected, &mut output).unwrap();
            assert_eq!(decode_response(&output[..len]).unwrap(), expected);
        }
    }

    #[test]
    fn round_trips_every_redacted_status_shape() {
        let statuses = [
            Status {
                device: DeviceState::Unprovisioned,
                transaction: TransactionState::Idle,
            },
            Status {
                device: DeviceState::Provisioned {
                    confirmed_generation: generation(1),
                },
                transaction: TransactionState::Owned {
                    session_id: session_id(11),
                    transaction_id: transaction_id(12),
                    schema: SchemaVersion::new(1, 2),
                },
            },
            Status {
                device: DeviceState::PendingVerification {
                    previous_generation: Some(generation(1)),
                    pending_generation: generation(2),
                    attempts: 3,
                },
                transaction: TransactionState::CandidateReceived {
                    session_id: session_id(11),
                    transaction_id: transaction_id(12),
                },
            },
            Status {
                device: DeviceState::RollbackRequired {
                    previous_generation: None,
                    rejected_generation: generation(2),
                    reason: RejectionReason::WifiUnavailable,
                },
                transaction: TransactionState::CandidateValidated {
                    session_id: session_id(11),
                    transaction_id: transaction_id(12),
                },
            },
            Status {
                device: DeviceState::RecoveryRequired {
                    reason: RecoveryReason::GenerationMismatch,
                },
                transaction: TransactionState::CommitInProgress {
                    session_id: session_id(11),
                    transaction_id: transaction_id(12),
                },
            },
            Status {
                device: DeviceState::ResetInProgress,
                transaction: TransactionState::Committed {
                    pending_generation: generation(4),
                },
            },
        ];
        let mut output = [0; MAX_RESPONSE_BYTES];
        for status in statuses {
            let expected = WireResponse::Success(Response {
                request_id: request_id(1),
                kind: ResponseKind::Status(status),
            });
            let len = encode_response(expected, &mut output).unwrap();
            assert_eq!(decode_response(&output[..len]).unwrap(), expected);
        }
    }

    #[test]
    fn error_response_has_a_stable_golden_vector() {
        let mut output = [0; 64];
        let len = encode_response(
            WireResponse::Error(ServiceError {
                request_id: request_id(1),
                kind: ErrorKind::Unauthorized,
            }),
            &mut output,
        )
        .unwrap();
        assert_eq!(
            &output[..len],
            &[
                0xa7, 0x00, 0x1a, 0x50, 0x52, 0x56, 0x31, 0x01, 0x01, 0x02, 0x00, 0x03, 0x08, 0x04,
                0x01, 0x06, 0x01, 0x07, 0x41, 0x06,
            ]
        );
    }
}
