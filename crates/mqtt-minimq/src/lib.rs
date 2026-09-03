#![no_std]
#![forbid(unsafe_code)]
#![doc = "MQTT 5 adapter backed by minimq over generic async byte streams."]

#[cfg(test)]
extern crate std;

use core::fmt;

use embedded_sdk_mqtt::{Config, ErrorKind, QoS, Snapshot, TopicFilter, TopicName};
use minimq::{
    Buffers, ConfigBuilder, ConnectEvent, Io, PeerError, PubError, Publication, ResourceError,
    Session, SubscriptionOptions,
};

/// Whether a composed byte stream authenticates and encrypts broker traffic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportSecurity {
    /// A verified encrypted stream, such as hostname-verified TLS.
    Encrypted,
    /// Raw TCP explicitly enabled for a local, isolated fixture.
    PlaintextFixture,
}

/// Borrowed MQTT username and password used only while building the session.
///
/// This type owns no secret storage and its debug output is always redacted.
#[derive(Clone, Copy)]
pub struct Credentials<'a> {
    username: &'a str,
    password: &'a [u8],
}

impl<'a> Credentials<'a> {
    /// Creates borrowed credentials, rejecting an empty or null-containing username.
    pub fn new(username: &'a str, password: &'a [u8]) -> Result<Self, AdapterConfigError> {
        if username.is_empty() || username.contains('\0') {
            return Err(AdapterConfigError::InvalidCredentials);
        }
        Ok(Self { username, password })
    }
}

impl fmt::Debug for Credentials<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Credentials(**REDACTED**)")
    }
}

/// Error detected before an MQTT session is created.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AdapterConfigError {
    /// RX and TX packet buffers must both be non-empty.
    EmptyBuffer,
    /// The RX buffer is smaller than the configured inbound packet limit.
    RxBufferTooSmall {
        /// Number of bytes required by the portable configuration.
        required: usize,
        /// Number of bytes supplied by the caller.
        available: usize,
    },
    /// Credentials may never be sent over the plaintext fixture transport.
    CredentialsRequireEncryption,
    /// The borrowed credential value was malformed.
    InvalidCredentials,
    /// `minimq` rejected translated configuration.
    Backend(minimq::ConfigError),
}

impl fmt::Display for AdapterConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBuffer => formatter.write_str("MQTT RX and TX buffers must not be empty"),
            Self::RxBufferTooSmall {
                required,
                available,
            } => write!(
                formatter,
                "MQTT RX buffer requires {required} bytes but only {available} were supplied"
            ),
            Self::CredentialsRequireEncryption => {
                formatter.write_str("MQTT credentials require an authenticated encrypted transport")
            }
            Self::InvalidCredentials => formatter.write_str("invalid MQTT credentials"),
            Self::Backend(error) => write!(formatter, "minimq configuration failed: {error}"),
        }
    }
}

impl core::error::Error for AdapterConfigError {}

impl From<minimq::ConfigError> for AdapterConfigError {
    fn from(value: minimq::ConfigError) -> Self {
        Self::Backend(value)
    }
}

/// MQTT operation error normalized by failure domain while retaining backend detail.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum Error<E> {
    /// The session cannot currently accept the operation.
    NotReady,
    /// The active session has disconnected.
    Disconnected,
    /// The caller supplied invalid operation arguments.
    InvalidRequest,
    /// The broker rejected an operation or sent invalid MQTT data.
    Peer(PeerError),
    /// A caller-owned buffer or local in-flight capacity was insufficient.
    Resource(ResourceError),
    /// The established byte stream failed.
    Transport(E),
    /// The byte stream reported a zero-length successful write.
    WriteZero,
    /// The payload did not fit in the bounded publish packet storage.
    PayloadTooLarge,
    /// A newer backend error has no more specific adapter mapping.
    Backend,
}

impl<E> Error<E> {
    /// Returns the stable SDK error category.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::NotReady => ErrorKind::NotReady,
            Self::Disconnected => ErrorKind::Disconnected,
            Self::InvalidRequest => ErrorKind::Configuration,
            Self::Peer(_) => ErrorKind::Protocol,
            Self::Resource(_) | Self::PayloadTooLarge => ErrorKind::Capacity,
            Self::Transport(_) | Self::WriteZero => ErrorKind::Transport,
            Self::Backend => ErrorKind::Protocol,
        }
    }
}

