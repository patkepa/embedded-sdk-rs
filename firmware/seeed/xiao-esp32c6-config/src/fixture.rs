//! Host-testable coordinator for the feature-gated XIAO HIL fixture.

use embedded_sdk_provisioning::{
    Action, Authority, AuthorizationError, AuthorizationPolicy, DeviceState, ErrorKind,
    MAX_CANDIDATE_BYTES, OperationKind, Repository, Request, Service, ServiceConfig, ServiceError,
    SessionContext, SessionId, WireResponse,
};
use embedded_sdk_storage::KeyValueStore;
use zeroize::Zeroize;

use crate::{MAX_ENCODED_BYTES, XiaoConfiguration};

struct HilFixturePolicy;

impl AuthorizationPolicy for HilFixturePolicy {
    fn authorize(
        &self,
        session: &SessionContext,
        operation: OperationKind,
        device_state: DeviceState,
    ) -> Result<(), AuthorizationError> {
        if session.authority() != Authority::HilFixture {
            return Err(AuthorizationError::InsufficientAuthority);
        }
        if matches!(
            device_state,
            DeviceState::PendingVerification { .. }
                | DeviceState::RollbackRequired { .. }
                | DeviceState::ResetInProgress
                | DeviceState::RecoveryRequired { .. }
        ) && !matches!(
            operation,
            OperationKind::Inspect | OperationKind::FactoryReset
        ) {
            return Err(AuthorizationError::InvalidDeviceState);
        }
        if operation == OperationKind::FactoryReset && !session.has_physical_presence() {
            return Err(AuthorizationError::PhysicalPresenceRequired);
        }
        Ok(())
    }
}

/// Redacted result of processing one fixture request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixtureOutcome {
    response: WireResponse,
    reboot_after_response: bool,
}

impl FixtureOutcome {
    /// Returns the response to encode and send to the fixture.
    #[must_use]
    pub const fn response(self) -> WireResponse {
        self.response
    }

    /// Returns whether the firmware must reboot after flushing the response.
    #[must_use]
    pub const fn reboot_after_response(self) -> bool {
        self.reboot_after_response
    }
}

/// Single-session adapter from decoded fixture requests to durable actions.
///
/// The caller owns framing and I/O. This coordinator owns transaction state,
/// identifies every caller as the compile-time-gated HIL authority, and maps
/// commit/reset actions onto the same repository used by boot recovery.
pub struct HilFixtureProvisioner {
    service: Service<XiaoConfiguration, HilFixturePolicy, MAX_CANDIDATE_BYTES>,
    session: SessionContext,
}

impl HilFixtureProvisioner {
    /// Creates the fixture session around already recovered device state.
    #[must_use]
    pub fn new(device_state: DeviceState, transaction_timeout_ticks: u64) -> Option<Self> {
        let config = ServiceConfig::new(transaction_timeout_ticks, true)?;
        let session_id = SessionId::new(1)?;
        Some(Self {
            service: Service::new(HilFixturePolicy, config, device_state),
            session: SessionContext::authenticated(session_id, Authority::HilFixture, true),
        })
    }

