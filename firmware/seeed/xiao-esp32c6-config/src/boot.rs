//! Host-testable boot selection over the portable provisioning repository.

use embedded_sdk_provisioning::{
    DeviceState, Generation, RecoveryReason, RejectionReason, Repository,
};
use embedded_sdk_storage::KeyValueStore;
use zeroize::Zeroize;

use crate::{DecodeError, XiaoConfiguration};

/// Product configuration selected after durable recovery.
///
/// This type deliberately implements neither `Debug` nor `Display` because it
/// owns credential-bearing configuration.
pub enum BootConfiguration {
    /// No persistent product configuration is available.
    Unprovisioned,
    /// A verified generation should be applied normally.
    Confirmed {
        /// Selected durable generation.
        generation: Generation,
        /// Complete validated product configuration.
        configuration: XiaoConfiguration,
    },
    /// A candidate should be applied for one bounded verification attempt.
    Pending {
        /// Candidate generation under verification.
        generation: Generation,
        /// Complete validated product configuration.
        configuration: XiaoConfiguration,
    },
}

/// Redacted boot-selection failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BootConfigurationError {
    /// The key-value store could not complete recovery or a transition.
    Storage,
    /// Repository records cannot be selected safely.
    Recovery(RecoveryReason),
    /// A confirmed record uses incompatible or invalid product data.
    ProductConfiguration,
    /// An invalid pending candidate was durably rejected and requires reboot.
    PendingRejected,
}

/// Recovers repository state and selects the product configuration for boot.
///
/// Reset and rollback markers are completed before selection. A pending
/// attempt count is durably incremented before its payload is returned. The
/// caller-owned record buffer is zeroized on every return path.
pub async fn recover_boot_configuration<S: KeyValueStore>(
    repository: &mut Repository<S>,
    scratch: &mut [u8],
    max_verification_attempts: u8,
) -> Result<BootConfiguration, BootConfigurationError> {
    let result =
        recover_boot_configuration_inner(repository, scratch, max_verification_attempts).await;
    scratch.zeroize();
    result
}

