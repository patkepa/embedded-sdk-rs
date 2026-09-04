#![no_std]
#![forbid(unsafe_code)]
#![doc = "Portable, allocation-free MQTT configuration and lifecycle contracts."]

use core::{fmt, str};

/// Maximum broker hostname length accepted by the SDK.
pub const MAX_HOSTNAME_LEN: usize = 253;
/// Maximum MQTT client identifier length accepted by the portable SDK.
///
/// Concrete adapters may impose a smaller limit and must reject identifiers
/// they cannot encode. The portable bound accommodates cloud identities such
/// as Azure IoT Hub device identifiers.
pub const MAX_CLIENT_ID_LEN: usize = 128;
/// Maximum topic or topic-filter length accepted by the SDK.
pub const MAX_TOPIC_LEN: usize = 256;
/// Largest MQTT packet this first SDK slice supports.
pub const MAX_PACKET_SIZE: u32 = 65_535;

/// Error returned while constructing portable MQTT configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// A required string was empty.
    Empty,
    /// A bounded string exceeded its SDK limit.
    TooLong,
    /// A broker hostname was malformed.
    InvalidHostname,
    /// A client identifier contained an MQTT-forbidden null character.
    InvalidClientId,
    /// A topic name was malformed or contained a wildcard.
    InvalidTopicName,
    /// A topic filter used MQTT wildcards incorrectly.
    InvalidTopicFilter,
    /// Port zero is not a valid broker destination.
    InvalidPort,
    /// The maximum packet size was zero or exceeded [`MAX_PACKET_SIZE`].
    InvalidMaximumPacketSize,
    /// A reconnect policy had an invalid range.
    InvalidReconnectPolicy,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "MQTT value must not be empty",
            Self::TooLong => "MQTT value exceeds its bounded capacity",
            Self::InvalidHostname => "invalid MQTT broker hostname",
            Self::InvalidClientId => "invalid MQTT client identifier",
            Self::InvalidTopicName => "invalid MQTT topic name",
            Self::InvalidTopicFilter => "invalid MQTT topic filter",
            Self::InvalidPort => "MQTT broker port must not be zero",
            Self::InvalidMaximumPacketSize => "invalid MQTT maximum packet size",
            Self::InvalidReconnectPolicy => "invalid MQTT reconnect policy",
        })
    }
}

impl core::error::Error for ConfigError {}

macro_rules! bounded_text {
    ($name:ident, $capacity:expr, $validator:expr, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Eq, Hash, PartialEq)]
        pub struct $name {
            bytes: [u8; $capacity],
            len: u16,
        }

        impl $name {
            /// Validates and copies a value into fixed-capacity storage.
            pub fn new(value: &str) -> Result<Self, ConfigError> {
                if value.is_empty() {
                    return Err(ConfigError::Empty);
                }
                if value.len() > $capacity {
                    return Err(ConfigError::TooLong);
                }
                ($validator)(value)?;
                let mut bytes = [0; $capacity];
                bytes[..value.len()].copy_from_slice(value.as_bytes());
                Ok(Self {
                    bytes,
                    len: value.len() as u16,
                })
            }

            /// Returns the validated value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
            }

            /// Returns the value length in bytes.
            #[must_use]
            pub const fn len(&self) -> usize {
                self.len as usize
            }

            /// Returns whether the value is empty.
            #[must_use]
            pub const fn is_empty(&self) -> bool {
                self.len == 0
            }
        }

        impl TryFrom<&str> for $name {
            type Error = ConfigError;
            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.as_str())
                    .finish()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

fn validate_hostname(value: &str) -> Result<(), ConfigError> {
    if value.ends_with('.')
        || value.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        Err(ConfigError::InvalidHostname)
    } else {
        Ok(())
    }
}

fn validate_client_id(value: &str) -> Result<(), ConfigError> {
    if value.contains('\0') {
        Err(ConfigError::InvalidClientId)
    } else {
        Ok(())
    }
}

fn validate_topic_name(value: &str) -> Result<(), ConfigError> {
    if value.contains(['\0', '+', '#']) {
        Err(ConfigError::InvalidTopicName)
    } else {
        Ok(())
    }
}