impl<E: fmt::Debug> fmt::Display for Error<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReady => formatter.write_str("MQTT session is not ready"),
            Self::Disconnected => formatter.write_str("MQTT session is disconnected"),
            Self::InvalidRequest => formatter.write_str("invalid MQTT operation"),
            Self::Peer(error) => write!(formatter, "MQTT peer error: {error}"),
            Self::Resource(error) => write!(formatter, "MQTT capacity error: {error}"),
            Self::Transport(error) => write!(formatter, "MQTT transport error: {error:?}"),
            Self::WriteZero => formatter.write_str("MQTT transport wrote zero bytes"),
            Self::PayloadTooLarge => formatter.write_str("MQTT payload exceeds packet capacity"),
            Self::Backend => formatter.write_str("unrecognized minimq backend error"),
        }
    }
}

impl<E: fmt::Debug> core::error::Error for Error<E> {}

impl<E> From<minimq::Error<E>> for Error<E> {
    fn from(value: minimq::Error<E>) -> Self {
        match value {
            minimq::Error::NotReady => Self::NotReady,
            minimq::Error::Disconnected => Self::Disconnected,
            minimq::Error::InvalidRequest => Self::InvalidRequest,
            minimq::Error::Peer(error) => Self::Peer(error),
            minimq::Error::Resource(error) => Self::Resource(error),
            minimq::Error::Transport(error) => Self::Transport(error),
            minimq::Error::WriteZero => Self::WriteZero,
            _ => Self::Backend,
        }
    }
}

/// One long-lived MQTT session retaining caller-owned buffers across reconnects.
pub struct Client<'buf> {
    session: Session<'buf>,
    snapshot: Snapshot,
}

impl<'buf> Client<'buf> {
    /// Translates portable SDK configuration into a `minimq` session.
    ///
    /// `rx` must be at least `config.maximum_packet_size()` bytes. The TX arena
    /// holds encoded packets and retained QoS 1/subscription replay state.
    pub fn new(
        config: &Config,
        rx: &'buf mut [u8],
        tx: &'buf mut [u8],
        security: TransportSecurity,
        credentials: Option<Credentials<'buf>>,
    ) -> Result<Self, AdapterConfigError> {
        if rx.is_empty() || tx.is_empty() {
            return Err(AdapterConfigError::EmptyBuffer);
        }
        let required = config.maximum_packet_size() as usize;
        if rx.len() < required {
            return Err(AdapterConfigError::RxBufferTooSmall {
                required,
                available: rx.len(),
            });
        }
        if security == TransportSecurity::PlaintextFixture && credentials.is_some() {
            return Err(AdapterConfigError::CredentialsRequireEncryption);
        }

        let mut builder = ConfigBuilder::new(Buffers::new(rx, tx))
            .client_id(config.client_id().as_str())?
            .keepalive_interval(config.keep_alive_seconds())
            .session_expiry_interval(config.session_expiry_seconds());
        if let Some(credentials) = credentials {
            builder = builder.auth(credentials.username, credentials.password)?;
        }
        Ok(Self {
            session: Session::new(builder),
            snapshot: Snapshot::default(),
        })
    }

    /// Returns stable lifecycle state and counters without credentials or backend values.
    #[must_use]
    pub const fn snapshot(&self) -> Snapshot {
        self.snapshot
    }

    /// Records a lifecycle transition owned by DNS, transport, or firmware composition.
    pub fn transition(&mut self, state: embedded_sdk_mqtt::ConnectionState) {
        self.snapshot.transition(state);
    }

    /// Records a recoverable failure from a layer composed outside MQTT framing.
    pub fn record_external_failure(&mut self, error: ErrorKind) {
        self.snapshot.record_failure(error);
    }

    /// Returns the actual inbound packet capacity.
    #[must_use]
    pub fn rx_capacity(&self) -> usize {
        self.session.max_rx_packet_size()
    }

    /// Returns the outbound encode and replay arena capacity.
    #[must_use]
    pub fn tx_capacity(&self) -> usize {
        self.session.max_tx_packet_size()
    }