async fn recover_boot_configuration_inner<S: KeyValueStore>(
    repository: &mut Repository<S>,
    scratch: &mut [u8],
    max_verification_attempts: u8,
) -> Result<BootConfiguration, BootConfigurationError> {
    let mut state = repository
        .recover(scratch)
        .await
        .map_err(|_| BootConfigurationError::Storage)?;
    loop {
        match state {
            DeviceState::Unprovisioned => return Ok(BootConfiguration::Unprovisioned),
            DeviceState::Provisioned {
                confirmed_generation,
            } => {
                let stored = repository
                    .load_confirmed(scratch)
                    .await
                    .map_err(|_| BootConfigurationError::Storage)?;
                let configuration = XiaoConfiguration::decode(stored.schema, stored.payload)
                    .map_err(|_| BootConfigurationError::ProductConfiguration)?;
                configuration
                    .validate()
                    .map_err(|_| BootConfigurationError::ProductConfiguration)?;
                return Ok(BootConfiguration::Confirmed {
                    generation: confirmed_generation,
                    configuration,
                });
            }
            DeviceState::PendingVerification {
                pending_generation, ..
            } => {
                state = repository
                    .start_verification_attempt(max_verification_attempts)
                    .await
                    .map_err(|_| BootConfigurationError::Storage)?;
                if !matches!(state, DeviceState::PendingVerification { .. }) {
                    continue;
                }
                let stored = repository
                    .load_pending(scratch)
                    .await
                    .map_err(|_| BootConfigurationError::Storage)?;
                let configuration = match XiaoConfiguration::decode(stored.schema, stored.payload) {
                    Ok(configuration) if configuration.validate().is_ok() => configuration,
                    Err(DecodeError::UnsupportedSchema) => {
                        return Err(BootConfigurationError::ProductConfiguration);
                    }
                    Ok(_) | Err(_) => {
                        repository
                            .reject_pending(RejectionReason::ApplyFailed)
                            .await
                            .map_err(|_| BootConfigurationError::Storage)?;
                        return Err(BootConfigurationError::PendingRejected);
                    }
                };
                return Ok(BootConfiguration::Pending {
                    generation: pending_generation,
                    configuration,
                });
            }
            DeviceState::RollbackRequired { .. } => {
                state = repository
                    .complete_rollback()
                    .await
                    .map_err(|_| BootConfigurationError::Storage)?;
            }
            DeviceState::ResetInProgress => {
                state = repository
                    .resume_factory_reset()
                    .await
                    .map_err(|_| BootConfigurationError::Storage)?;
            }
            DeviceState::RecoveryRequired { reason } => {
                return Err(BootConfigurationError::Recovery(reason));
            }
            _ => {
                return Err(BootConfigurationError::Recovery(
                    RecoveryReason::CorruptedRecord,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use embassy_futures::block_on;
    use embedded_sdk_provisioning::{
        DeviceState, MAX_SLOT_RECORD_BYTES, Repository, SLOT_A_KEY, SLOT_B_KEY, STATE_KEY,
    };
    use embedded_sdk_storage::{Fetch, Key, KeyValueStore};
    use std::vec::Vec;

    use super::{BootConfiguration, BootConfigurationError, recover_boot_configuration};
    use crate::CURRENT_SCHEMA;

    const OPEN_CONFIGURATION: &[u8] = b"XCF1\0\0\x04\0\0wifi";

    fn expect_boot(result: Result<BootConfiguration, BootConfigurationError>) -> BootConfiguration {
        match result {
            Ok(configuration) => configuration,
            Err(error) => panic!("boot selection failed: {error:?}"),
        }
    }

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

    #[test]
    fn blank_repository_selects_unprovisioned() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut scratch = [0xaa; MAX_SLOT_RECORD_BYTES];
            assert!(matches!(
                expect_boot(recover_boot_configuration(&mut repository, &mut scratch, 1).await),
                BootConfiguration::Unprovisioned
            ));
            assert!(scratch.iter().all(|byte| *byte == 0));
        });
    }

    #[test]
    fn pending_then_confirmed_configuration_is_selected() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut scratch = [0; MAX_SLOT_RECORD_BYTES];
            repository.recover(&mut scratch).await.unwrap();
            let generation = repository
                .stage_candidate(CURRENT_SCHEMA, OPEN_CONFIGURATION, &mut scratch)
                .await
                .unwrap();

            match expect_boot(recover_boot_configuration(&mut repository, &mut scratch, 1).await) {
                BootConfiguration::Pending {
                    generation: actual,
                    configuration,
                } => {
                    assert_eq!(actual, generation);
                    assert_eq!(
                        configuration.station_config().unwrap().ssid().as_bytes(),
                        b"wifi"
                    );
                }
                _ => panic!("pending configuration was not selected"),
            }
            repository.confirm_pending().await.unwrap();
            assert!(matches!(
                expect_boot(recover_boot_configuration(&mut repository, &mut scratch, 1).await),
                BootConfiguration::Confirmed {
                    generation: actual,
                    ..
                } if actual == generation
            ));
        });
    }

    #[test]
    fn exhausted_pending_attempt_rolls_back_before_selection() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut scratch = [0; MAX_SLOT_RECORD_BYTES];
            repository.recover(&mut scratch).await.unwrap();
            repository
                .stage_candidate(CURRENT_SCHEMA, OPEN_CONFIGURATION, &mut scratch)
                .await
                .unwrap();
            repository.start_verification_attempt(1).await.unwrap();

            assert!(matches!(
                expect_boot(recover_boot_configuration(&mut repository, &mut scratch, 1).await),
                BootConfiguration::Unprovisioned
            ));
            assert_eq!(repository.device_state(), DeviceState::Unprovisioned);
        });
    }

    #[test]
    fn malformed_pending_configuration_is_durably_rejected() {
        block_on(async {
            let mut repository = Repository::new(MemoryStore::default());
            let mut scratch = [0; MAX_SLOT_RECORD_BYTES];
            repository.recover(&mut scratch).await.unwrap();
            repository
                .stage_candidate(CURRENT_SCHEMA, b"malformed", &mut scratch)
                .await
                .unwrap();

            assert_eq!(
                recover_boot_configuration(&mut repository, &mut scratch, 1)
                    .await
                    .err(),
                Some(BootConfigurationError::PendingRejected)
            );
            assert!(matches!(
                repository.device_state(),
                DeviceState::RollbackRequired { .. }
            ));
        });
    }
}