    /// Handles one decoded request and completes any required durable effect.
    ///
    /// `repository_scratch` must satisfy the repository's maximum slot-record
    /// bound. `candidate_scratch` is erased before this method returns.
    pub async fn handle<S: KeyValueStore>(
        &mut self,
        request: Request<'_>,
        now_tick: u64,
        repository: &mut Repository<S>,
        repository_scratch: &mut [u8],
        candidate_scratch: &mut [u8; MAX_ENCODED_BYTES],
    ) -> FixtureOutcome {
        candidate_scratch.zeroize();
        let request_id = request.request_id();
        let action = match self.service.handle(&self.session, request, now_tick) {
            Ok(action) => action,
            Err(error) => return outcome(error, false),
        };

        match action {
            Action::Respond(response) => outcome(response, false),
            Action::CommitCandidate => {
                let result = match self.service.candidate_for_commit() {
                    Some(candidate) => candidate
                        .encode(candidate_scratch)
                        .ok()
                        .map(|len| (candidate.schema(), len)),
                    None => None,
                };
                let staged = match result {
                    Some((schema, len)) => repository
                        .stage_candidate(schema, &candidate_scratch[..len], repository_scratch)
                        .await
                        .ok(),
                    None => None,
                };
                candidate_scratch.zeroize();

                match staged {
                    Some(generation) => match self.service.complete_commit(generation) {
                        Ok(response) => outcome(response, true),
                        Err(kind) => outcome(ServiceError { request_id, kind }, true),
                    },
                    None => match self.service.fail_commit() {
                        Ok(error) => outcome(error, false),
                        Err(kind) => outcome(ServiceError { request_id, kind }, false),
                    },
                }
            }
            Action::FactoryReset => {
                let reset = async {
                    repository.begin_factory_reset().await?;
                    repository.resume_factory_reset().await?;
                    Ok::<(), embedded_sdk_provisioning::RepositoryError<S::Error>>(())
                }
                .await;
                match reset {
                    Ok(()) => match self.service.complete_factory_reset() {
                        Ok(response) => outcome(response, true),
                        Err(kind) => outcome(ServiceError { request_id, kind }, true),
                    },
                    Err(_) => outcome(
                        ServiceError {
                            request_id,
                            kind: ErrorKind::Storage,
                        },
                        true,
                    ),
                }
            }
            _ => outcome(
                ServiceError {
                    request_id,
                    kind: ErrorKind::InvalidTransition,
                },
                false,
            ),
        }
    }
}

