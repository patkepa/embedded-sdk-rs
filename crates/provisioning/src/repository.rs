//! Durable two-slot provisioning repository.

use embedded_sdk_config::SchemaVersion;
use embedded_sdk_storage::{Fetch, Key, KeyValueStore};
use zeroize::Zeroize;

use crate::{DeviceState, Generation, MAX_CANDIDATE_BYTES, RecoveryReason, RejectionReason};

/// Permanently assigned SDK namespace for provisioning records.
pub const PROVISIONING_NAMESPACE: u16 = 0x0001;
/// Atomic credential-free repository state record.
pub const STATE_KEY: Key = Key::new(PROVISIONING_NAMESPACE, 1);
/// First complete product-configuration slot.
pub const SLOT_A_KEY: Key = Key::new(PROVISIONING_NAMESPACE, 2);
/// Second complete product-configuration slot.
pub const SLOT_B_KEY: Key = Key::new(PROVISIONING_NAMESPACE, 3);

const STATE_MAGIC: [u8; 4] = *b"PST1";
const SLOT_MAGIC: [u8; 4] = *b"PSL1";
const PERSISTENT_FORMAT_VERSION: u16 = 1;
const STATE_RECORD_BYTES: usize = 24;
const SLOT_HEADER_BYTES: usize = 20;
const SLOT_CRC_BYTES: usize = 4;
const SLOT_OVERHEAD_BYTES: usize = SLOT_HEADER_BYTES + SLOT_CRC_BYTES;

/// Largest complete slot record, including format metadata and CRC.
pub const MAX_SLOT_RECORD_BYTES: usize = SLOT_OVERHEAD_BYTES + MAX_CANDIDATE_BYTES;

const STATE_READY: u8 = 0;
const STATE_ROLLBACK: u8 = 1;
const STATE_RESET: u8 = 2;

/// Repository failure that does not include configuration contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RepositoryError<E> {
    /// The underlying key-value store failed.
    Storage(E),
    /// Caller-owned record storage is too small.
    BufferTooSmall,
    /// The candidate exceeds the fixed repository limit.
    CandidateTooLarge,
    /// The requested durable transition is not valid in the recovered state.
    InvalidTransition,
    /// A just-written slot could not be read back byte-for-byte.
    ReadVerificationFailed,
}

/// Complete pending product configuration borrowed from caller-owned storage.
///
/// This type intentionally implements neither `Debug` nor `Display` because
/// its payload can contain credentials. The caller must zeroize `payload`
/// storage after decoding the product configuration.
pub struct StoredCandidate<'a> {
    /// Product schema declared when the candidate was staged.
    pub schema: SchemaVersion,
    /// Durable candidate generation.
    pub generation: Generation,
    /// Complete product-owned encoded configuration.
    pub payload: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Slot {
    A,
    B,
}

impl Slot {
    const fn key(self) -> Key {
        match self {
            Self::A => SLOT_A_KEY,
            Self::B => SLOT_B_KEY,
        }
    }

    const fn encoded(self) -> u8 {
        match self {
            Self::A => 1,
            Self::B => 2,
        }
    }

