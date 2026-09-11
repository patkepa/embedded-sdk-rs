use core::fmt;

use embedded_sdk_cloud_core::{ConnectionState, ErrorKind};
use embedded_sdk_mqtt::{ReconnectBackoff, ReconnectPolicy};

use crate::{HubClient, SasKeySlot};

/// Connection-attempt failure classified at the firmware composition layer.
///
/// The classification preserves enough context to avoid reporting DNS as TLS,
/// treating an unavailable clock as a network fault, or ignoring Azure's
/// service-provided retry delay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConnectionFailure {
    /// Link or IP configuration is no longer usable.
    NetworkUnavailable,
    /// A trusted wall-clock value is unavailable for TLS or SAS.
    TrustedTimeUnavailable,
    /// Hub DNS resolution failed.
    EndpointResolution,
    /// The ordered transport could not be connected or was lost.
    Transport,
    /// TLS negotiation or peer verification failed.
    Tls,
    /// A local credential could not be loaded or generated.
    Credential,
    /// IoT Hub rejected the presented identity or credential.
    AuthenticationRejected,
    /// MQTT framing, session setup, or synchronization failed.
    Protocol,
    /// IoT Hub rejected an operation without asking the client to throttle.
    ServiceRejected,
    /// IoT Hub supplied a retry delay which takes precedence over local policy.
    Throttled {
        /// Service-provided delay before another attempt.
        retry_after_ms: u32,
    },
}

impl ConnectionFailure {
    /// Returns the provider-independent health category.
    #[must_use]
    pub const fn kind(self) -> ErrorKind {
        match self {
            Self::NetworkUnavailable | Self::EndpointResolution | Self::Transport => {
                ErrorKind::Network
            }
            Self::TrustedTimeUnavailable => ErrorKind::Time,
            Self::Tls => ErrorKind::Tls,
            Self::Credential | Self::AuthenticationRejected => ErrorKind::Authentication,
            Self::Protocol => ErrorKind::Protocol,
            Self::ServiceRejected => ErrorKind::ServiceRejected,
            Self::Throttled { .. } => ErrorKind::Throttled,
        }
    }
}

/// Required firmware action after one connection-attempt failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RecoveryAction {
    /// Wait for link and IP configuration rather than running a retry timer.
    WaitForNetwork,
    /// Wait for an authenticated or integrity-checked time snapshot.
    WaitForTrustedTime,
    /// Immediately acquire a credential from the named alternate key slot.
    RetryWithSasSlot(SasKeySlot),
    /// Wait for a bounded local or service-directed delay.
    Backoff {
        /// Complete delay, including any local jitter.
        delay_ms: u32,
    },
}

/// Invalid recovery policy rejected before cloud I/O.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SupervisorConfigError {
    /// Online stability must be observed for a nonzero interval.
    InvalidStabilityInterval,
}

impl fmt::Display for SupervisorConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Azure IoT stability interval must be nonzero")
    }
}

impl core::error::Error for SupervisorConfigError {}

/// Bounded Azure connection recovery and SAS-slot selection state.
///
/// Firmware owns all timers and I/O. This state only chooses the next action,
/// updates redacted [`HubClient`] health, and delays resetting local backoff
/// until the connection has remained online for the configured interval.
pub struct HubRecoverySupervisor {
    backoff: ReconnectBackoff,
    active_sas_slot: SasKeySlot,
    immediate_alternate_used: bool,
    online_since_ms: Option<u64>,
    stability_interval_ms: u64,
}

impl HubRecoverySupervisor {
    /// Creates recovery state with primary SAS credentials selected.
    pub const fn new(
        reconnect: ReconnectPolicy,
        stability_interval_ms: u64,
    ) -> Result<Self, SupervisorConfigError> {
        if stability_interval_ms == 0 {
            return Err(SupervisorConfigError::InvalidStabilityInterval);
        }
        Ok(Self {
            backoff: reconnect.backoff(),
            active_sas_slot: SasKeySlot::Primary,
            immediate_alternate_used: false,
            online_since_ms: None,
            stability_interval_ms,
        })
    }

    /// Returns the key slot to use for the next SAS acquisition.
    #[must_use]
    pub const fn active_sas_slot(&self) -> SasKeySlot {
        self.active_sas_slot
    }

    /// Records the monotonic instant at which the session became online.
    ///
    /// This deliberately does not reset retry state. A short-lived CONNECT is
    /// not sufficient evidence that the outage has ended.
    pub const fn record_online(&mut self, monotonic_now_ms: u64) {
        self.online_since_ms = Some(monotonic_now_ms);
    }

    /// Resets transient recovery state after a stable online interval.
    ///
    /// Returns `true` only when this call performs the reset. A backwards
    /// monotonic input fails closed and leaves the previous state intact.
    pub fn observe_online_stability(&mut self, monotonic_now_ms: u64) -> bool {
        let Some(online_since_ms) = self.online_since_ms else {
            return false;
        };
        let Some(online_ms) = monotonic_now_ms.checked_sub(online_since_ms) else {
            return false;
        };
        if online_ms < self.stability_interval_ms {
            return false;
        }

        self.backoff.reset();
        self.immediate_alternate_used = false;
        self.online_since_ms = None;
        true
    }

