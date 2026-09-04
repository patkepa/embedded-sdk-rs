#![no_std]
#![forbid(unsafe_code)]
#![doc = "Transport-neutral device provisioning contracts and transaction model."]

mod authority;
mod codec;
mod protocol;
mod repository;
mod service;
mod status;

pub use authority::{
    Authority, AuthorizationError, AuthorizationPolicy, OperationKind, SessionContext,
};
pub use codec::{
    CodecError, WireResponse, decode_request, decode_response, encode_request, encode_response,
};
pub use protocol::{
    Capabilities, CommitDisposition, Generation, Request, RequestId, Response, ResponseKind,
    SessionId, TransactionId, TransactionState,
};
pub use repository::{
    MAX_SLOT_RECORD_BYTES, PROVISIONING_NAMESPACE, Repository, RepositoryError, SLOT_A_KEY,
    SLOT_B_KEY, STATE_KEY, StoredCandidate,
};
pub use service::{Action, ProvisioningCandidate, Service, ServiceConfig, ServiceError};
pub use status::{DeviceState, ErrorKind, RecoveryReason, RejectionReason, Status};

/// Current major version of the transport-neutral request model.
pub const PROTOCOL_VERSION_MAJOR: u16 = 1;
/// Current minor version of the transport-neutral request model.
pub const PROTOCOL_VERSION_MINOR: u16 = 0;

/// Maximum complete product candidate carried by the initial wire schema.
pub const MAX_CANDIDATE_BYTES: usize = 1024;
/// Maximum decoded CBOR request envelope.
pub const MAX_REQUEST_BYTES: usize = 1088;
/// Maximum encoded CBOR response envelope.
pub const MAX_RESPONSE_BYTES: usize = 256;
/// Maximum serial or BLE-reassembled frame including transport overhead.
pub const MAX_TRANSPORT_FRAME_BYTES: usize = 1104;