fn validate_topic_filter(value: &str) -> Result<(), ConfigError> {
    let levels = value.split('/');
    let level_count = levels.clone().count();
    for (index, level) in levels.enumerate() {
        if level.contains('\0')
            || (level.contains('+') && level != "+")
            || (level.contains('#') && (level != "#" || index + 1 != level_count))
        {
            return Err(ConfigError::InvalidTopicFilter);
        }
    }
    Ok(())
}

bounded_text!(
    BrokerHostname,
    MAX_HOSTNAME_LEN,
    validate_hostname,
    "A validated, bounded DNS broker hostname."
);
bounded_text!(
    ClientId,
    MAX_CLIENT_ID_LEN,
    validate_client_id,
    "A validated, bounded MQTT client identifier."
);
bounded_text!(
    TopicName,
    MAX_TOPIC_LEN,
    validate_topic_name,
    "A validated MQTT topic name without wildcards."
);
bounded_text!(
    TopicFilter,
    MAX_TOPIC_LEN,
    validate_topic_filter,
    "A validated MQTT subscription filter."
);

/// A validated non-zero TCP port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrokerPort(u16);

impl BrokerPort {
    /// Creates a broker port, rejecting zero.
    pub const fn new(value: u16) -> Result<Self, ConfigError> {
        if value == 0 {
            Err(ConfigError::InvalidPort)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the TCP port number.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// MQTT quality of service supported by the first adapter slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QoS {
    /// Best-effort delivery, at most once.
    AtMostOnce,
    /// Acknowledged delivery within a live or resumed session, at least once.
    AtLeastOnce,
}

/// Non-zero MQTT packet identifier used to correlate acknowledged operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationId(u16);

impl OperationId {
    /// Smallest valid MQTT packet identifier.
    pub const MIN: Self = Self(1);

    /// Creates an identifier, rejecting MQTT's reserved zero value.
    #[must_use]
    pub const fn new(value: u16) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    /// Returns the MQTT packet identifier.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Borrowed inbound MQTT publication valid until the next session operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InboundPublish<'a> {
    topic: &'a str,
    payload: &'a [u8],
    qos: QoS,
    retained: bool,
    acknowledgement_required: bool,
}

impl<'a> InboundPublish<'a> {
    /// Creates a publication decoded and validated by a concrete backend.
    #[must_use]
    pub const fn new(
        topic: &'a str,
        payload: &'a [u8],
        qos: QoS,
        retained: bool,
        acknowledgement_required: bool,
    ) -> Self {
        Self {
            topic,
            payload,
            qos,
            retained,
            acknowledgement_required,
        }
    }

    /// Returns the broker-provided topic name.
    #[must_use]
    pub const fn topic(self) -> &'a str {
        self.topic
    }

    /// Returns the application payload.
    #[must_use]
    pub const fn payload(self) -> &'a [u8] {
        self.payload
    }

    /// Returns the delivery quality of service.
    #[must_use]
    pub const fn qos(self) -> QoS {
        self.qos
    }

    /// Returns whether the broker marked the publication retained.
    #[must_use]
    pub const fn retained(self) -> bool {
        self.retained
    }

    /// Returns whether acceptance must be followed by an explicit PUBACK.
    #[must_use]
    pub const fn acknowledgement_required(self) -> bool {
        self.acknowledgement_required
    }
}

/// Backend-independent event produced while driving a live MQTT session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionEvent<'a> {
    /// An inbound publication awaits application handling and possibly ACK.
    Publish(InboundPublish<'a>),
    /// A QoS 1 outbound publication was acknowledged by the broker.
    Published(OperationId),
    /// A subscription was accepted at the supplied QoS.
    Subscribed {
        /// Correlates the original subscribe operation.
        operation: OperationId,
        /// QoS granted by the broker.
        granted_qos: QoS,
    },
    /// Keepalive or another internal control exchange made progress.
    Progress,
}

