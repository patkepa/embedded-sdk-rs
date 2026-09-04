use embedded_sdk_config::SchemaVersion;
use zeroize::Zeroize;

use crate::{
    Authority, AuthorizationPolicy, Capabilities, CommitDisposition, DeviceState, ErrorKind,
    Generation, PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR, Request, RequestId, Response,
    ResponseKind, SessionContext, SessionId, Status, TransactionId, TransactionState,
};

/// Product configuration decoded and validated by the portable service.
///
/// Implementations must erase secret-bearing storage in [`Zeroize::zeroize`].
/// Decode and validation errors must not contain or format input bytes.
pub trait ProvisioningCandidate: Sized + Zeroize {
    /// Redacted candidate decoding failure.
    type DecodeError;
    /// Redacted product semantic validation failure.
    type ValidationError;

    /// Decodes one complete bounded product candidate.
    fn decode(version: SchemaVersion, bytes: &[u8]) -> Result<Self, Self::DecodeError>;

    /// Validates product invariants and field-level authority policy.
    fn validate_for(&self, authority: Authority) -> Result<(), Self::ValidationError>;
}

/// Fixed service behavior independent of transports and hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceConfig {
    /// Inactivity ticks after which a transient transaction is discarded.
    pub transaction_timeout_ticks: u64,
    /// Whether durable candidates require reboot before application.
    pub reboot_to_apply: bool,
}

impl ServiceConfig {
    /// Creates service configuration, rejecting a zero transaction timeout.
    #[must_use]
    pub const fn new(transaction_timeout_ticks: u64, reboot_to_apply: bool) -> Option<Self> {
        if transaction_timeout_ticks == 0 {
            None
        } else {
            Some(Self {
                transaction_timeout_ticks,
                reboot_to_apply,
            })
        }
    }
}

/// Redacted service failure correlated to the request that caused it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceError {
    /// Identifier copied from the request.
    pub request_id: RequestId,
    /// Stable failure category with no candidate data.
    pub kind: ErrorKind,
}