    const fn decode(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::A),
            2 => Some(Self::B),
            _ => None,
        }
    }

    const fn opposite(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SlotReference {
    slot: Slot,
    generation: Generation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DurableState {
    Unloaded,
    Ready {
        confirmed: Option<SlotReference>,
        pending: Option<SlotReference>,
        attempts: u8,
    },
    Rollback {
        previous: Option<SlotReference>,
        rejected: SlotReference,
        reason: RejectionReason,
    },
    Reset,
    Recovery(RecoveryReason),
}

/// Allocation-free durable provisioning repository over a [`KeyValueStore`].
///
/// A repository must be recovered before it accepts mutations. Candidate slot
/// records carry their own version, generation, length, and CRC in addition to
/// integrity supplied by the storage engine. Only the atomic state record
/// selects slots for activation.
pub struct Repository<S> {
    store: S,
    state: DurableState,
}

impl<S: KeyValueStore> Repository<S> {
    /// Creates an unopened repository. Call [`Self::recover`] before mutation.
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self {
            store,
            state: DurableState::Unloaded,
        }
    }

    /// Releases the underlying key-value store.
    pub fn into_store(self) -> S {
        self.store
    }

    /// Returns the redacted state established by recovery and later writes.
    #[must_use]
    pub fn device_state(&self) -> DeviceState {
        match self.state {
            DurableState::Unloaded => DeviceState::RecoveryRequired {
                reason: RecoveryReason::StorageFailure,
            },
            DurableState::Ready {
                confirmed: None,
                pending: None,
                ..
            } => DeviceState::Unprovisioned,
            DurableState::Ready {
                confirmed: Some(confirmed),
                pending: None,
                ..
            } => DeviceState::Provisioned {
                confirmed_generation: confirmed.generation,
            },
            DurableState::Ready {
                confirmed,
                pending: Some(pending),
                attempts,
            } => DeviceState::PendingVerification {
                previous_generation: confirmed.map(|reference| reference.generation),
                pending_generation: pending.generation,
                attempts,
            },
            DurableState::Rollback {
                previous,
                rejected,
                reason,
            } => DeviceState::RollbackRequired {
                previous_generation: previous.map(|reference| reference.generation),
                rejected_generation: rejected.generation,
                reason,
            },
            DurableState::Reset => DeviceState::ResetInProgress,
            DurableState::Recovery(reason) => DeviceState::RecoveryRequired { reason },
        }
    }

    /// Opens and validates the state record and every referenced candidate.
    ///
    /// `scratch` must hold a complete maximum-size slot record. It is zeroized
    /// before this method returns.
    pub async fn recover(
        &mut self,
        scratch: &mut [u8],
    ) -> Result<DeviceState, RepositoryError<S::Error>> {
        let result = self.recover_inner(scratch).await;
        scratch.zeroize();
        result
    }

    async fn recover_inner(
        &mut self,
        scratch: &mut [u8],
    ) -> Result<DeviceState, RepositoryError<S::Error>> {
        if scratch.len() < SLOT_OVERHEAD_BYTES + MAX_CANDIDATE_BYTES {
            return Err(RepositoryError::BufferTooSmall);
        }

        let fetch = self
            .store
            .get(STATE_KEY, &mut scratch[..STATE_RECORD_BYTES])
            .await
            .map_err(RepositoryError::Storage)?;
        let recovered = match fetch {
            Fetch::NotFound => DurableState::Ready {
                confirmed: None,
                pending: None,
                attempts: 0,
            },
            Fetch::Found { len } => match decode_state(&scratch[..len]) {
                Ok(state) => state,
                Err(RecordError::Incompatible) => {
                    DurableState::Recovery(RecoveryReason::IncompatiblePersistentVersion)
                }
                Err(RecordError::Corrupted) => {
                    DurableState::Recovery(RecoveryReason::CorruptedRecord)
                }
            },
            _ => DurableState::Recovery(RecoveryReason::CorruptedRecord),
        };

        self.state = recovered;
        let references = match recovered {
            DurableState::Ready {
                confirmed, pending, ..
            } => (confirmed, pending),
            DurableState::Rollback {
                previous, rejected, ..
            } => (previous, Some(rejected)),
            DurableState::Unloaded | DurableState::Reset | DurableState::Recovery(_) => {
                return Ok(self.device_state());
            }
        };

        if references.0.is_some_and(|confirmed| {
            references
                .1
                .is_some_and(|pending| pending.slot == confirmed.slot)
        }) {
            self.state = DurableState::Recovery(RecoveryReason::CorruptedRecord);
            return Ok(self.device_state());
        }

        for reference in [references.0, references.1].into_iter().flatten() {
            if let Some(reason) = self.validate_reference(reference, scratch).await? {
                self.state = DurableState::Recovery(reason);
                break;
            }
        }
        Ok(self.device_state())
    }

    async fn validate_reference(
        &mut self,
        reference: SlotReference,
        scratch: &mut [u8],
    ) -> Result<Option<RecoveryReason>, RepositoryError<S::Error>> {
        let fetch = self
            .store
            .get(reference.slot.key(), scratch)
            .await
            .map_err(RepositoryError::Storage)?;
        let len = match fetch {
            Fetch::NotFound => return Ok(Some(RecoveryReason::MissingSlot)),
            Fetch::Found { len } => len,
            _ => return Ok(Some(RecoveryReason::CorruptedRecord)),
        };
        let record = match decode_slot(&scratch[..len]) {
            Ok(record) => record,
            Err(RecordError::Incompatible) => {
                return Ok(Some(RecoveryReason::IncompatiblePersistentVersion));
            }
            Err(RecordError::Corrupted) => return Ok(Some(RecoveryReason::CorruptedRecord)),
        };
        Ok((record.generation != reference.generation)
            .then_some(RecoveryReason::GenerationMismatch))
    }

    /// Writes, read-verifies, and atomically selects a validated candidate as pending.
    ///
    /// `scratch` is zeroized on every return path.
    pub async fn stage_candidate(
        &mut self,
        schema: SchemaVersion,
        payload: &[u8],
        scratch: &mut [u8],
    ) -> Result<Generation, RepositoryError<S::Error>> {
        let result = self.stage_candidate_inner(schema, payload, scratch).await;
        scratch.zeroize();
        result
    }

    async fn stage_candidate_inner(
        &mut self,
        schema: SchemaVersion,
        payload: &[u8],
        scratch: &mut [u8],
    ) -> Result<Generation, RepositoryError<S::Error>> {
        if payload.len() > MAX_CANDIDATE_BYTES {
            return Err(RepositoryError::CandidateTooLarge);
        }
        let (confirmed, pending) = match self.state {
            DurableState::Ready {
                confirmed, pending, ..
            } => (confirmed, pending),
            _ => return Err(RepositoryError::InvalidTransition),
        };
        if pending.is_some() {
            return Err(RepositoryError::InvalidTransition);
        }

        let generation_value =
            confirmed.map_or(1, |reference| reference.generation.get().saturating_add(1));
        let generation = Generation::new(generation_value).ok_or_else(|| {
            self.state = DurableState::Recovery(RecoveryReason::GenerationExhausted);
            RepositoryError::InvalidTransition
        })?;
        if confirmed.is_some_and(|reference| reference.generation.get() == u32::MAX) {
            self.state = DurableState::Recovery(RecoveryReason::GenerationExhausted);
            return Err(RepositoryError::InvalidTransition);
        }
        let slot = confirmed.map_or(Slot::A, |reference| reference.slot.opposite());
        let record_len = encode_slot(schema, generation, payload, scratch)?;
        self.store
            .put(slot.key(), &scratch[..record_len])
            .await
            .map_err(RepositoryError::Storage)?;

        scratch[..record_len].zeroize();
        let verified_len = match self
            .store
            .get(slot.key(), scratch)
            .await
            .map_err(RepositoryError::Storage)?
        {
            Fetch::NotFound => return Err(RepositoryError::ReadVerificationFailed),
            Fetch::Found { len } => len,
            _ => return Err(RepositoryError::ReadVerificationFailed),
        };
        let verified = decode_slot(&scratch[..verified_len])
            .map_err(|_| RepositoryError::ReadVerificationFailed)?;
        if verified.schema != schema
            || verified.generation != generation
            || verified.payload != payload
        {
            return Err(RepositoryError::ReadVerificationFailed);
        }

        let next = DurableState::Ready {
            confirmed,
            pending: Some(SlotReference { slot, generation }),
            attempts: 0,
        };
        self.write_state(next).await?;
        Ok(generation)
    }

    /// Loads the complete pending candidate into caller-owned storage.
    pub async fn load_pending<'a>(
        &mut self,
        output: &'a mut [u8],
    ) -> Result<StoredCandidate<'a>, RepositoryError<S::Error>> {
        let pending = match self.state {
            DurableState::Ready {
                pending: Some(pending),
                ..
            } => pending,
            _ => return Err(RepositoryError::InvalidTransition),
        };
        self.load_reference(pending, output).await
    }

    /// Loads the complete confirmed configuration into caller-owned storage.
    pub async fn load_confirmed<'a>(
        &mut self,
        output: &'a mut [u8],
    ) -> Result<StoredCandidate<'a>, RepositoryError<S::Error>> {
        let confirmed = match self.state {
            DurableState::Ready {
                confirmed: Some(confirmed),
                ..
            } => confirmed,
            _ => return Err(RepositoryError::InvalidTransition),
        };
        self.load_reference(confirmed, output).await
    }

    async fn load_reference<'a>(
        &mut self,
        reference: SlotReference,
        output: &'a mut [u8],
    ) -> Result<StoredCandidate<'a>, RepositoryError<S::Error>> {
        let len = match self
            .store
            .get(reference.slot.key(), output)
            .await
            .map_err(RepositoryError::Storage)?
        {
            Fetch::NotFound => {
                self.state = DurableState::Recovery(RecoveryReason::MissingSlot);
                return Err(RepositoryError::InvalidTransition);
            }
            Fetch::Found { len } => len,
            _ => {
                self.state = DurableState::Recovery(RecoveryReason::CorruptedRecord);
                return Err(RepositoryError::InvalidTransition);
            }
        };
        let record = decode_slot(&output[..len]).map_err(|error| {
            self.state = DurableState::Recovery(match error {
                RecordError::Corrupted => RecoveryReason::CorruptedRecord,
                RecordError::Incompatible => RecoveryReason::IncompatiblePersistentVersion,
            });
            RepositoryError::InvalidTransition
        })?;
        if record.generation != reference.generation {
            self.state = DurableState::Recovery(RecoveryReason::GenerationMismatch);
            return Err(RepositoryError::InvalidTransition);
        }
        Ok(record)
    }

    /// Atomically records that one bounded boot-time verification attempt started.
    ///
    /// Calling this after `max_attempts` have already started moves the
    /// repository to rollback-required without applying the candidate again.
    pub async fn start_verification_attempt(
        &mut self,
        max_attempts: u8,
    ) -> Result<DeviceState, RepositoryError<S::Error>> {
        let (confirmed, pending, attempts) = match self.state {
            DurableState::Ready {
                confirmed,
                pending: Some(pending),
                attempts,
            } => (confirmed, pending, attempts),
            _ => return Err(RepositoryError::InvalidTransition),
        };
        let next = if max_attempts == 0 || attempts >= max_attempts {
            DurableState::Rollback {
                previous: confirmed,
                rejected: pending,
                reason: RejectionReason::AttemptsExhausted,
            }
        } else {
            DurableState::Ready {
                confirmed,
                pending: Some(pending),
                attempts: attempts + 1,
            }
        };
        self.write_state(next).await?;
        Ok(self.device_state())
    }

    /// Atomically promotes the pending candidate to confirmed after verification.
    pub async fn confirm_pending(&mut self) -> Result<Generation, RepositoryError<S::Error>> {
        let pending = match self.state {
            DurableState::Ready {
                pending: Some(pending),
                ..
            } => pending,
            _ => return Err(RepositoryError::InvalidTransition),
        };
        self.write_state(DurableState::Ready {
            confirmed: Some(pending),
            pending: None,
            attempts: 0,
        })
        .await?;
        Ok(pending.generation)
    }

    /// Atomically records a redacted rejection and requires explicit rollback.
    pub async fn reject_pending(
        &mut self,
        reason: RejectionReason,
    ) -> Result<DeviceState, RepositoryError<S::Error>> {
        let (previous, rejected) = match self.state {
            DurableState::Ready {
                confirmed,
                pending: Some(pending),
                ..
            } => (confirmed, pending),
            _ => return Err(RepositoryError::InvalidTransition),
        };
        self.write_state(DurableState::Rollback {
            previous,
            rejected,
            reason,
        })
        .await?;
        Ok(self.device_state())
    }

    /// Atomically restores the previous confirmed generation, if one existed.
    pub async fn complete_rollback(&mut self) -> Result<DeviceState, RepositoryError<S::Error>> {
        let previous = match self.state {
            DurableState::Rollback { previous, .. } => previous,
            _ => return Err(RepositoryError::InvalidTransition),
        };
        self.write_state(DurableState::Ready {
            confirmed: previous,
            pending: None,
            attempts: 0,
        })
        .await?;
        Ok(self.device_state())
    }

    /// Atomically marks logical factory reset as in progress.
    pub async fn begin_factory_reset(&mut self) -> Result<(), RepositoryError<S::Error>> {
        match self.state {
            DurableState::Unloaded => return Err(RepositoryError::InvalidTransition),
            DurableState::Reset => return Ok(()),
            DurableState::Ready { .. }
            | DurableState::Rollback { .. }
            | DurableState::Recovery(_) => {}
        }
        self.write_state(DurableState::Reset).await
    }

    /// Resumes the restartable reset deletion order and returns unprovisioned.
    pub async fn resume_factory_reset(&mut self) -> Result<DeviceState, RepositoryError<S::Error>> {
        if self.state != DurableState::Reset {
            return Err(RepositoryError::InvalidTransition);
        }
        self.store
            .delete(SLOT_A_KEY)
            .await
            .map_err(RepositoryError::Storage)?;
        self.store
            .delete(SLOT_B_KEY)
            .await
            .map_err(RepositoryError::Storage)?;
        self.store
            .delete(STATE_KEY)
            .await
            .map_err(RepositoryError::Storage)?;
        self.state = DurableState::Ready {
            confirmed: None,
            pending: None,
            attempts: 0,
        };
        Ok(self.device_state())
    }

    async fn write_state(&mut self, next: DurableState) -> Result<(), RepositoryError<S::Error>> {
        let encoded = encode_state(next).ok_or(RepositoryError::InvalidTransition)?;
        match self.store.put(STATE_KEY, &encoded).await {
            Ok(()) => {
                self.state = next;
                Ok(())
            }
            Err(error) => {
                self.state = DurableState::Recovery(RecoveryReason::StorageFailure);
                Err(RepositoryError::Storage(error))
            }
        }
    }
}