/// Operation retained by a backend across a transport reconnect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionPendingOperation {
    /// An outbound QoS 1 publication is being replayed until PUBACK.
    Publish(OperationId),
    /// A subscription request is being replayed until SUBACK.
    Subscribe(OperationId),
}

/// Behavioral capabilities of a concrete live MQTT session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionCapabilities(u8);

impl SessionCapabilities {
    /// The application controls when an inbound QoS 1 PUBACK is sent.
    pub const MANUAL_INBOUND_ACK: Self = Self(1 << 0);
    /// QoS 1 PUBACK is surfaced with a correlating operation identifier.
    pub const CORRELATED_PUBLISH_ACK: Self = Self(1 << 1);
    /// SUBACK is surfaced with a correlating operation identifier and QoS.
    pub const CORRELATED_SUBSCRIPTION_ACK: Self = Self(1 << 2);

    /// Creates an empty capability set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns the union of two capability sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns whether every requested capability is implemented.
    #[must_use]
    pub const fn contains(self, requested: Self) -> bool {
        self.0 & requested.0 == requested.0
    }
}

/// Narrow asynchronous contract required by cloud-provider session drivers.
///
/// Implementations retain all packet storage and backend-specific state. An
/// inbound event borrows that storage until the caller finishes processing it.
/// This contract deliberately does not model MQTT-version-specific session
/// configuration or connection establishment.
#[allow(
    async_fn_in_trait,
    reason = "embedded providers are statically dispatched and do not require dyn compatibility"
)]
pub trait MqttSession {
    /// Concrete backend failure retaining diagnostics below [`ErrorKind`].
    type Error;

    /// Reports acknowledgment behavior required by provider drivers.
    fn capabilities(&self) -> SessionCapabilities;

    /// Returns the operation replayed from the previous transport, if any.
    fn pending_operation(&self) -> Option<SessionPendingOperation>;

    /// Returns the stable category for a concrete failure.
    fn classify_error(error: &Self::Error) -> ErrorKind;

    /// Starts one subscription operation.
    async fn subscribe(
        &mut self,
        filter: &TopicFilter,
        qos: QoS,
    ) -> Result<OperationId, Self::Error>;

    /// Starts an outbound publication.
    async fn publish(
        &mut self,
        topic: &TopicName,
        payload: &[u8],
        qos: QoS,
    ) -> Result<Option<OperationId>, Self::Error>;

    /// Waits for one inbound or acknowledgment event.
    async fn poll(&mut self) -> Result<SessionEvent<'_>, Self::Error>;

    /// Acknowledges the last delivered inbound QoS 1 publication.
    async fn acknowledge_received(&mut self) -> Result<(), Self::Error>;

    /// Sends a graceful MQTT disconnect.
    async fn disconnect(&mut self) -> Result<(), Self::Error>;
}

/// MQTT wire protocol selected for a session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolVersion {
    /// MQTT version 3.1.1.
    V3_1_1,
    /// MQTT version 5.0.
    V5,
}

/// MQTT 3.1.1 session behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V311SessionConfig {
    clean_session: bool,
}

impl V311SessionConfig {
    /// Creates MQTT 3.1.1 session behavior.
    #[must_use]
    pub const fn new(clean_session: bool) -> Self {
        Self { clean_session }
    }

    /// Returns whether the broker should discard session state on disconnect.
    #[must_use]
    pub const fn clean_session(self) -> bool {
        self.clean_session
    }
}

/// MQTT 5 session behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V5SessionConfig {
    session_expiry_seconds: u32,
}

impl V5SessionConfig {
    /// Creates MQTT 5 session behavior with an explicit expiry interval.
    #[must_use]
    pub const fn new(session_expiry_seconds: u32) -> Self {
        Self {
            session_expiry_seconds,
        }
    }

    /// Returns the requested broker session expiry interval.
    #[must_use]
    pub const fn session_expiry_seconds(self) -> u32 {
        self.session_expiry_seconds
    }
}

/// Version-specific MQTT session behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionConfig {
    /// MQTT 3.1.1 session options.
    V3_1_1(V311SessionConfig),
    /// MQTT 5 session options.
    V5(V5SessionConfig),
}

