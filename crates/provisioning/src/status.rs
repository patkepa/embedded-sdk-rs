use crate::{Generation, TransactionState};

/// Bounded reason a candidate was rejected during activation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RejectionReason {
    /// The product configuration could not be applied.
    ApplyFailed,
    /// Wi-Fi did not associate before its deadline.
    WifiUnavailable,
    /// The network stack did not obtain usable IP configuration in time.
    NetworkUnavailable,
    /// The configured verification probe did not succeed in time.
    VerificationFailed,
    /// The maximum verification attempt count was reached.
    AttemptsExhausted,
}

/// Bounded reason automatic recovery cannot safely select a configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RecoveryReason {
    /// A provisioning record failed structural or integrity validation.
    CorruptedRecord,
    /// A record uses a persistent schema this firmware cannot read.
    IncompatiblePersistentVersion,
    /// The state record refers to a missing slot.
    MissingSlot,
    /// State and slot generation numbers do not agree.
    GenerationMismatch,
    /// Allocating another generation would wrap the fixed-width counter.
    GenerationExhausted,
    /// A persistent transition failed without a safely inferable outcome.
    StorageFailure,
}

/// Externally visible provisioning state with no configuration contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DeviceState {
    /// No usable confirmed or pending product configuration exists.
    Unprovisioned,
    /// One verified product configuration is active.
    Provisioned {
        /// Active confirmed generation.
        confirmed_generation: Generation,
    },
    /// A complete durable candidate awaits or is undergoing verification.
    PendingVerification {
        /// Previous working generation retained for rollback.
        previous_generation: Option<Generation>,
        /// Candidate generation being verified.
        pending_generation: Generation,
        /// Number of durable verification attempts already started.
        attempts: u8,
    },
    /// Verification failed and the repository must complete rollback.
    RollbackRequired {
        /// Previous working generation, if one exists.
        previous_generation: Option<Generation>,
        /// Candidate generation that was rejected.
        rejected_generation: Generation,
        /// Redacted failure category.
        reason: RejectionReason,
    },
    /// Stored state cannot be safely interpreted or changed automatically.
    RecoveryRequired {
        /// Redacted recovery category.
        reason: RecoveryReason,
    },
    /// Logical reset is deleting records in a restartable order.
    ResetInProgress,
}

/// Redacted public snapshot returned by status operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Status {
    /// Current durable device state.
    pub device: DeviceState,
    /// Current transient transaction state.
    pub transaction: TransactionState,
}

/// Stable redacted service error category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The request wire version is not supported.
    UnsupportedProtocolVersion,
    /// The product schema is not supported.
    UnsupportedProductSchema,
    /// A fixed-width identifier used the reserved zero value.
    InvalidIdentifier,
    /// A bounded input exceeded its documented maximum.
    CapacityExceeded,
    /// A request is malformed without exposing its input bytes.
    MalformedRequest,
    /// The session is not authorized for the operation.
    Unauthorized,
    /// Another session owns the mutable transaction.
    TransactionBusy,
    /// The request does not refer to the active transaction.
    TransactionMismatch,
    /// The operation is invalid in the current transaction state.
    InvalidTransition,
    /// A request identifier was reused with different request contents.
    RequestConflict,
    /// Product candidate decoding failed.
    CandidateDecode,
    /// Product candidate semantic validation failed.
    CandidateValidation,
    /// Persistent storage could not complete the operation.
    Storage,
    /// Durable state requires explicit recovery.
    RecoveryRequired,
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::{DeviceState, Status};
    use crate::TransactionState;

    #[test]
    fn status_debug_is_structural_and_redacted() {
        let status = Status {
            device: DeviceState::Unprovisioned,
            transaction: TransactionState::Idle,
        };
        let rendered = std::format!("{status:?}");

        assert_eq!(
            rendered,
            "Status { device: Unprovisioned, transaction: Idle }"
        );
    }
}