#[derive(Clone, Copy)]
enum RecordError {
    Corrupted,
    Incompatible,
}

fn encode_slot<E>(
    schema: SchemaVersion,
    generation: Generation,
    payload: &[u8],
    output: &mut [u8],
) -> Result<usize, RepositoryError<E>> {
    let len = SLOT_OVERHEAD_BYTES
        .checked_add(payload.len())
        .ok_or(RepositoryError::CandidateTooLarge)?;
    if payload.len() > MAX_CANDIDATE_BYTES {
        return Err(RepositoryError::CandidateTooLarge);
    }
    if output.len() < len {
        return Err(RepositoryError::BufferTooSmall);
    }
    output[..len].fill(0);
    output[..4].copy_from_slice(&SLOT_MAGIC);
    output[4..6].copy_from_slice(&PERSISTENT_FORMAT_VERSION.to_be_bytes());
    output[6..8].copy_from_slice(&schema.major.to_be_bytes());
    output[8..10].copy_from_slice(&schema.minor.to_be_bytes());
    output[12..16].copy_from_slice(&generation.get().to_be_bytes());
    output[16..18].copy_from_slice(&(payload.len() as u16).to_be_bytes());
    output[SLOT_HEADER_BYTES..SLOT_HEADER_BYTES + payload.len()].copy_from_slice(payload);
    let crc_offset = len - SLOT_CRC_BYTES;
    let crc = crc32(&output[..crc_offset]);
    output[crc_offset..len].copy_from_slice(&crc.to_be_bytes());
    Ok(len)
}