impl SessionConfig {
    /// Returns the selected MQTT wire version.
    #[must_use]
    pub const fn protocol_version(self) -> ProtocolVersion {
        match self {
            Self::V3_1_1(_) => ProtocolVersion::V3_1_1,
            Self::V5(_) => ProtocolVersion::V5,
        }
    }
}

/// Validated portable MQTT session configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    hostname: BrokerHostname,
    port: BrokerPort,
    client_id: ClientId,
    keep_alive_seconds: u16,
    session: SessionConfig,
    maximum_packet_size: u32,
}

impl Config {
    /// Creates an MQTT 3.1.1 configuration with explicit resource limits.
    pub const fn new_v311(
        hostname: BrokerHostname,
        port: BrokerPort,
        client_id: ClientId,
        keep_alive_seconds: u16,
        clean_session: bool,
        maximum_packet_size: u32,
    ) -> Result<Self, ConfigError> {
        Self::new_inner(
            hostname,
            port,
            client_id,
            keep_alive_seconds,
            SessionConfig::V3_1_1(V311SessionConfig::new(clean_session)),
            maximum_packet_size,
        )
    }

    /// Creates an MQTT 5 configuration with explicit resource and session limits.
    pub const fn new_v5(
        hostname: BrokerHostname,
        port: BrokerPort,
        client_id: ClientId,
        keep_alive_seconds: u16,
        session_expiry_seconds: u32,
        maximum_packet_size: u32,
    ) -> Result<Self, ConfigError> {
        Self::new_inner(
            hostname,
            port,
            client_id,
            keep_alive_seconds,
            SessionConfig::V5(V5SessionConfig::new(session_expiry_seconds)),
            maximum_packet_size,
        )
    }

    const fn new_inner(
        hostname: BrokerHostname,
        port: BrokerPort,
        client_id: ClientId,
        keep_alive_seconds: u16,
        session: SessionConfig,
        maximum_packet_size: u32,
    ) -> Result<Self, ConfigError> {
        if maximum_packet_size == 0 || maximum_packet_size > MAX_PACKET_SIZE {
            return Err(ConfigError::InvalidMaximumPacketSize);
        }
        Ok(Self {
            hostname,
            port,
            client_id,
            keep_alive_seconds,
            session,
            maximum_packet_size,
        })
    }

    /// Returns the broker hostname.
    pub const fn hostname(&self) -> &BrokerHostname {
        &self.hostname
    }
    /// Returns the broker TCP port.
    pub const fn port(&self) -> BrokerPort {
        self.port
    }
    /// Returns the MQTT client identifier.
    pub const fn client_id(&self) -> &ClientId {
        &self.client_id
    }
    /// Returns the keepalive interval; zero disables keepalive.
    pub const fn keep_alive_seconds(&self) -> u16 {
        self.keep_alive_seconds
    }
    /// Returns the selected version-specific session behavior.
    pub const fn session(&self) -> SessionConfig {
        self.session
    }
    /// Returns the selected MQTT wire protocol.
    pub const fn protocol_version(&self) -> ProtocolVersion {
        self.session.protocol_version()
    }
    /// Returns the largest inbound MQTT packet accepted by this client.
    pub const fn maximum_packet_size(&self) -> u32 {
        self.maximum_packet_size
    }
}

/// MQTT lifecycle state, separate from link, DNS, TCP, and TLS state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConnectionState {
    /// MQTT is not configured or enabled.
    #[default]
    Disabled,
    /// The service is waiting for network readiness.
    WaitingForNetwork,
    /// The service is resolving the configured broker.
    ResolvingBroker,
    /// A transport is being established outside the protocol adapter.
    ConnectingTransport,
    /// An encrypted transport is authenticating the broker.
    AuthenticatingTransport,
    /// MQTT CONNECT is in progress.
    ConnectingSession,
    /// A fresh broker session is active.
    Connected,
    /// A previous broker session was resumed.
    Resumed,
    /// A recoverable failure is waiting for its retry deadline.
    BackingOff,
}