fn outcome(response: impl Into<WireResponse>, reboot_after_response: bool) -> FixtureOutcome {
    FixtureOutcome {
        response: response.into(),
        reboot_after_response,
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use embassy_futures::block_on;
    use embedded_sdk_config::SchemaVersion;
    use embedded_sdk_provisioning::{
        CommitDisposition, DeviceState, MAX_SLOT_RECORD_BYTES, Repository, Request, RequestId,
        ResponseKind, SLOT_A_KEY, SLOT_B_KEY, STATE_KEY, TransactionId, WireResponse,
    };
    use embedded_sdk_storage::{Fetch, Key, KeyValueStore};
    use std::vec::Vec;

    use super::HilFixtureProvisioner;
    use crate::{CURRENT_SCHEMA, MAX_ENCODED_BYTES};

    const OPEN_CONFIGURATION: &[u8] = b"XCF1\0\0\x04\0\0wifi";

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestError {
        BufferTooSmall,
    }

    #[derive(Default)]
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

    fn request_id(value: u32) -> RequestId {
        RequestId::new(value).unwrap()
    }

    fn transaction_id() -> TransactionId {
        TransactionId::new(7).unwrap()
    }

    #[test]
    fn validated_candidate_is_staged_and_requests_reboot() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut repository_scratch = [0; MAX_SLOT_RECORD_BYTES];
            repository.recover(&mut repository_scratch).await.unwrap();
            let mut candidate_scratch = [0xaa; MAX_ENCODED_BYTES];
            let mut fixture =
                HilFixtureProvisioner::new(DeviceState::Unprovisioned, 1_000).unwrap();

            let begin = fixture
                .handle(
                    Request::Begin {
                        request_id: request_id(1),
                        transaction_id: transaction_id(),
                        schema: CURRENT_SCHEMA,
                    },
                    1,
                    &mut repository,
                    &mut repository_scratch,
                    &mut candidate_scratch,
                )
                .await;
            assert!(!begin.reboot_after_response());

            fixture
                .handle(
                    Request::SubmitCandidate {
                        request_id: request_id(2),
                        transaction_id: transaction_id(),
                        encoded: OPEN_CONFIGURATION,
                    },
                    2,
                    &mut repository,
                    &mut repository_scratch,
                    &mut candidate_scratch,
                )
                .await;
            fixture
                .handle(
                    Request::Validate {
                        request_id: request_id(3),
                        transaction_id: transaction_id(),
                    },
                    3,
                    &mut repository,
                    &mut repository_scratch,
                    &mut candidate_scratch,
                )
                .await;
            let committed = fixture
                .handle(
                    Request::Commit {
                        request_id: request_id(4),
                        transaction_id: transaction_id(),
                    },
                    4,
                    &mut repository,
                    &mut repository_scratch,
                    &mut candidate_scratch,
                )
                .await;

            assert!(committed.reboot_after_response());
            assert!(matches!(
                committed.response(),
                WireResponse::Success(embedded_sdk_provisioning::Response {
                    kind: ResponseKind::Committed(CommitDisposition::RebootRequired { .. }),
                    ..
                })
            ));
            assert!(matches!(
                repository.device_state(),
                DeviceState::PendingVerification { attempts: 0, .. }
            ));
            assert!(candidate_scratch.iter().all(|byte| *byte == 0));
        });
    }

    #[test]
    fn factory_reset_is_completed_and_requests_reboot() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut repository_scratch = [0; MAX_SLOT_RECORD_BYTES];
            repository.recover(&mut repository_scratch).await.unwrap();
            let mut candidate_scratch = [0xaa; MAX_ENCODED_BYTES];
            let mut fixture =
                HilFixtureProvisioner::new(DeviceState::Unprovisioned, 1_000).unwrap();

            let reset = fixture
                .handle(
                    Request::FactoryReset {
                        request_id: request_id(1),
                    },
                    1,
                    &mut repository,
                    &mut repository_scratch,
                    &mut candidate_scratch,
                )
                .await;

            assert!(reset.reboot_after_response());
            assert!(matches!(
                reset.response(),
                WireResponse::Success(embedded_sdk_provisioning::Response {
                    kind: ResponseKind::FactoryReset,
                    ..
                })
            ));
            assert_eq!(repository.device_state(), DeviceState::Unprovisioned);
            assert!(candidate_scratch.iter().all(|byte| *byte == 0));
        });
    }

    #[test]
    fn fixture_factory_reset_can_clear_recovery_state() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut repository_scratch = [0; MAX_SLOT_RECORD_BYTES];
            repository.recover(&mut repository_scratch).await.unwrap();
            repository
                .stage_candidate(CURRENT_SCHEMA, OPEN_CONFIGURATION, &mut repository_scratch)
                .await
                .unwrap();
            let mut store = repository.into_store();
            store.slot_a.as_mut().unwrap()[20] ^= 0x55;

            let mut repository = Repository::new(store);
            let state = repository.recover(&mut repository_scratch).await.unwrap();
            assert!(matches!(state, DeviceState::RecoveryRequired { .. }));
            let mut candidate_scratch = [0; MAX_ENCODED_BYTES];
            let mut fixture = HilFixtureProvisioner::new(state, 1_000).unwrap();
            let reset = fixture
                .handle(
                    Request::FactoryReset {
                        request_id: request_id(1),
                    },
                    1,
                    &mut repository,
                    &mut repository_scratch,
                    &mut candidate_scratch,
                )
                .await;

            assert!(reset.reboot_after_response());
            assert!(matches!(
                reset.response(),
                WireResponse::Success(embedded_sdk_provisioning::Response {
                    kind: ResponseKind::FactoryReset,
                    ..
                })
            ));
            assert_eq!(repository.device_state(), DeviceState::Unprovisioned);
        });
    }

    #[test]
    fn transaction_timeout_is_enforced_by_fixture_ticks() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut repository_scratch = [0; MAX_SLOT_RECORD_BYTES];
            repository.recover(&mut repository_scratch).await.unwrap();
            let mut candidate_scratch = [0; MAX_ENCODED_BYTES];
            let mut fixture = HilFixtureProvisioner::new(DeviceState::Unprovisioned, 10).unwrap();
            fixture
                .handle(
                    Request::Begin {
                        request_id: request_id(1),
                        transaction_id: transaction_id(),
                        schema: SchemaVersion::new(1, 0),
                    },
                    1,
                    &mut repository,
                    &mut repository_scratch,
                    &mut candidate_scratch,
                )
                .await;

            let status = fixture
                .handle(
                    Request::Status {
                        request_id: request_id(2),
                    },
                    11,
                    &mut repository,
                    &mut repository_scratch,
                    &mut candidate_scratch,
                )
                .await;
            assert!(matches!(
                status.response(),
                WireResponse::Success(embedded_sdk_provisioning::Response {
                    kind: ResponseKind::Status(embedded_sdk_provisioning::Status {
                        transaction: embedded_sdk_provisioning::TransactionState::Idle,
                        ..
                    }),
                    ..
                })
            ));
        });
    }
}
