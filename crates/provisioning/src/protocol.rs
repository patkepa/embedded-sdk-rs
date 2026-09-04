use core::num::{NonZeroU32, NonZeroU64};

use embedded_sdk_config::SchemaVersion;

use crate::OperationKind;

/// Nonzero identifier for one transport connection or authenticated session.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(NonZeroU64);

impl SessionId {
    /// Creates a session identifier, rejecting the reserved zero value.
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Returns the fixed-width wire value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Nonzero client-selected identifier for one provisioning transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TransactionId(NonZeroU64);

impl TransactionId {
    /// Creates a transaction identifier, rejecting the reserved zero value.
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Returns the fixed-width wire value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Nonzero client-selected identifier used for idempotent request retries.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RequestId(NonZeroU32);

impl RequestId {
    /// Creates a request identifier, rejecting the reserved zero value.
    #[must_use]
    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Returns the fixed-width wire value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// Nonzero durable product-configuration generation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Generation(NonZeroU32);

impl Generation {
    /// Creates a generation, rejecting zero as the unassigned value.
    #[must_use]
    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Returns the persistent fixed-width value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// One decoded transport-neutral provisioning request.
///
/// Candidate bytes are borrowed from a bounded transport buffer. They are not
/// included in debug output, responses, or status.
pub enum Request<'a> {
    /// Reports supported protocol and bounded-message capabilities.
    Capabilities {
        /// Identifier used to correlate the response.
        request_id: RequestId,
    },
    /// Reports redacted device and transaction status.
    Status {
        /// Identifier used to correlate the response.
        request_id: RequestId,
    },
    /// Acquires the single mutable transaction for this session.
    Begin {
        /// Identifier used for idempotent retry handling.
        request_id: RequestId,
        /// Client-selected transaction identifier.
        transaction_id: TransactionId,
        /// Product schema the candidate will use.
        schema: SchemaVersion,
    },
    /// Supplies one complete bounded encoded product candidate.
    SubmitCandidate {
        /// Identifier used for idempotent retry handling.
        request_id: RequestId,
        /// Identifier of the active transaction.
        transaction_id: TransactionId,
        /// Complete encoded candidate borrowed from the transport buffer.
        encoded: &'a [u8],
    },
    /// Runs product semantic validation on the submitted candidate.
    Validate {
        /// Identifier used for idempotent retry handling.
        request_id: RequestId,
        /// Identifier of the active transaction.
        transaction_id: TransactionId,
    },
    /// Durably stages the validated candidate as pending.
    Commit {
        /// Identifier used for idempotent retry handling.
        request_id: RequestId,
        /// Identifier of the active transaction.
        transaction_id: TransactionId,
    },
    /// Releases the calling session's transient transaction.
    Abort {
        /// Identifier used for idempotent retry handling.
        request_id: RequestId,
        /// Identifier of the active transaction.
        transaction_id: TransactionId,
    },
    /// Starts the authorized restartable logical reset procedure.
    FactoryReset {
        /// Identifier used for idempotent retry handling.
        request_id: RequestId,
    },
}

impl Request<'_> {
    /// Returns the request identifier shared by all operations.
    #[must_use]
    pub const fn request_id(&self) -> RequestId {
        match self {
            Self::Capabilities { request_id }
            | Self::Status { request_id }
            | Self::Begin { request_id, .. }
            | Self::SubmitCandidate { request_id, .. }
            | Self::Validate { request_id, .. }
            | Self::Commit { request_id, .. }
            | Self::Abort { request_id, .. }
            | Self::FactoryReset { request_id } => *request_id,
        }
    }

    /// Returns the transaction identifier for transaction-scoped operations.
    #[must_use]
    pub const fn transaction_id(&self) -> Option<TransactionId> {
        match self {
            Self::Begin { transaction_id, .. }
            | Self::SubmitCandidate { transaction_id, .. }
            | Self::Validate { transaction_id, .. }
            | Self::Commit { transaction_id, .. }
            | Self::Abort { transaction_id, .. } => Some(*transaction_id),
            Self::Capabilities { .. } | Self::Status { .. } | Self::FactoryReset { .. } => None,
        }
    }