    /// Establishes or resumes MQTT over an already-connected async byte stream.
    ///
    /// The caller should externally timeout or cancel this wait together with
    /// transport I/O. A failed connection leaves this client reusable.
    pub async fn connect<IO: Io>(
        &mut self,
        io: IO,
    ) -> Result<Connection<'_, 'buf, IO>, Error<IO::Error>> {
        self.snapshot
            .transition(embedded_sdk_mqtt::ConnectionState::ConnectingSession);
        let Client { session, snapshot } = self;
        match session.connect(io).await {
            Ok(inner) => {
                match inner.connect_event() {
                    ConnectEvent::Connected => snapshot.record_connected(),
                    ConnectEvent::Reconnected => snapshot.record_resumed(),
                }
                Ok(Connection { inner, snapshot })
            }
            Err(error) => {
                let error = Error::from(error);
                snapshot.record_failure(error.kind());
                Err(error)
            }
        }
    }
}

/// Borrowed inbound publication valid until the next connection operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InboundPublish<'a> {
    topic: &'a str,
    payload: &'a [u8],
    qos: QoS,
    retained: bool,
}

impl<'a> InboundPublish<'a> {
    /// Returns the broker-provided topic name.
    pub const fn topic(&self) -> &'a str {
        self.topic
    }
    /// Returns the broker-provided payload.
    pub const fn payload(&self) -> &'a [u8] {
        self.payload
    }
    /// Returns the delivery QoS used by the broker.
    pub const fn qos(&self) -> QoS {
        self.qos
    }
    /// Returns whether the broker marked the publication retained.
    pub const fn retained(&self) -> bool {
        self.retained
    }
}

/// Active MQTT connection. Dropping it performs an abrupt close; call
/// [`disconnect`](Self::disconnect) for a graceful MQTT DISCONNECT.
pub struct Connection<'a, 'buf, IO> {
    inner: minimq::Connection<'a, 'buf, IO>,
    snapshot: &'a mut Snapshot,
}

impl<IO: Io> Connection<'_, '_, IO> {
    /// Returns the connection event that created this handle.
    #[must_use]
    pub fn resumed(&self) -> bool {
        self.inner.connect_event() == ConnectEvent::Reconnected
    }

    /// Returns the current portable lifecycle snapshot.
    #[must_use]
    pub const fn snapshot(&self) -> Snapshot {
        *self.snapshot
    }

    /// Returns whether the connection is still live.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    /// Adds application-queue drops to the portable lifecycle counters.
    pub fn record_queue_drops(&mut self, count: u32) {
        self.snapshot.record_queue_drops(count);
    }

    /// Publishes bytes at QoS 0 or QoS 1.
    pub async fn publish(
        &mut self,
        topic: &TopicName,
        payload: &[u8],
        qos: QoS,
    ) -> Result<(), Error<IO::Error>> {
        let publication = Publication::bytes(topic.as_str(), payload).qos(to_minimq_qos(qos));
        match self.inner.publish(publication).await {
            Ok(_) => {
                self.snapshot.record_publish();
                Ok(())
            }
            Err(PubError::Session(error)) => Err(self.observe(error.into())),
            Err(PubError::Payload(())) => Err(Error::PayloadTooLarge),
        }
    }

    /// Subscribes to one validated topic filter at QoS 0 or QoS 1.
    pub async fn subscribe(
        &mut self,
        filter: &TopicFilter,
        qos: QoS,
    ) -> Result<(), Error<IO::Error>> {
        let options = SubscriptionOptions::default().maximum_qos(to_minimq_qos(qos));
        let filter = minimq::TopicFilter::new(filter.as_str()).options(options);
        self.inner
            .subscribe(&[filter], &[])
            .await
            .map(|_| ())
            .map_err(|error| self.observe(error.into()))
    }

    /// Unsubscribes from one validated topic filter.
    pub async fn unsubscribe(&mut self, filter: &TopicFilter) -> Result<(), Error<IO::Error>> {
        self.inner
            .unsubscribe(&[filter.as_str()], &[])
            .await
            .map(|_| ())
            .map_err(|error| self.observe(error.into()))
    }