/// Backend-independent MQTT error category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Invalid SDK or backend configuration.
    Configuration,
    /// Caller-owned capacity or packet limits were exceeded.
    Capacity,
    /// The byte transport failed.
    Transport,
    /// The broker rejected an operation or sent invalid protocol data.
    Protocol,
    /// The broker rejected the supplied connection identity or credential.
    Authentication,
    /// The active connection was closed.
    Disconnected,
    /// An operation cannot currently make progress.
    NotReady,
}

/// Stable counters and state suitable for health telemetry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    /// Current MQTT-specific lifecycle state.
    pub state: ConnectionState,
    /// Successful fresh MQTT sessions.
    pub connections: u32,
    /// Successfully resumed broker sessions.
    pub resumptions: u32,
    /// Recoverable MQTT/transport failures.
    pub failures: u32,
    /// Successful outbound publishes.
    pub publishes: u32,
    /// Inbound publish packets delivered to the application.
    pub received: u32,
    /// Messages rejected or dropped by bounded application queues.
    pub queue_drops: u32,
    /// Most recent normalized error, if any.
    pub last_error: Option<ErrorKind>,
}

impl Snapshot {
    /// Moves to a lifecycle state without changing counters.
    pub fn transition(&mut self, state: ConnectionState) {
        self.state = state;
    }
    /// Records a fresh connection.
    pub fn record_connected(&mut self) {
        self.state = ConnectionState::Connected;
        self.connections = self.connections.saturating_add(1);
        self.last_error = None;
    }
    /// Records a resumed broker session.
    pub fn record_resumed(&mut self) {
        self.state = ConnectionState::Resumed;
        self.resumptions = self.resumptions.saturating_add(1);
        self.last_error = None;
    }
    /// Records a recoverable failure.
    pub fn record_failure(&mut self, error: ErrorKind) {
        self.state = ConnectionState::BackingOff;
        self.failures = self.failures.saturating_add(1);
        self.last_error = Some(error);
    }
    /// Records a successful publish.
    pub fn record_publish(&mut self) {
        self.publishes = self.publishes.saturating_add(1);
    }
    /// Records a delivered inbound publish.
    pub fn record_receive(&mut self) {
        self.received = self.received.saturating_add(1);
    }
    /// Records one message rejected or dropped by an application queue.
    pub fn record_queue_drop(&mut self) {
        self.queue_drops = self.queue_drops.saturating_add(1);
    }
    /// Records several messages rejected or dropped by an application queue.
    pub fn record_queue_drops(&mut self, count: u32) {
        self.queue_drops = self.queue_drops.saturating_add(count);
    }
}

/// Bounded exponential reconnect policy with caller-provided deterministic jitter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectPolicy {
    initial_delay_ms: u32,
    maximum_delay_ms: u32,
    jitter_ms: u32,
}

impl ReconnectPolicy {
    /// Validates a reconnect delay range.
    pub const fn new(
        initial_delay_ms: u32,
        maximum_delay_ms: u32,
        jitter_ms: u32,
    ) -> Result<Self, ConfigError> {
        if initial_delay_ms == 0 || maximum_delay_ms < initial_delay_ms {
            Err(ConfigError::InvalidReconnectPolicy)
        } else {
            Ok(Self {
                initial_delay_ms,
                maximum_delay_ms,
                jitter_ms,
            })
        }
    }

    /// Creates mutable reconnect state for this policy.
    #[must_use]
    pub const fn backoff(self) -> ReconnectBackoff {
        ReconnectBackoff {
            policy: self,
            attempts: 0,
        }
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay_ms: 1_000,
            maximum_delay_ms: 60_000,
            jitter_ms: 250,
        }
    }
}

/// Mutable reconnect attempt state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectBackoff {
    policy: ReconnectPolicy,
    attempts: u8,
}