    /// Returns the broad class used for early authorization.
    #[must_use]
    pub const fn operation_kind(&self) -> OperationKind {
        match self {
            Self::Capabilities { .. } | Self::Status { .. } => OperationKind::Inspect,
            Self::Begin { .. }
            | Self::SubmitCandidate { .. }
            | Self::Validate { .. }
            | Self::Commit { .. } => OperationKind::Configure,
            Self::Abort { .. } => OperationKind::Abort,
            Self::FactoryReset { .. } => OperationKind::FactoryReset,
        }
    }
}

/// Redacted state of the single in-memory provisioning transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransactionState {
    /// No session owns a mutable transaction.
    Idle,
    /// A session owns a transaction but has not submitted a candidate.
    Owned {
        /// Owning authenticated session.
        session_id: SessionId,
        /// Active transaction identifier.
        transaction_id: TransactionId,
        /// Declared product schema.
        schema: SchemaVersion,
    },
    /// A complete candidate is held in bounded transient memory.
    CandidateReceived {
        /// Owning authenticated session.
        session_id: SessionId,
        /// Active transaction identifier.
        transaction_id: TransactionId,
    },
    /// The transient candidate passed product validation.
    CandidateValidated {
        /// Owning authenticated session.
        session_id: SessionId,
        /// Active transaction identifier.
        transaction_id: TransactionId,
    },
    /// Durable commit has started and must be driven to completion.
    CommitInProgress {
        /// Owning authenticated session.
        session_id: SessionId,
        /// Active transaction identifier.
        transaction_id: TransactionId,
    },
    /// The candidate is durable and awaiting boot-time verification.
    Committed {
        /// Pending durable generation.
        pending_generation: Generation,
    },
}

/// Bounded capabilities reported without exposing product configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    /// Supported protocol major version.
    pub protocol_major: u16,
    /// Supported protocol minor version.
    pub protocol_minor: u16,
    /// Maximum complete encoded candidate accepted by the service.
    pub max_candidate_bytes: u32,
    /// Whether a durable candidate requires reboot before verification.
    pub reboot_to_apply: bool,
}

/// Product application behavior after a candidate becomes durable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CommitDisposition {
    /// Firmware must reboot before applying and verifying the candidate.
    RebootRequired {
        /// Newly pending durable generation.
        pending_generation: Generation,
    },
    /// Product firmware has scheduled coordinated live application.
    ApplyScheduled {
        /// Newly pending durable generation.
        pending_generation: Generation,
    },
}

/// Redacted successful response payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ResponseKind {
    /// Protocol and capacity information.
    Capabilities(Capabilities),
    /// Current redacted device and transaction state.
    Status(crate::Status),
    /// The session acquired the mutable transaction.
    TransactionBegun,
    /// One complete bounded candidate was accepted into transient memory.
    CandidateReceived,
    /// The product candidate passed semantic validation.
    CandidateValidated,
    /// The candidate is durably pending application and verification.
    Committed(CommitDisposition),
    /// Transient transaction state was released.
    Aborted,
    /// Restartable logical reset completed.
    FactoryReset,
}

/// Successful response correlated to one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Response {
    /// Identifier copied from the request.
    pub request_id: RequestId,
    /// Redacted response payload.
    pub kind: ResponseKind,
}

#[cfg(test)]
mod tests {
    use embedded_sdk_config::SchemaVersion;

    use super::{Request, RequestId, SessionId, TransactionId};
    use crate::OperationKind;

    #[test]
    fn identifiers_reserve_zero() {
        assert!(SessionId::new(0).is_none());
        assert!(TransactionId::new(0).is_none());
        assert!(RequestId::new(0).is_none());
        assert_eq!(RequestId::new(7).unwrap().get(), 7);
    }

    #[test]
    fn request_metadata_is_available_without_inspecting_candidate() {
        let request_id = RequestId::new(3).unwrap();
        let transaction_id = TransactionId::new(9).unwrap();
        let request = Request::Begin {
            request_id,
            transaction_id,
            schema: SchemaVersion::new(1, 0),
        };

        assert_eq!(request.request_id(), request_id);
        assert_eq!(request.transaction_id(), Some(transaction_id));
        assert_eq!(request.operation_kind(), OperationKind::Configure);
    }
}