fn decode_slot(input: &[u8]) -> Result<StoredCandidate<'_>, RecordError> {
    if input.len() < SLOT_OVERHEAD_BYTES || input[..4] != SLOT_MAGIC {
        return Err(RecordError::Corrupted);
    }
    if u16::from_be_bytes([input[4], input[5]]) != PERSISTENT_FORMAT_VERSION {
        return Err(RecordError::Incompatible);
    }
    if input[10] != 0 || input[11] != 0 || input[18] != 0 || input[19] != 0 {
        return Err(RecordError::Corrupted);
    }
    let payload_len = usize::from(u16::from_be_bytes([input[16], input[17]]));
    if payload_len > MAX_CANDIDATE_BYTES || input.len() != SLOT_OVERHEAD_BYTES + payload_len {
        return Err(RecordError::Corrupted);
    }
    let crc_offset = input.len() - SLOT_CRC_BYTES;
    let expected_crc = u32::from_be_bytes([
        input[crc_offset],
        input[crc_offset + 1],
        input[crc_offset + 2],
        input[crc_offset + 3],
    ]);
    if crc32(&input[..crc_offset]) != expected_crc {
        return Err(RecordError::Corrupted);
    }
    let generation = Generation::new(u32::from_be_bytes([
        input[12], input[13], input[14], input[15],
    ]))
    .ok_or(RecordError::Corrupted)?;
    Ok(StoredCandidate {
        schema: SchemaVersion::new(
            u16::from_be_bytes([input[6], input[7]]),
            u16::from_be_bytes([input[8], input[9]]),
        ),
        generation,
        payload: &input[SLOT_HEADER_BYTES..crc_offset],
    })
}