impl ReconnectBackoff {
    /// Returns the number of delays produced since the last reset.
    #[must_use]
    pub const fn attempts(&self) -> u8 {
        self.attempts
    }
    /// Resets the exponential sequence after a successful session.
    pub fn reset(&mut self) {
        self.attempts = 0;
    }
    /// Produces the next bounded delay using caller-supplied jitter entropy.
    pub fn next_delay_ms(&mut self, jitter: u32) -> u32 {
        let shift = self.attempts.min(31);
        let base = self
            .policy
            .initial_delay_ms
            .checked_shl(u32::from(shift))
            .unwrap_or(u32::MAX)
            .min(self.policy.maximum_delay_ms);
        self.attempts = self.attempts.saturating_add(1);
        let jitter = if self.policy.jitter_ms == 0 {
            0
        } else {
            jitter % self.policy.jitter_ms.saturating_add(1)
        };
        base.saturating_add(jitter)
            .min(self.policy.maximum_delay_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_topics_and_filters() {
        assert!(TopicName::new("devices/a/telemetry").is_ok());
        assert_eq!(
            TopicName::new("devices/+/telemetry"),
            Err(ConfigError::InvalidTopicName)
        );
        assert!(TopicFilter::new("devices/+/commands/#").is_ok());
        assert_eq!(
            TopicFilter::new("devices/a#"),
            Err(ConfigError::InvalidTopicFilter)
        );
        assert_eq!(
            TopicFilter::new("devices/#/bad"),
            Err(ConfigError::InvalidTopicFilter)
        );
    }

    #[test]
    fn validates_host_and_bounded_values() {
        assert!(BrokerHostname::new("broker.example.test").is_ok());
        assert_eq!(
            BrokerHostname::new("-broker.test"),
            Err(ConfigError::InvalidHostname)
        );
        assert_eq!(ClientId::new(""), Err(ConfigError::Empty));
        assert_eq!(BrokerPort::new(0), Err(ConfigError::InvalidPort));
    }

    #[test]
    fn mqtt_versions_keep_their_session_semantics_separate() {
        let hostname = BrokerHostname::new("broker.example.test").unwrap();
        let port = BrokerPort::new(8883).unwrap();
        let client_id = ClientId::new("test-client").unwrap();

        let v311 = Config::new_v311(hostname, port, client_id, 60, false, 1024).unwrap();
        assert_eq!(v311.protocol_version(), ProtocolVersion::V3_1_1);
        assert_eq!(
            v311.session(),
            SessionConfig::V3_1_1(V311SessionConfig::new(false))
        );

        let v5 = Config::new_v5(hostname, port, client_id, 60, 300, 1024).unwrap();
        assert_eq!(v5.protocol_version(), ProtocolVersion::V5);
        assert_eq!(v5.session(), SessionConfig::V5(V5SessionConfig::new(300)));
    }

    #[test]
    fn portable_client_identifier_accepts_cloud_identity_length() {
        let identifier = "a".repeat(MAX_CLIENT_ID_LEN);
        assert!(ClientId::new(&identifier).is_ok());
        assert_eq!(
            ClientId::new(&(identifier + "a")),
            Err(ConfigError::TooLong)
        );
    }

    #[test]
    fn reconnect_delay_is_exponential_bounded_and_resettable() {
        let mut backoff = ReconnectPolicy::new(100, 450, 25).unwrap().backoff();
        assert_eq!(backoff.next_delay_ms(7), 107);
        assert_eq!(backoff.next_delay_ms(100), 222);
        assert_eq!(backoff.next_delay_ms(0), 400);
        assert_eq!(backoff.next_delay_ms(25), 450);
        backoff.reset();
        assert_eq!(backoff.next_delay_ms(0), 100);
    }

    #[test]
    fn lifecycle_counters_saturate_and_clear_success_error() {
        let mut snapshot = Snapshot::default();
        snapshot.record_failure(ErrorKind::Transport);
        snapshot.record_connected();
        snapshot.record_publish();
        snapshot.record_receive();
        assert_eq!(snapshot.state, ConnectionState::Connected);
        assert_eq!(snapshot.last_error, None);
        assert_eq!(
            (
                snapshot.failures,
                snapshot.connections,
                snapshot.publishes,
                snapshot.received
            ),
            (1, 1, 1, 1)
        );
    }
}