    /// Cooperatively advances acknowledgements, replay, keepalive, and inbound traffic.
    ///
    /// This method does not wait for future reads. Wrap it in an external timeout
    /// if the underlying transport can stall writes or flushes.
    pub async fn drive(&mut self) -> Result<Option<InboundPublish<'_>>, Error<IO::Error>> {
        let Connection { inner, snapshot } = self;
        match inner.drive().await {
            Ok(Some(message)) => {
                let message = from_minimq_publish(message).ok_or_else(|| {
                    let error = Error::Peer(PeerError::InvalidPacket);
                    observe_error(snapshot, &error);
                    error
                })?;
                snapshot.record_receive();
                Ok(Some(message))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                let error = Error::from(error);
                observe_error(snapshot, &error);
                Err(error)
            }
        }
    }

    /// Waits for the next inbound publish while servicing all MQTT state.
    ///
    /// This wait is cancellation-safe; callers remain responsible for an
    /// external wall-clock timeout and transport-level I/O deadlines.
    pub async fn receive(&mut self) -> Result<InboundPublish<'_>, Error<IO::Error>> {
        let Connection { inner, snapshot } = self;
        match inner.recv().await {
            Ok(message) => {
                let message = from_minimq_publish(message).ok_or_else(|| {
                    let error = Error::Peer(PeerError::InvalidPacket);
                    observe_error(snapshot, &error);
                    error
                })?;
                snapshot.record_receive();
                Ok(message)
            }
            Err(error) => {
                let error = Error::from(error);
                observe_error(snapshot, &error);
                Err(error)
            }
        }
    }

    /// Sends MQTT DISCONNECT and marks the handle closed.
    pub async fn disconnect(&mut self) -> Result<(), Error<IO::Error>> {
        match self.inner.disconnect().await {
            Ok(()) => {
                self.snapshot
                    .transition(embedded_sdk_mqtt::ConnectionState::WaitingForNetwork);
                Ok(())
            }
            Err(error) => Err(self.observe(error.into())),
        }
    }

    fn observe(&mut self, error: Error<IO::Error>) -> Error<IO::Error> {
        if matches!(
            error.kind(),
            ErrorKind::Transport | ErrorKind::Protocol | ErrorKind::Disconnected
        ) {
            self.snapshot.record_failure(error.kind());
        }
        error
    }
}

fn observe_error<E>(snapshot: &mut Snapshot, error: &Error<E>) {
    if matches!(
        error.kind(),
        ErrorKind::Transport | ErrorKind::Protocol | ErrorKind::Disconnected
    ) {
        snapshot.record_failure(error.kind());
    }
}

const fn to_minimq_qos(qos: QoS) -> minimq::QoS {
    match qos {
        QoS::AtMostOnce => minimq::QoS::AtMostOnce,
        QoS::AtLeastOnce => minimq::QoS::AtLeastOnce,
    }
}

fn from_minimq_qos(qos: minimq::QoS) -> Option<QoS> {
    match qos {
        minimq::QoS::AtMostOnce => Some(QoS::AtMostOnce),
        minimq::QoS::AtLeastOnce => Some(QoS::AtLeastOnce),
        minimq::QoS::ExactlyOnce => None,
    }
}