fn encode_state(state: DurableState) -> Option<[u8; STATE_RECORD_BYTES]> {
    let (tag, confirmed, pending, attempts, reason) = match state {
        DurableState::Ready {
            confirmed,
            pending,
            attempts,
        } => (STATE_READY, confirmed, pending, attempts, 0),
        DurableState::Rollback {
            previous,
            rejected,
            reason,
        } => (
            STATE_ROLLBACK,
            previous,
            Some(rejected),
            0,
            encode_rejection_reason(reason),
        ),
        DurableState::Reset => (STATE_RESET, None, None, 0, 0),
        DurableState::Unloaded | DurableState::Recovery(_) => return None,
    };
    let mut output = [0; STATE_RECORD_BYTES];
    output[..4].copy_from_slice(&STATE_MAGIC);
    output[4..6].copy_from_slice(&PERSISTENT_FORMAT_VERSION.to_be_bytes());
    output[6] = tag;
    encode_reference(confirmed, &mut output[7..12]);
    encode_reference(pending, &mut output[12..17]);
    output[17] = attempts;
    output[18] = reason;
    let crc = crc32(&output[..20]);
    output[20..24].copy_from_slice(&crc.to_be_bytes());
    Some(output)
}

fn decode_state(input: &[u8]) -> Result<DurableState, RecordError> {
    if input.len() != STATE_RECORD_BYTES || input[..4] != STATE_MAGIC {
        return Err(RecordError::Corrupted);
    }
    if u16::from_be_bytes([input[4], input[5]]) != PERSISTENT_FORMAT_VERSION {
        return Err(RecordError::Incompatible);
    }
    if input[19] != 0
        || crc32(&input[..20]) != u32::from_be_bytes([input[20], input[21], input[22], input[23]])
    {
        return Err(RecordError::Corrupted);
    }
    let confirmed = decode_reference(&input[7..12])?;
    let pending = decode_reference(&input[12..17])?;
    match input[6] {
        STATE_READY if input[18] == 0 && (pending.is_some() || input[17] == 0) => {
            Ok(DurableState::Ready {
                confirmed,
                pending,
                attempts: input[17],
            })
        }
        STATE_ROLLBACK if pending.is_some() && input[17] == 0 => Ok(DurableState::Rollback {
            previous: confirmed,
            rejected: pending.ok_or(RecordError::Corrupted)?,
            reason: decode_rejection_reason(input[18])?,
        }),
        STATE_RESET
            if confirmed.is_none() && pending.is_none() && input[17] == 0 && input[18] == 0 =>
        {
            Ok(DurableState::Reset)
        }
        _ => Err(RecordError::Corrupted),
    }
}

fn encode_reference(reference: Option<SlotReference>, output: &mut [u8]) {
    if let Some(reference) = reference {
        output[0] = reference.slot.encoded();
        output[1..5].copy_from_slice(&reference.generation.get().to_be_bytes());
    } else {
        output.fill(0);
    }
}

