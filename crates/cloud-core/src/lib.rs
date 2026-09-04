#![no_std]
#![forbid(unsafe_code)]
#![doc = "Provider-independent, allocation-free cloud lifecycle contracts."]

/// Cloud features implemented by a provider client and its composed backend.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Capabilities(u16);

impl Capabilities {
    /// Device-to-cloud telemetry publishing.
    pub const TELEMETRY: Self = Self(1 << 0);
    /// Cloud-to-device message delivery.
    pub const CLOUD_TO_DEVICE: Self = Self(1 << 1);
    /// Request-response command delivery.
    pub const COMMANDS: Self = Self(1 << 2);
    /// Provider-managed desired and reported state synchronization.
    pub const STATE_SYNCHRONIZATION: Self = Self(1 << 3);
    /// Zero-touch or fleet provisioning.
    pub const PROVISIONING: Self = Self(1 << 4);
    /// Power-loss-safe outbound message retention.
    pub const DURABLE_OUTBOX: Self = Self(1 << 5);

    /// Creates an empty capability set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns whether all requested capabilities are present.
    #[must_use]
    pub const fn contains(self, requested: Self) -> bool {
        self.0 & requested.0 == requested.0
    }

    /// Returns the union of two capability sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Lifecycle state of a cloud connection independently of its provider.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConnectionState {
    /// Cloud connectivity is not configured or enabled.
    #[default]
    Disabled,
    /// The service is waiting for usable IP connectivity.
    WaitingForNetwork,
    /// The service cannot authenticate until wall-clock time is trusted.
    WaitingForTime,
    /// The device is obtaining or refreshing a cloud assignment.
    Provisioning,
    /// The service is resolving its provider endpoint.
    ResolvingEndpoint,
    /// A TCP or equivalent ordered transport is being established.
    ConnectingTransport,
    /// The encrypted transport and peer identity are being authenticated.
    AuthenticatingTransport,
    /// The provider's application protocol session is being established.
    ConnectingProtocol,
    /// Provider-managed state is being reconciled after connection.
    Synchronizing,
    /// The cloud session is ready for its enabled operations.
    Online,
    /// An expiring credential is being replaced.
    RefreshingCredentials,
    /// A recoverable failure is waiting for its retry deadline.
    BackingOff,
    /// The service cannot operate without external intervention.
    Failed,
}

/// Provider-independent cloud failure category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Invalid local configuration.
    Configuration,
    /// A caller-owned buffer, queue, or in-flight table was exhausted.
    Capacity,
    /// Link, IP, DNS, or ordered transport failure.
    Network,
    /// Trusted time was unavailable or invalid.
    Time,
    /// Encrypted transport negotiation or peer verification failed.
    Tls,
    /// A credential could not be produced or was rejected.
    Authentication,
    /// MQTT or another application protocol failed.
    Protocol,
    /// The cloud provider rejected an otherwise valid operation.
    ServiceRejected,
    /// The provider requested that the operation be rate-limited.
    Throttled,
    /// Persistent assignment, credential, or outbox storage failed.
    Storage,
    /// The product application rejected or failed an operation.
    Application,
}

/// Stable cloud health and activity counters suitable for telemetry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    /// Current provider-independent lifecycle state.
    pub state: ConnectionState,
    /// Successful application-protocol connections.
    pub connections: u32,
    /// Recoverable failures.
    pub failures: u32,
    /// Successfully acknowledged outbound messages.
    pub outbound_acknowledged: u32,
    /// Inbound messages accepted by the application boundary.
    pub inbound_accepted: u32,
    /// Messages rejected or dropped at a bounded queue.
    pub queue_drops: u32,
    /// Most recently observed normalized failure.
    pub last_error: Option<ErrorKind>,
}

impl Snapshot {
    /// Changes lifecycle state without changing counters.
    pub const fn transition(&mut self, state: ConnectionState) {
        self.state = state;
    }

    /// Records a successful connection.
    pub const fn record_connected(&mut self) {
        self.state = ConnectionState::Online;
        self.connections = self.connections.saturating_add(1);
        self.last_error = None;
    }

    /// Records a recoverable failure and enters backoff.
    pub const fn record_failure(&mut self, error: ErrorKind) {
        self.state = ConnectionState::BackingOff;
        self.failures = self.failures.saturating_add(1);
        self.last_error = Some(error);
    }

    /// Records an acknowledged outbound message.
    pub const fn record_outbound_acknowledged(&mut self) {
        self.outbound_acknowledged = self.outbound_acknowledged.saturating_add(1);
    }

    /// Records an inbound message accepted by the application boundary.
    pub const fn record_inbound_accepted(&mut self) {
        self.inbound_accepted = self.inbound_accepted.saturating_add(1);
    }

    /// Records a message dropped or rejected at a bounded queue.
    pub const fn record_queue_drop(&mut self) {
        self.queue_drops = self.queue_drops.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{Capabilities, ConnectionState, ErrorKind, Snapshot};

    #[test]
    fn capabilities_are_additive_and_independent() {
        let supported = Capabilities::TELEMETRY.union(Capabilities::COMMANDS);

        assert!(supported.contains(Capabilities::TELEMETRY));
        assert!(supported.contains(Capabilities::COMMANDS));
        assert!(!supported.contains(Capabilities::STATE_SYNCHRONIZATION));
        assert!(!supported.contains(Capabilities::DURABLE_OUTBOX));
    }

    #[test]
    fn snapshot_preserves_failure_domain_and_saturates_counters() {
        let mut snapshot = Snapshot {
            failures: u32::MAX,
            ..Snapshot::default()
        };

        snapshot.transition(ConnectionState::AuthenticatingTransport);
        snapshot.record_failure(ErrorKind::Tls);

        assert_eq!(snapshot.state, ConnectionState::BackingOff);
        assert_eq!(snapshot.failures, u32::MAX);
        assert_eq!(snapshot.last_error, Some(ErrorKind::Tls));
    }
}