/// Work that must be completed by the service owner before responding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Action {
    /// A complete redacted response can be returned immediately.
    Respond(Response),
    /// Persist [`Service::candidate_for_commit`] using the repository commit sequence.
    CommitCandidate,
    /// Run the repository's restartable logical factory-reset sequence.
    FactoryReset,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveTransaction {
    session_id: SessionId,
    transaction_id: TransactionId,
    schema: SchemaVersion,
    last_activity_tick: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Owned(ActiveTransaction),
    CandidateReceived(ActiveTransaction),
    CandidateValidated(ActiveTransaction),
    CommitInProgress(ActiveTransaction),
    Committed(Generation),
    ResetInProgress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationSignature {
    Begin {
        transaction_id: TransactionId,
        schema: SchemaVersion,
    },
    SubmitCandidate {
        transaction_id: TransactionId,
    },
    Validate {
        transaction_id: TransactionId,
    },
    Commit {
        transaction_id: TransactionId,
    },
    Abort {
        transaction_id: TransactionId,
    },
    FactoryReset,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Replay {
    session_id: SessionId,
    request_id: RequestId,
    signature: MutationSignature,
    result: Result<Response, ServiceError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingEffect {
    Commit {
        session_id: SessionId,
        request_id: RequestId,
        transaction_id: TransactionId,
    },
    FactoryReset {
        session_id: SessionId,
        request_id: RequestId,
    },
}

/// Allocation-free owner of one transport-neutral provisioning transaction.
///
/// Calls are serialized through `&mut self`. Repository mutations are exposed
/// as [`Action`] values so the caller can drive each durable operation to
/// completion without making this crate depend on a storage implementation.
pub struct Service<C, P, const MAX_CANDIDATE: usize>
where
    C: ProvisioningCandidate,
    P: AuthorizationPolicy,
{
    policy: P,
    config: ServiceConfig,
    device_state: DeviceState,
    phase: Phase,
    candidate_bytes: [u8; MAX_CANDIDATE],
    candidate_len: usize,
    candidate: Option<C>,
    replay: Option<Replay>,
    pending_effect: Option<PendingEffect>,
}

impl<C, P, const MAX_CANDIDATE: usize> Service<C, P, MAX_CANDIDATE>
where
    C: ProvisioningCandidate,
    P: AuthorizationPolicy,
{
    /// Creates a service around recovered redacted device state.
    #[must_use]
    pub fn new(policy: P, config: ServiceConfig, device_state: DeviceState) -> Self {
        Self {
            policy,
            config,
            device_state,
            phase: Phase::Idle,
            candidate_bytes: [0; MAX_CANDIDATE],
            candidate_len: 0,
            candidate: None,
            replay: None,
            pending_effect: None,
        }
    }

    /// Returns the current redacted service status.
    #[must_use]
    pub fn status(&self) -> Status {
        Status {
            device: self.device_state,
            transaction: self.transaction_state(),
        }
    }

    /// Expires an inactive transient transaction at `now_tick`.
    ///
    /// Commit and reset effects are never cancelled by timeout because their
    /// persistent outcome must first be resolved by the repository owner.
    pub fn expire(&mut self, now_tick: u64) {
        let active = match self.phase {
            Phase::Owned(active)
            | Phase::CandidateReceived(active)
            | Phase::CandidateValidated(active) => Some(active),
            Phase::Idle
            | Phase::CommitInProgress(_)
            | Phase::Committed(_)
            | Phase::ResetInProgress => None,
        };

        if active.is_some_and(|active| {
            now_tick.saturating_sub(active.last_activity_tick)
                >= self.config.transaction_timeout_ticks
        }) {
            self.clear_transient();
            self.phase = Phase::Idle;
            self.replay = None;
        }
    }

    /// Handles one decoded request after expiring inactive transient state.
    pub fn handle(
        &mut self,
        session: &SessionContext,
        request: Request<'_>,
        now_tick: u64,
    ) -> Result<Action, ServiceError> {
        self.expire(now_tick);
        let request_id = request.request_id();

        self.policy
            .authorize(session, request.operation_kind(), self.device_state)
            .map_err(|_| ServiceError {
                request_id,
                kind: ErrorKind::Unauthorized,
            })?;

        if let Some(replayed) = self.replay_for(session, &request) {
            return replayed.map(Action::Respond);
        }

        match request {
            Request::Capabilities { request_id } => Ok(Action::Respond(Response {
                request_id,
                kind: ResponseKind::Capabilities(Capabilities {
                    protocol_major: PROTOCOL_VERSION_MAJOR,
                    protocol_minor: PROTOCOL_VERSION_MINOR,
                    max_candidate_bytes: u32::try_from(MAX_CANDIDATE).unwrap_or(u32::MAX),
                    reboot_to_apply: self.config.reboot_to_apply,
                }),
            })),
            Request::Status { request_id } => Ok(Action::Respond(Response {
                request_id,
                kind: ResponseKind::Status(self.status()),
            })),
            Request::Begin {
                request_id,
                transaction_id,
                schema,
            } => self.begin(*session, request_id, transaction_id, schema, now_tick),
            Request::SubmitCandidate {
                request_id,
                transaction_id,
                encoded,
            } => self.submit(*session, request_id, transaction_id, encoded, now_tick),
            Request::Validate {
                request_id,
                transaction_id,
            } => self.validate(*session, request_id, transaction_id, now_tick),
            Request::Commit {
                request_id,
                transaction_id,
            } => self.start_commit(*session, request_id, transaction_id, now_tick),
            Request::Abort {
                request_id,
                transaction_id,
            } => self.abort(*session, request_id, transaction_id),
            Request::FactoryReset { request_id } => self.start_factory_reset(*session, request_id),
        }
    }

    /// Returns the validated candidate while a commit effect is pending.
    #[must_use]
    pub fn candidate_for_commit(&self) -> Option<&C> {
        match (self.phase, self.pending_effect) {
            (Phase::CommitInProgress(_), Some(PendingEffect::Commit { .. })) => {
                self.candidate.as_ref()
            }
            _ => None,
        }
    }

    /// Completes the pending repository commit with its assigned generation.
    pub fn complete_commit(&mut self, generation: Generation) -> Result<Response, ErrorKind> {
        let (active, session_id, request_id, transaction_id) =
            match (self.phase, self.pending_effect) {
                (
                    Phase::CommitInProgress(active),
                    Some(PendingEffect::Commit {
                        session_id,
                        request_id,
                        transaction_id,
                    }),
                ) => (active, session_id, request_id, transaction_id),
                _ => return Err(ErrorKind::InvalidTransition),
            };

        if active.session_id != session_id || active.transaction_id != transaction_id {
            return Err(ErrorKind::TransactionMismatch);
        }

        let previous_generation = match self.device_state {
            DeviceState::Provisioned {
                confirmed_generation,
            } => Some(confirmed_generation),
            _ => None,
        };
        self.device_state = DeviceState::PendingVerification {
            previous_generation,
            pending_generation: generation,
            attempts: 0,
        };
        self.phase = Phase::Committed(generation);
        self.pending_effect = None;
        self.clear_transient();

        let disposition = if self.config.reboot_to_apply {
            CommitDisposition::RebootRequired {
                pending_generation: generation,
            }
        } else {
            CommitDisposition::ApplyScheduled {
                pending_generation: generation,
            }
        };
        let response = Response {
            request_id,
            kind: ResponseKind::Committed(disposition),
        };
        self.replay = Some(Replay {
            session_id,
            request_id,
            signature: MutationSignature::Commit { transaction_id },
            result: Ok(response),
        });
        Ok(response)
    }

    /// Resolves a failed repository commit and retains the validated candidate.
    pub fn fail_commit(&mut self) -> Result<ServiceError, ErrorKind> {
        let (active, session_id, request_id, transaction_id) =
            match (self.phase, self.pending_effect) {
                (
                    Phase::CommitInProgress(active),
                    Some(PendingEffect::Commit {
                        session_id,
                        request_id,
                        transaction_id,
                    }),
                ) => (active, session_id, request_id, transaction_id),
                _ => return Err(ErrorKind::InvalidTransition),
            };

        self.phase = Phase::CandidateValidated(active);
        self.pending_effect = None;
        let error = ServiceError {
            request_id,
            kind: ErrorKind::Storage,
        };
        self.replay = Some(Replay {
            session_id,
            request_id,
            signature: MutationSignature::Commit { transaction_id },
            result: Err(error),
        });
        Ok(error)
    }

    /// Completes a pending restartable repository reset.
    pub fn complete_factory_reset(&mut self) -> Result<Response, ErrorKind> {
        let (session_id, request_id) = match (self.phase, self.pending_effect) {
            (
                Phase::ResetInProgress,
                Some(PendingEffect::FactoryReset {
                    session_id,
                    request_id,
                }),
            ) => (session_id, request_id),
            _ => return Err(ErrorKind::InvalidTransition),
        };

        self.clear_transient();
        self.device_state = DeviceState::Unprovisioned;
        self.phase = Phase::Idle;
        self.pending_effect = None;
        let response = Response {
            request_id,
            kind: ResponseKind::FactoryReset,
        };
        self.replay = Some(Replay {
            session_id,
            request_id,
            signature: MutationSignature::FactoryReset,
            result: Ok(response),
        });
        Ok(response)
    }

    fn begin(
        &mut self,
        session: SessionContext,
        request_id: RequestId,
        transaction_id: TransactionId,
        schema: SchemaVersion,
        now_tick: u64,
    ) -> Result<Action, ServiceError> {
        if !matches!(self.phase, Phase::Idle) {
            return Err(ServiceError {
                request_id,
                kind: self.ownership_error(session.session_id(), transaction_id),
            });
        }

        self.phase = Phase::Owned(ActiveTransaction {
            session_id: session.session_id(),
            transaction_id,
            schema,
            last_activity_tick: now_tick,
        });
        let response = Response {
            request_id,
            kind: ResponseKind::TransactionBegun,
        };
        self.remember(
            session.session_id(),
            request_id,
            MutationSignature::Begin {
                transaction_id,
                schema,
            },
            Ok(response),
        );
        Ok(Action::Respond(response))
    }

    fn submit(
        &mut self,
        session: SessionContext,
        request_id: RequestId,
        transaction_id: TransactionId,
        encoded: &[u8],
        now_tick: u64,
    ) -> Result<Action, ServiceError> {
        let mut active = match self.phase {
            Phase::Owned(active) => active,
            _ => {
                return Err(ServiceError {
                    request_id,
                    kind: self.ownership_error(session.session_id(), transaction_id),
                });
            }
        };
        self.check_owner(active, session.session_id(), transaction_id, request_id)?;
        if encoded.len() > MAX_CANDIDATE {
            return Err(ServiceError {
                request_id,
                kind: ErrorKind::CapacityExceeded,
            });
        }

        self.candidate_bytes.zeroize();
        self.candidate_bytes[..encoded.len()].copy_from_slice(encoded);
        self.candidate_len = encoded.len();
        active.last_activity_tick = now_tick;
        self.phase = Phase::CandidateReceived(active);
        let response = Response {
            request_id,
            kind: ResponseKind::CandidateReceived,
        };
        self.remember(
            session.session_id(),
            request_id,
            MutationSignature::SubmitCandidate { transaction_id },
            Ok(response),
        );
        Ok(Action::Respond(response))
    }

    fn validate(
        &mut self,
        session: SessionContext,
        request_id: RequestId,
        transaction_id: TransactionId,
        now_tick: u64,
    ) -> Result<Action, ServiceError> {
        let mut active = match self.phase {
            Phase::CandidateReceived(active) => active,
            _ => {
                return Err(ServiceError {
                    request_id,
                    kind: self.ownership_error(session.session_id(), transaction_id),
                });
            }
        };
        self.check_owner(active, session.session_id(), transaction_id, request_id)?;

        let mut candidate =
            match C::decode(active.schema, &self.candidate_bytes[..self.candidate_len]) {
                Ok(candidate) => candidate,
                Err(_) => {
                    return self.remembered_error(
                        session.session_id(),
                        request_id,
                        MutationSignature::Validate { transaction_id },
                        ErrorKind::CandidateDecode,
                    );
                }
            };
        if candidate.validate_for(session.authority()).is_err() {
            candidate.zeroize();
            return self.remembered_error(
                session.session_id(),
                request_id,
                MutationSignature::Validate { transaction_id },
                ErrorKind::CandidateValidation,
            );
        }

        self.candidate_bytes.zeroize();
        self.candidate_len = 0;
        self.candidate = Some(candidate);
        active.last_activity_tick = now_tick;
        self.phase = Phase::CandidateValidated(active);
        let response = Response {
            request_id,
            kind: ResponseKind::CandidateValidated,
        };
        self.remember(
            session.session_id(),
            request_id,
            MutationSignature::Validate { transaction_id },
            Ok(response),
        );
        Ok(Action::Respond(response))
    }

    fn start_commit(
        &mut self,
        session: SessionContext,
        request_id: RequestId,
        transaction_id: TransactionId,
        now_tick: u64,
    ) -> Result<Action, ServiceError> {
        let mut active = match self.phase {
            Phase::CandidateValidated(active) => active,
            _ => {
                return Err(ServiceError {
                    request_id,
                    kind: self.ownership_error(session.session_id(), transaction_id),
                });
            }
        };
        self.check_owner(active, session.session_id(), transaction_id, request_id)?;
        active.last_activity_tick = now_tick;
        self.phase = Phase::CommitInProgress(active);
        self.pending_effect = Some(PendingEffect::Commit {
            session_id: session.session_id(),
            request_id,
            transaction_id,
        });
        self.replay = None;
        Ok(Action::CommitCandidate)
    }

    fn abort(
        &mut self,
        session: SessionContext,
        request_id: RequestId,
        transaction_id: TransactionId,
    ) -> Result<Action, ServiceError> {
        match self.phase {
            Phase::Idle => {}
            Phase::Owned(active)
            | Phase::CandidateReceived(active)
            | Phase::CandidateValidated(active) => {
                self.check_owner(active, session.session_id(), transaction_id, request_id)?;
            }
            Phase::CommitInProgress(_) | Phase::Committed(_) | Phase::ResetInProgress => {
                return Err(ServiceError {
                    request_id,
                    kind: ErrorKind::InvalidTransition,
                });
            }
        }

        self.clear_transient();
        self.phase = Phase::Idle;
        let response = Response {
            request_id,
            kind: ResponseKind::Aborted,
        };
        self.remember(
            session.session_id(),
            request_id,
            MutationSignature::Abort { transaction_id },
            Ok(response),
        );
        Ok(Action::Respond(response))
    }

    fn start_factory_reset(
        &mut self,
        session: SessionContext,
        request_id: RequestId,
    ) -> Result<Action, ServiceError> {
        if matches!(
            self.phase,
            Phase::CommitInProgress(_) | Phase::ResetInProgress
        ) {
            return Err(ServiceError {
                request_id,
                kind: ErrorKind::InvalidTransition,
            });
        }

        self.clear_transient();
        self.phase = Phase::ResetInProgress;
        self.device_state = DeviceState::ResetInProgress;
        self.pending_effect = Some(PendingEffect::FactoryReset {
            session_id: session.session_id(),
            request_id,
        });
        self.replay = None;
        Ok(Action::FactoryReset)
    }

    fn replay_for(
        &self,
        session: &SessionContext,
        request: &Request<'_>,
    ) -> Option<Result<Response, ServiceError>> {
        let replay = self.replay?;
        if replay.session_id != session.session_id() || replay.request_id != request.request_id() {
            return None;
        }

        let signature_matches = match (replay.signature, request) {
            (
                MutationSignature::Begin {
                    transaction_id: expected_transaction,
                    schema: expected_schema,
                },
                Request::Begin {
                    transaction_id,
                    schema,
                    ..
                },
            ) => expected_transaction == *transaction_id && expected_schema == *schema,
            (
                MutationSignature::SubmitCandidate {
                    transaction_id: expected_transaction,
                },
                Request::SubmitCandidate {
                    transaction_id,
                    encoded,
                    ..
                },
            ) => {
                expected_transaction == *transaction_id
                    && matches!(self.phase, Phase::CandidateReceived(_))
                    && self.candidate_len == encoded.len()
                    && self.candidate_bytes[..self.candidate_len] == **encoded
            }
            (
                MutationSignature::Validate {
                    transaction_id: expected_transaction,
                },
                Request::Validate { transaction_id, .. },
            )
            | (
                MutationSignature::Commit {
                    transaction_id: expected_transaction,
                },
                Request::Commit { transaction_id, .. },
            )
            | (
                MutationSignature::Abort {
                    transaction_id: expected_transaction,
                },
                Request::Abort { transaction_id, .. },
            ) => expected_transaction == *transaction_id,
            (MutationSignature::FactoryReset, Request::FactoryReset { .. }) => true,
            _ => false,
        };

        if signature_matches {
            Some(replay.result)
        } else {
            Some(Err(ServiceError {
                request_id: request.request_id(),
                kind: ErrorKind::RequestConflict,
            }))
        }
    }

    fn transaction_state(&self) -> TransactionState {
        match self.phase {
            Phase::Idle | Phase::ResetInProgress => TransactionState::Idle,
            Phase::Owned(active) => TransactionState::Owned {
                session_id: active.session_id,
                transaction_id: active.transaction_id,
                schema: active.schema,
            },
            Phase::CandidateReceived(active) => TransactionState::CandidateReceived {
                session_id: active.session_id,
                transaction_id: active.transaction_id,
            },
            Phase::CandidateValidated(active) => TransactionState::CandidateValidated {
                session_id: active.session_id,
                transaction_id: active.transaction_id,
            },
            Phase::CommitInProgress(active) => TransactionState::CommitInProgress {
                session_id: active.session_id,
                transaction_id: active.transaction_id,
            },
            Phase::Committed(pending_generation) => {
                TransactionState::Committed { pending_generation }
            }
        }
    }

    fn ownership_error(&self, session_id: SessionId, transaction_id: TransactionId) -> ErrorKind {
        let active = match self.phase {
            Phase::Owned(active)
            | Phase::CandidateReceived(active)
            | Phase::CandidateValidated(active)
            | Phase::CommitInProgress(active) => Some(active),
            Phase::Idle | Phase::Committed(_) | Phase::ResetInProgress => None,
        };
        match active {
            Some(active) if active.session_id != session_id => ErrorKind::TransactionBusy,
            Some(active) if active.transaction_id != transaction_id => {
                ErrorKind::TransactionMismatch
            }
            _ => ErrorKind::InvalidTransition,
        }
    }

    fn check_owner(
        &self,
        active: ActiveTransaction,
        session_id: SessionId,
        transaction_id: TransactionId,
        request_id: RequestId,
    ) -> Result<(), ServiceError> {
        if active.session_id != session_id {
            Err(ServiceError {
                request_id,
                kind: ErrorKind::TransactionBusy,
            })
        } else if active.transaction_id != transaction_id {
            Err(ServiceError {
                request_id,
                kind: ErrorKind::TransactionMismatch,
            })
        } else {
            Ok(())
        }
    }

    fn remembered_error(
        &mut self,
        session_id: SessionId,
        request_id: RequestId,
        signature: MutationSignature,
        kind: ErrorKind,
    ) -> Result<Action, ServiceError> {
        let error = ServiceError { request_id, kind };
        self.remember(session_id, request_id, signature, Err(error));
        Err(error)
    }

    fn remember(
        &mut self,
        session_id: SessionId,
        request_id: RequestId,
        signature: MutationSignature,
        result: Result<Response, ServiceError>,
    ) {
        self.replay = Some(Replay {
            session_id,
            request_id,
            signature,
            result,
        });
    }

    fn clear_transient(&mut self) {
        self.candidate_bytes.zeroize();
        self.candidate_len = 0;
        if let Some(mut candidate) = self.candidate.take() {
            candidate.zeroize();
        }
    }
}

impl<C, P, const MAX_CANDIDATE: usize> Drop for Service<C, P, MAX_CANDIDATE>
where
    C: ProvisioningCandidate,
    P: AuthorizationPolicy,
{
    fn drop(&mut self) {
        self.clear_transient();
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use embedded_sdk_config::SchemaVersion;
    use zeroize::Zeroize;

    use super::{Action, ProvisioningCandidate, Service, ServiceConfig};
    use crate::{
        Authority, AuthorizationError, AuthorizationPolicy, DeviceState, ErrorKind, Generation,
        OperationKind, Request, RequestId, ResponseKind, SessionContext, SessionId, TransactionId,
        TransactionState,
    };

    struct Candidate {
        bytes: [u8; 8],
        len: usize,
    }

    impl Zeroize for Candidate {
        fn zeroize(&mut self) {
            self.bytes.zeroize();
            self.len.zeroize();
        }
    }

    impl ProvisioningCandidate for Candidate {
        type DecodeError = ();
        type ValidationError = ();

        fn decode(_version: SchemaVersion, bytes: &[u8]) -> Result<Self, Self::DecodeError> {
            if bytes.len() > 8 || bytes.is_empty() {
                return Err(());
            }
            let mut stored = [0; 8];
            stored[..bytes.len()].copy_from_slice(bytes);
            Ok(Self {
                bytes: stored,
                len: bytes.len(),
            })
        }

        fn validate_for(&self, _authority: Authority) -> Result<(), Self::ValidationError> {
            if self.len > 0 && self.bytes[0] == 1 {
                Ok(())
            } else {
                Err(())
            }
        }
    }

    struct Policy;

    impl AuthorizationPolicy for Policy {
        fn authorize(
            &self,
            session: &SessionContext,
            operation: OperationKind,
            _device_state: DeviceState,
        ) -> Result<(), AuthorizationError> {
            if operation == OperationKind::Inspect || session.authority() == Authority::HilFixture {
                if operation != OperationKind::FactoryReset || session.has_physical_presence() {
                    return Ok(());
                }
                return Err(AuthorizationError::PhysicalPresenceRequired);
            }
            Err(AuthorizationError::InsufficientAuthority)
        }
    }

    type TestService = Service<Candidate, Policy, 8>;

    fn service() -> TestService {
        Service::new(
            Policy,
            ServiceConfig::new(10, true).unwrap(),
            DeviceState::Unprovisioned,
        )
    }

    fn session(id: u64, authority: Authority, presence: bool) -> SessionContext {
        SessionContext::authenticated(SessionId::new(id).unwrap(), authority, presence)
    }

    fn request_id(id: u32) -> RequestId {
        RequestId::new(id).unwrap()
    }

    fn transaction_id(id: u64) -> TransactionId {
        TransactionId::new(id).unwrap()
    }

    fn begin(service: &mut TestService, session: &SessionContext, request: u32, now: u64) {
        assert_eq!(
            service
                .handle(
                    session,
                    Request::Begin {
                        request_id: request_id(request),
                        transaction_id: transaction_id(5),
                        schema: SchemaVersion::new(1, 0),
                    },
                    now,
                )
                .unwrap(),
            Action::Respond(crate::Response {
                request_id: request_id(request),
                kind: ResponseKind::TransactionBegun,
            })
        );
    }

    #[test]
    fn complete_transaction_stages_then_reports_pending_generation() {
        let mut service = service();
        let fixture = session(1, Authority::HilFixture, true);
        begin(&mut service, &fixture, 1, 0);
        service
            .handle(
                &fixture,
                Request::SubmitCandidate {
                    request_id: request_id(2),
                    transaction_id: transaction_id(5),
                    encoded: &[1, 2, 3],
                },
                1,
            )
            .unwrap();
        service
            .handle(
                &fixture,
                Request::Validate {
                    request_id: request_id(3),
                    transaction_id: transaction_id(5),
                },
                2,
            )
            .unwrap();
        assert!(service.candidate_for_commit().is_none());
        assert_eq!(
            service
                .handle(
                    &fixture,
                    Request::Commit {
                        request_id: request_id(4),
                        transaction_id: transaction_id(5),
                    },
                    3,
                )
                .unwrap(),
            Action::CommitCandidate
        );
        assert_eq!(service.candidate_for_commit().unwrap().len, 3);

        let generation = Generation::new(1).unwrap();
        let response = service.complete_commit(generation).unwrap();
        assert_eq!(
            response.kind,
            ResponseKind::Committed(crate::CommitDisposition::RebootRequired {
                pending_generation: generation,
            })
        );
        assert_eq!(
            service.status().device,
            DeviceState::PendingVerification {
                previous_generation: None,
                pending_generation: generation,
                attempts: 0,
            }
        );
        assert!(service.candidate_for_commit().is_none());
    }

    #[test]
    fn only_the_owning_session_can_mutate_a_transaction() {
        let mut service = service();
        let owner = session(1, Authority::HilFixture, false);
        let other = session(2, Authority::HilFixture, false);
        begin(&mut service, &owner, 1, 0);

        let error = service
            .handle(
                &other,
                Request::SubmitCandidate {
                    request_id: request_id(2),
                    transaction_id: transaction_id(5),
                    encoded: &[1],
                },
                1,
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::TransactionBusy);

        let status = service
            .handle(
                &other,
                Request::Status {
                    request_id: request_id(3),
                },
                1,
            )
            .unwrap();
        assert!(matches!(status, Action::Respond(_)));
    }

    #[test]
    fn repeated_request_is_idempotent_but_changed_content_conflicts() {
        let mut service = service();
        let fixture = session(1, Authority::HilFixture, false);
        begin(&mut service, &fixture, 1, 0);
        let submit = Request::SubmitCandidate {
            request_id: request_id(2),
            transaction_id: transaction_id(5),
            encoded: &[1, 7],
        };
        let first = service.handle(&fixture, submit, 1).unwrap();
        let duplicate = service
            .handle(
                &fixture,
                Request::SubmitCandidate {
                    request_id: request_id(2),
                    transaction_id: transaction_id(5),
                    encoded: &[1, 7],
                },
                2,
            )
            .unwrap();
        assert_eq!(duplicate, first);

        let conflict = service
            .handle(
                &fixture,
                Request::SubmitCandidate {
                    request_id: request_id(2),
                    transaction_id: transaction_id(5),
                    encoded: &[1, 8],
                },
                3,
            )
            .unwrap_err();
        assert_eq!(conflict.kind, ErrorKind::RequestConflict);
    }

    #[test]
    fn timeout_releases_only_transient_state() {
        let mut service = service();
        let fixture = session(1, Authority::HilFixture, false);
        begin(&mut service, &fixture, 1, 4);

        service.expire(13);
        assert!(matches!(
            service.status().transaction,
            TransactionState::Owned { .. }
        ));
        service.expire(14);
        assert_eq!(service.status().transaction, TransactionState::Idle);
    }

    #[test]
    fn invalid_candidate_does_not_advance_to_validated() {
        let mut service = service();
        let fixture = session(1, Authority::HilFixture, false);
        begin(&mut service, &fixture, 1, 0);
        service
            .handle(
                &fixture,
                Request::SubmitCandidate {
                    request_id: request_id(2),
                    transaction_id: transaction_id(5),
                    encoded: &[0, 9],
                },
                1,
            )
            .unwrap();

        let error = service
            .handle(
                &fixture,
                Request::Validate {
                    request_id: request_id(3),
                    transaction_id: transaction_id(5),
                },
                2,
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::CandidateValidation);
        assert!(matches!(
            service.status().transaction,
            TransactionState::CandidateReceived { .. }
        ));
    }

    #[test]
    fn unauthorized_mutation_is_rejected_before_state_change() {
        let mut service = service();
        let owner = session(1, Authority::OwnerSetup, true);
        let error = service
            .handle(
                &owner,
                Request::Begin {
                    request_id: request_id(1),
                    transaction_id: transaction_id(5),
                    schema: SchemaVersion::new(1, 0),
                },
                0,
            )
            .unwrap_err();

        assert_eq!(error.kind, ErrorKind::Unauthorized);
        assert_eq!(service.status().transaction, TransactionState::Idle);
    }

    #[test]
    fn abort_is_safe_to_repeat() {
        let mut service = service();
        let fixture = session(1, Authority::HilFixture, false);
        begin(&mut service, &fixture, 1, 0);
        let abort = Request::Abort {
            request_id: request_id(2),
            transaction_id: transaction_id(5),
        };
        let first = service.handle(&fixture, abort, 1).unwrap();
        let second = service
            .handle(
                &fixture,
                Request::Abort {
                    request_id: request_id(2),
                    transaction_id: transaction_id(5),
                },
                2,
            )
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(service.status().transaction, TransactionState::Idle);
    }

    #[test]
    fn factory_reset_requires_policy_and_completes_explicitly() {
        let mut service = service();
        let absent = session(1, Authority::HilFixture, false);
        let error = service
            .handle(
                &absent,
                Request::FactoryReset {
                    request_id: request_id(1),
                },
                0,
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Unauthorized);

        let present = session(2, Authority::HilFixture, true);
        assert_eq!(
            service
                .handle(
                    &present,
                    Request::FactoryReset {
                        request_id: request_id(2),
                    },
                    0,
                )
                .unwrap(),
            Action::FactoryReset
        );
        assert_eq!(service.status().device, DeviceState::ResetInProgress);
        assert_eq!(
            service.complete_factory_reset().unwrap().kind,
            ResponseKind::FactoryReset
        );
        assert_eq!(service.status().device, DeviceState::Unprovisioned);
    }
}