    /// Records a failure and chooses the next bounded recovery action.
    ///
    /// `jitter_entropy` must come from firmware's secure hardware RNG. It is
    /// ignored when the device must wait for a prerequisite or when Azure
    /// supplied an explicit retry delay.
    pub fn recover(
        &mut self,
        hub: &mut HubClient,
        failure: ConnectionFailure,
        jitter_entropy: u32,
    ) -> RecoveryAction {
        self.online_since_ms = None;
        hub.failed(failure.kind());

        match failure {
            ConnectionFailure::NetworkUnavailable => {
                hub.transition(ConnectionState::WaitingForNetwork);
                RecoveryAction::WaitForNetwork
            }
            ConnectionFailure::TrustedTimeUnavailable => {
                hub.transition(ConnectionState::WaitingForTime);
                RecoveryAction::WaitForTrustedTime
            }
            ConnectionFailure::AuthenticationRejected => {
                self.active_sas_slot = self.active_sas_slot.alternate();
                hub.transition(ConnectionState::RefreshingCredentials);
                if self.immediate_alternate_used {
                    RecoveryAction::Backoff {
                        delay_ms: self.backoff.next_delay_ms(jitter_entropy),
                    }
                } else {
                    self.immediate_alternate_used = true;
                    RecoveryAction::RetryWithSasSlot(self.active_sas_slot)
                }
            }
            ConnectionFailure::Throttled { retry_after_ms } => RecoveryAction::Backoff {
                delay_ms: retry_after_ms.max(1),
            },
            ConnectionFailure::EndpointResolution
            | ConnectionFailure::Transport
            | ConnectionFailure::Tls
            | ConnectionFailure::Credential
            | ConnectionFailure::Protocol
            | ConnectionFailure::ServiceRejected => RecoveryAction::Backoff {
                delay_ms: self.backoff.next_delay_ms(jitter_entropy),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use embedded_sdk_cloud_core::{ConnectionState, ErrorKind};
    use embedded_sdk_mqtt::ReconnectPolicy;

    use super::*;
    use crate::{DeviceId, HubCapabilities, HubConfig, HubHostname};

    fn hub() -> HubClient {
        HubClient::new(
            HubConfig::new(
                HubHostname::new("unit.azure-devices.net").unwrap(),
                DeviceId::new("sensor-01").unwrap(),
                60,
                1_024,
            )
            .unwrap(),
            HubCapabilities::TELEMETRY,
        )
    }

    fn supervisor() -> HubRecoverySupervisor {
        HubRecoverySupervisor::new(ReconnectPolicy::new(100, 1_000, 25).unwrap(), 30_000).unwrap()
    }

    #[test]
    fn prerequisites_wait_without_consuming_local_backoff() {
        let mut hub = hub();
        let mut supervisor = supervisor();

        assert_eq!(
            supervisor.recover(&mut hub, ConnectionFailure::NetworkUnavailable, 7),
            RecoveryAction::WaitForNetwork
        );
        assert_eq!(hub.snapshot().state, ConnectionState::WaitingForNetwork);
        assert_eq!(hub.snapshot().last_error, Some(ErrorKind::Network));

        assert_eq!(
            supervisor.recover(&mut hub, ConnectionFailure::TrustedTimeUnavailable, 7),
            RecoveryAction::WaitForTrustedTime
        );
        assert_eq!(hub.snapshot().state, ConnectionState::WaitingForTime);

        assert_eq!(
            supervisor.recover(&mut hub, ConnectionFailure::Tls, 7),
            RecoveryAction::Backoff { delay_ms: 107 }
        );
    }

    #[test]
    fn authentication_rejection_tries_alternate_once_before_backoff() {
        let mut hub = hub();
        let mut supervisor = supervisor();

        assert_eq!(
            supervisor.recover(&mut hub, ConnectionFailure::AuthenticationRejected, 4),
            RecoveryAction::RetryWithSasSlot(SasKeySlot::Secondary)
        );
        assert_eq!(supervisor.active_sas_slot(), SasKeySlot::Secondary);
        assert_eq!(hub.snapshot().state, ConnectionState::RefreshingCredentials);

        assert_eq!(
            supervisor.recover(&mut hub, ConnectionFailure::AuthenticationRejected, 4),
            RecoveryAction::Backoff { delay_ms: 104 }
        );
        assert_eq!(supervisor.active_sas_slot(), SasKeySlot::Primary);
    }

    #[test]
    fn service_retry_after_takes_precedence_without_advancing_local_sequence() {
        let mut hub = hub();
        let mut supervisor = supervisor();

        assert_eq!(
            supervisor.recover(
                &mut hub,
                ConnectionFailure::Throttled {
                    retry_after_ms: 45_000,
                },
                9,
            ),
            RecoveryAction::Backoff { delay_ms: 45_000 }
        );
        assert_eq!(hub.snapshot().last_error, Some(ErrorKind::Throttled));
        assert_eq!(
            supervisor.recover(&mut hub, ConnectionFailure::Transport, 9),
            RecoveryAction::Backoff { delay_ms: 109 }
        );
    }

    #[test]
    fn backoff_resets_only_after_stable_online_operation() {
        let mut hub = hub();
        let mut supervisor = supervisor();

        assert_eq!(
            supervisor.recover(&mut hub, ConnectionFailure::Transport, 0),
            RecoveryAction::Backoff { delay_ms: 100 }
        );
        supervisor.record_online(10_000);
        assert!(!supervisor.observe_online_stability(39_999));
        assert_eq!(
            supervisor.recover(&mut hub, ConnectionFailure::Transport, 0),
            RecoveryAction::Backoff { delay_ms: 200 }
        );

        supervisor.record_online(50_000);
        assert!(!supervisor.observe_online_stability(49_999));
        assert!(supervisor.observe_online_stability(80_000));
        assert!(!supervisor.observe_online_stability(90_000));
        assert_eq!(
            supervisor.recover(&mut hub, ConnectionFailure::Transport, 0),
            RecoveryAction::Backoff { delay_ms: 100 }
        );
    }

    #[test]
    fn rejects_zero_stability_window() {
        assert!(matches!(
            HubRecoverySupervisor::new(ReconnectPolicy::default(), 0),
            Err(SupervisorConfigError::InvalidStabilityInterval)
        ));
    }
}