fn decode_reference(input: &[u8]) -> Result<Option<SlotReference>, RecordError> {
    let generation = u32::from_be_bytes([input[1], input[2], input[3], input[4]]);
    match (input[0], generation) {
        (0, 0) => Ok(None),
        (slot, generation) => Ok(Some(SlotReference {
            slot: Slot::decode(slot).ok_or(RecordError::Corrupted)?,
            generation: Generation::new(generation).ok_or(RecordError::Corrupted)?,
        })),
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

const fn decode_rejection_reason(value: u8) -> Result<RejectionReason, RecordError> {
    match value {
        1 => Ok(RejectionReason::ApplyFailed),
        2 => Ok(RejectionReason::WifiUnavailable),
        3 => Ok(RejectionReason::NetworkUnavailable),
        4 => Ok(RejectionReason::VerificationFailed),
        5 => Ok(RejectionReason::AttemptsExhausted),
        _ => Err(RecordError::Corrupted),
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0_u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    extern crate std;

    use embassy_futures::block_on;
    use embedded_sdk_config::SchemaVersion;
    use embedded_sdk_storage::{Fetch, Key, KeyValueStore};
    use std::vec::Vec;

    use super::{
        DurableState, Repository, SLOT_A_KEY, SLOT_B_KEY, STATE_KEY, Slot, SlotReference,
        encode_slot, encode_state,
    };
    use crate::{DeviceState, Generation, RecoveryReason, RejectionReason};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestError {
        BufferTooSmall,
        Interrupted,
    }

    #[derive(Clone, Default)]
    struct MemoryStore {
        state: Option<Vec<u8>>,
        slot_a: Option<Vec<u8>>,
        slot_b: Option<Vec<u8>>,
    }

    impl MemoryStore {
        fn value(&self, key: Key) -> &Option<Vec<u8>> {
            match key {
                STATE_KEY => &self.state,
                SLOT_A_KEY => &self.slot_a,
                SLOT_B_KEY => &self.slot_b,
                _ => unreachable!(),
            }
        }

        fn value_mut(&mut self, key: Key) -> &mut Option<Vec<u8>> {
            match key {
                STATE_KEY => &mut self.state,
                SLOT_A_KEY => &mut self.slot_a,
                SLOT_B_KEY => &mut self.slot_b,
                _ => unreachable!(),
            }
        }
    }

    struct FaultStore {
        inner: MemoryStore,
        fail_at: Option<usize>,
        fail_after_mutation: bool,
        calls: usize,
    }

    impl FaultStore {
        fn new(inner: MemoryStore) -> Self {
            Self {
                inner,
                fail_at: None,
                fail_after_mutation: false,
                calls: 0,
            }
        }

        fn arm(&mut self, fail_at: usize, fail_after_mutation: bool) {
            self.fail_at = Some(fail_at);
            self.fail_after_mutation = fail_after_mutation;
            self.calls = 0;
        }

        fn disarm(&mut self) {
            self.fail_at = None;
            self.calls = 0;
        }

        fn failure_mode(&mut self) -> Option<bool> {
            self.calls += 1;
            (self.fail_at == Some(self.calls)).then_some(self.fail_after_mutation)
        }
    }

    impl KeyValueStore for FaultStore {
        type Error = TestError;

        async fn get(&mut self, key: Key, output: &mut [u8]) -> Result<Fetch, Self::Error> {
            if self.failure_mode().is_some() {
                return Err(TestError::Interrupted);
            }
            self.inner.get(key, output).await
        }

        async fn put(&mut self, key: Key, value: &[u8]) -> Result<(), Self::Error> {
            match self.failure_mode() {
                Some(false) => Err(TestError::Interrupted),
                Some(true) => {
                    self.inner.put(key, value).await?;
                    Err(TestError::Interrupted)
                }
                None => self.inner.put(key, value).await,
            }
        }

        async fn delete(&mut self, key: Key) -> Result<(), Self::Error> {
            match self.failure_mode() {
                Some(false) => Err(TestError::Interrupted),
                Some(true) => {
                    self.inner.delete(key).await?;
                    Err(TestError::Interrupted)
                }
                None => self.inner.delete(key).await,
            }
        }

        async fn clear(&mut self) -> Result<(), Self::Error> {
            match self.failure_mode() {
                Some(false) => Err(TestError::Interrupted),
                Some(true) => {
                    self.inner.clear().await?;
                    Err(TestError::Interrupted)
                }
                None => self.inner.clear().await,
            }
        }
    }

    impl KeyValueStore for MemoryStore {
        type Error = TestError;

        async fn get(&mut self, key: Key, output: &mut [u8]) -> Result<Fetch, Self::Error> {
            let Some(value) = self.value(key) else {
                return Ok(Fetch::NotFound);
            };
            if output.len() < value.len() {
                return Err(TestError::BufferTooSmall);
            }
            output[..value.len()].copy_from_slice(value);
            Ok(Fetch::Found { len: value.len() })
        }

        async fn put(&mut self, key: Key, value: &[u8]) -> Result<(), Self::Error> {
            *self.value_mut(key) = Some(value.to_vec());
            Ok(())
        }

        async fn delete(&mut self, key: Key) -> Result<(), Self::Error> {
            *self.value_mut(key) = None;
            Ok(())
        }

        async fn clear(&mut self) -> Result<(), Self::Error> {
            self.state = None;
            self.slot_a = None;
            self.slot_b = None;
            Ok(())
        }
    }

    fn scratch() -> [u8; 1048] {
        [0; 1048]
    }

    #[test]
    fn stages_recovers_loads_and_confirms_a_candidate() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut scratch = scratch();
            assert_eq!(
                repository.recover(&mut scratch).await.unwrap(),
                DeviceState::Unprovisioned
            );

            let generation = repository
                .stage_candidate(SchemaVersion::new(1, 0), b"secret candidate", &mut scratch)
                .await
                .unwrap();
            assert_eq!(generation.get(), 1);
            let loaded = repository.load_pending(&mut scratch).await.unwrap();
            assert_eq!(loaded.schema, SchemaVersion::new(1, 0));
            assert_eq!(loaded.generation, generation);
            assert_eq!(loaded.payload, b"secret candidate");

            let mut repository = Repository::new(repository.into_store());
            assert!(matches!(
                repository.recover(&mut scratch).await.unwrap(),
                DeviceState::PendingVerification {
                    previous_generation: None,
                    pending_generation,
                    attempts: 0,
                } if pending_generation == generation
            ));
            assert_eq!(repository.confirm_pending().await.unwrap(), generation);
            assert_eq!(
                repository.device_state(),
                DeviceState::Provisioned {
                    confirmed_generation: generation
                }
            );
            let loaded = repository.load_confirmed(&mut scratch).await.unwrap();
            assert_eq!(loaded.generation, generation);
            assert_eq!(loaded.payload, b"secret candidate");
        });
    }

    #[test]
    fn replacement_uses_the_slot_opposite_the_confirmed_generation() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut scratch = scratch();
            repository.recover(&mut scratch).await.unwrap();
            repository
                .stage_candidate(SchemaVersion::new(1, 0), b"first", &mut scratch)
                .await
                .unwrap();
            repository.confirm_pending().await.unwrap();
            let first_slot = repository.store.slot_a.clone();

            let second = repository
                .stage_candidate(SchemaVersion::new(1, 0), b"second", &mut scratch)
                .await
                .unwrap();
            assert_eq!(second.get(), 2);
            assert_eq!(repository.store.slot_a, first_slot);
            assert!(repository.store.slot_b.is_some());
        });
    }

    #[test]
    fn corrupt_referenced_slot_requires_recovery_without_erasing_it() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut scratch = scratch();
            repository.recover(&mut scratch).await.unwrap();
            repository
                .stage_candidate(SchemaVersion::new(1, 0), b"candidate", &mut scratch)
                .await
                .unwrap();
            let mut store = repository.into_store();
            store.slot_a.as_mut().unwrap()[20] ^= 0x55;

            let mut repository = Repository::new(store);
            assert_eq!(
                repository.recover(&mut scratch).await.unwrap(),
                DeviceState::RecoveryRequired {
                    reason: RecoveryReason::CorruptedRecord
                }
            );
            assert!(repository.store.slot_a.is_some());
        });
    }

    #[test]
    fn explicit_factory_reset_can_clear_recovery_state() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut scratch = scratch();
            repository.recover(&mut scratch).await.unwrap();
            repository
                .stage_candidate(SchemaVersion::new(1, 0), b"candidate", &mut scratch)
                .await
                .unwrap();
            let mut store = repository.into_store();
            store.slot_a.as_mut().unwrap()[20] ^= 0x55;

            let mut repository = Repository::new(store);
            assert!(matches!(
                repository.recover(&mut scratch).await.unwrap(),
                DeviceState::RecoveryRequired { .. }
            ));
            repository.begin_factory_reset().await.unwrap();
            assert_eq!(
                repository.resume_factory_reset().await.unwrap(),
                DeviceState::Unprovisioned
            );
            assert!(repository.store.state.is_none());
            assert!(repository.store.slot_a.is_none());
            assert!(repository.store.slot_b.is_none());
        });
    }

    #[test]
    fn attempts_exhaustion_requires_then_completes_rollback() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut scratch = scratch();
            repository.recover(&mut scratch).await.unwrap();
            repository
                .stage_candidate(SchemaVersion::new(1, 0), b"candidate", &mut scratch)
                .await
                .unwrap();
            repository.start_verification_attempt(1).await.unwrap();
            let state = repository.start_verification_attempt(1).await.unwrap();
            assert!(matches!(
                state,
                DeviceState::RollbackRequired {
                    previous_generation: None,
                    reason: RejectionReason::AttemptsExhausted,
                    ..
                }
            ));
            assert_eq!(
                repository.complete_rollback().await.unwrap(),
                DeviceState::Unprovisioned
            );
        });
    }

    #[test]
    fn reset_marker_survives_reopen_and_deletion_is_restartable() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut scratch = scratch();
            repository.recover(&mut scratch).await.unwrap();
            repository
                .stage_candidate(SchemaVersion::new(1, 0), b"candidate", &mut scratch)
                .await
                .unwrap();
            repository.begin_factory_reset().await.unwrap();

            let mut repository = Repository::new(repository.into_store());
            assert_eq!(
                repository.recover(&mut scratch).await.unwrap(),
                DeviceState::ResetInProgress
            );
            assert_eq!(
                repository.resume_factory_reset().await.unwrap(),
                DeviceState::Unprovisioned
            );
            assert!(repository.store.state.is_none());
            assert!(repository.store.slot_a.is_none());
            assert!(repository.store.slot_b.is_none());
        });
    }

    #[test]
    fn interruption_at_each_stage_boundary_recovers_to_old_or_complete_pending() {
        for (fail_at, after_mutation) in [(1, false), (1, true), (2, false), (3, false), (3, true)]
        {
            block_on(async {
                let store = FaultStore::new(MemoryStore::default());
                let mut repository = Repository::new(store);
                let mut scratch = scratch();
                repository.recover(&mut scratch).await.unwrap();
                repository.store.arm(fail_at, after_mutation);
                assert!(
                    repository
                        .stage_candidate(
                            SchemaVersion::new(1, 0),
                            b"complete candidate",
                            &mut scratch,
                        )
                        .await
                        .is_err()
                );

                let mut store = repository.into_store();
                store.disarm();
                let mut repository = Repository::new(store);
                let state = repository.recover(&mut scratch).await.unwrap();
                if fail_at == 3 && after_mutation {
                    assert!(matches!(state, DeviceState::PendingVerification { .. }));
                    let loaded = repository.load_pending(&mut scratch).await.unwrap();
                    assert_eq!(loaded.payload, b"complete candidate");
                } else {
                    assert_eq!(state, DeviceState::Unprovisioned);
                }
            });
        }
    }

    #[test]
    fn interruption_at_each_reset_delete_is_restartable() {
        for (fail_at, after_mutation) in [
            (1, false),
            (1, true),
            (2, false),
            (2, true),
            (3, false),
            (3, true),
        ] {
            block_on(async {
                let store = FaultStore::new(MemoryStore::default());
                let mut repository = Repository::new(store);
                let mut scratch = scratch();
                repository.recover(&mut scratch).await.unwrap();
                repository
                    .stage_candidate(SchemaVersion::new(1, 0), b"candidate", &mut scratch)
                    .await
                    .unwrap();
                repository.begin_factory_reset().await.unwrap();
                repository.store.arm(fail_at, after_mutation);
                assert!(repository.resume_factory_reset().await.is_err());

                let mut store = repository.into_store();
                store.disarm();
                let mut repository = Repository::new(store);
                let state = repository.recover(&mut scratch).await.unwrap();
                if fail_at == 3 && after_mutation {
                    assert_eq!(state, DeviceState::Unprovisioned);
                } else {
                    assert_eq!(state, DeviceState::ResetInProgress);
                    assert_eq!(
                        repository.resume_factory_reset().await.unwrap(),
                        DeviceState::Unprovisioned
                    );
                }
            });
        }
    }

    #[test]
    fn interrupted_confirmation_keeps_old_and_pending_or_promotes_complete_candidate() {
        let base = block_on(async {
            let store = FaultStore::new(MemoryStore::default());
            let mut repository = Repository::new(store);
            let mut scratch = scratch();
            repository.recover(&mut scratch).await.unwrap();
            repository
                .stage_candidate(SchemaVersion::new(1, 0), b"confirmed", &mut scratch)
                .await
                .unwrap();
            repository.confirm_pending().await.unwrap();
            repository
                .stage_candidate(SchemaVersion::new(1, 0), b"replacement", &mut scratch)
                .await
                .unwrap();
            repository.into_store().inner
        });

        for after_mutation in [false, true] {
            block_on(async {
                let mut repository = Repository::new(FaultStore::new(base.clone()));
                let mut scratch = scratch();
                repository.recover(&mut scratch).await.unwrap();
                repository.store.arm(1, after_mutation);
                assert!(repository.confirm_pending().await.is_err());

                let mut store = repository.into_store();
                assert!(store.inner.slot_a.is_some());
                assert!(store.inner.slot_b.is_some());
                store.disarm();
                let mut repository = Repository::new(store);
                let state = repository.recover(&mut scratch).await.unwrap();
                if after_mutation {
                    assert!(matches!(
                        state,
                        DeviceState::Provisioned {
                            confirmed_generation
                        } if confirmed_generation.get() == 2
                    ));
                } else {
                    assert!(matches!(
                        state,
                        DeviceState::PendingVerification {
                            previous_generation: Some(previous),
                            pending_generation,
                            ..
                        } if previous.get() == 1 && pending_generation.get() == 2
                    ));
                }
            });
        }
    }

    #[test]
    fn generation_wrap_requires_recovery() {
        block_on(async {
            let generation = Generation::new(u32::MAX).unwrap();
            let reference = SlotReference {
                slot: Slot::A,
                generation,
            };
            let mut slot = scratch();
            let slot_len = encode_slot::<TestError>(
                SchemaVersion::new(1, 0),
                generation,
                b"last generation",
                &mut slot,
            )
            .unwrap();
            let state = encode_state(DurableState::Ready {
                confirmed: Some(reference),
                pending: None,
                attempts: 0,
            })
            .unwrap();
            let store = MemoryStore {
                state: Some(state.to_vec()),
                slot_a: Some(slot[..slot_len].to_vec()),
                slot_b: None,
            };
            let mut repository = Repository::new(store);
            repository.recover(&mut slot).await.unwrap();
            assert!(
                repository
                    .stage_candidate(SchemaVersion::new(1, 0), b"must not wrap", &mut slot,)
                    .await
                    .is_err()
            );
            assert_eq!(
                repository.device_state(),
                DeviceState::RecoveryRequired {
                    reason: RecoveryReason::GenerationExhausted
                }
            );
        });
    }
}