fn from_minimq_publish(message: minimq::InboundPublish<'_>) -> Option<InboundPublish<'_>> {
    Some(InboundPublish {
        topic: message.topic(),
        payload: message.payload(),
        qos: from_minimq_qos(message.qos())?,
        retained: message.retained(),
    })
}

#[cfg(test)]
mod tests {
    use core::{future::Future, task::Poll};
    use std::{
        collections::VecDeque,
        format,
        task::{Context, Waker},
        vec::Vec,
    };

    use embedded_io_async::{ErrorKind as IoErrorKind, ErrorType, Read, Write};
    use embedded_sdk_mqtt::{
        BrokerHostname, BrokerPort, ClientId, Config, ConnectionState, ErrorKind, QoS, TopicName,
    };

    use super::{AdapterConfigError, Client, Credentials, TransportSecurity};

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    struct FragmentedIo {
        rx: VecDeque<u8>,
        tx: Vec<u8>,
    }

    impl FragmentedIo {
        fn new(rx: &[u8]) -> Self {
            Self {
                rx: rx.iter().copied().collect(),
                tx: Vec::new(),
            }
        }
    }

    impl ErrorType for FragmentedIo {
        type Error = IoErrorKind;
    }

    impl Read for FragmentedIo {
        async fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
            if buffer.is_empty() {
                return Ok(0);
            }
            match self.rx.pop_front() {
                Some(byte) => {
                    buffer[0] = byte;
                    Ok(1)
                }
                None => core::future::pending().await,
            }
        }
    }

    impl Write for FragmentedIo {
        async fn write(&mut self, buffer: &[u8]) -> Result<usize, Self::Error> {
            let len = buffer.len().min(2);
            self.tx.extend_from_slice(&buffer[..len]);
            Ok(len)
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn config(maximum_packet_size: u32) -> Config {
        Config::new(
            BrokerHostname::new("broker.example.test").unwrap(),
            BrokerPort::new(1883).unwrap(),
            ClientId::new("adapter-test").unwrap(),
            30,
            60,
            maximum_packet_size,
        )
        .unwrap()
    }

    #[test]
    fn rejects_credentials_on_plaintext_fixture() {
        let mut rx = [0; 64];
        let mut tx = [0; 128];
        let credentials = Credentials::new("user", b"secret").unwrap();
        let result = Client::new(
            &config(64),
            &mut rx,
            &mut tx,
            TransportSecurity::PlaintextFixture,
            Some(credentials),
        );
        assert!(matches!(
            result,
            Err(AdapterConfigError::CredentialsRequireEncryption)
        ));
        assert_eq!(format!("{credentials:?}"), "Credentials(**REDACTED**)");
    }

    #[test]
    fn rejects_an_rx_buffer_below_the_advertised_limit() {
        let mut rx = [0; 63];
        let mut tx = [0; 128];
        let result = Client::new(
            &config(64),
            &mut rx,
            &mut tx,
            TransportSecurity::PlaintextFixture,
            None,
        );
        assert!(matches!(
            result,
            Err(AdapterConfigError::RxBufferTooSmall {
                required: 64,
                available: 63
            })
        ));
    }

    #[test]
    fn connects_publishes_and_receives_over_fragmented_io() {
        // MQTT 5 CONNACK followed by a QoS 0 PUBLISH on topic `a` with payload `x`.
        let io = FragmentedIo::new(&[
            0x20, 0x03, 0x00, 0x00, 0x00, 0x30, 0x05, 0, 1, b'a', 0, b'x',
        ]);
        let mut rx = [0; 64];
        let mut tx = [0; 256];
        let mut client = Client::new(
            &config(64),
            &mut rx,
            &mut tx,
            TransportSecurity::PlaintextFixture,
            None,
        )
        .unwrap();

        let mut connection = block_on(client.connect(io)).unwrap();
        assert!(!connection.resumed());
        assert_eq!(connection.snapshot().state, ConnectionState::Connected);
        block_on(connection.publish(&TopicName::new("out").unwrap(), b"value", QoS::AtLeastOnce))
            .unwrap();
        let inbound = block_on(connection.receive()).unwrap();
        assert_eq!(inbound.topic(), "a");
        assert_eq!(inbound.payload(), b"x");
        assert_eq!(inbound.qos(), QoS::AtMostOnce);
    }

    #[test]
    fn normalizes_broker_rejection_and_retains_diagnostic_state() {
        // MQTT 5 CONNACK with Not Authorized (0x87).
        let io = FragmentedIo::new(&[0x20, 0x03, 0x00, 0x87, 0x00]);
        let mut rx = [0; 64];
        let mut tx = [0; 128];
        let mut client = Client::new(
            &config(64),
            &mut rx,
            &mut tx,
            TransportSecurity::PlaintextFixture,
            None,
        )
        .unwrap();

        let error = match block_on(client.connect(io)) {
            Ok(_) => panic!("broker rejection unexpectedly connected"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), ErrorKind::Protocol);
        assert_eq!(client.snapshot().state, ConnectionState::BackingOff);
        assert_eq!(client.snapshot().failures, 1);
    }

    #[test]
    fn reports_broker_session_resumption_across_transports() {
        let mut rx = [0; 64];
        let mut tx = [0; 128];
        let mut client = Client::new(
            &config(64),
            &mut rx,
            &mut tx,
            TransportSecurity::PlaintextFixture,
            None,
        )
        .unwrap();
        let first = FragmentedIo::new(&[0x20, 0x03, 0x00, 0x00, 0x00]);
        let second = FragmentedIo::new(&[0x20, 0x03, 0x01, 0x00, 0x00]);

        assert!(!block_on(client.connect(first)).unwrap().resumed());
        assert!(block_on(client.connect(second)).unwrap().resumed());
        assert_eq!(client.snapshot().resumptions, 1);
    }
}
