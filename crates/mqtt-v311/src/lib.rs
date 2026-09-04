#![no_std]
#![forbid(unsafe_code)]
#![doc = "Experimental allocation-free MQTT 3.1.1 adapter over async byte streams."]

#[cfg(test)]
extern crate std;

mod codec;

use core::{convert::Infallible, fmt};

use embedded_io_async::{Read, Write};
use embedded_sdk_mqtt::{
    Config, ErrorKind, MqttSession, QoS, SessionCapabilities, SessionConfig, Snapshot, TopicFilter,
    TopicName,
};

pub use embedded_sdk_mqtt::{InboundPublish, OperationId, SessionEvent as Event};

use codec::ControlPacket;

/// Whether the supplied byte stream authenticates and encrypts broker traffic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportSecurity {
    /// A verified TLS stream.
    Encrypted,
    /// Raw TCP enabled only for a local isolated test broker.
    PlaintextFixture,
}

/// MQTT username and password borrowed only while encoding CONNECT.
#[derive(Clone, Copy)]
pub struct Credentials<'a> {
    username: &'a str,
    password: &'a [u8],
}

impl<'a> Credentials<'a> {
    /// Creates a credential pair suitable for an encrypted transport.
    pub fn new(username: &'a str, password: &'a [u8]) -> Result<Self, AdapterConfigError> {
        if username.is_empty() || username.contains('\0') || password.is_empty() {
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

/// Configuration rejected before opening an MQTT connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AdapterConfigError {
    /// This adapter accepts only MQTT 3.1.1 configuration.
    ProtocolMismatch,
    /// RX, TX, and replay buffers must all be non-empty.
    EmptyBuffer,
    /// RX storage was smaller than the advertised inbound packet limit.
    RxBufferTooSmall {
        /// Required bytes.
        required: usize,
        /// Available bytes.
        available: usize,
    },
    /// A credential field was empty or malformed.
    InvalidCredentials,
}

impl fmt::Display for AdapterConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProtocolMismatch => formatter.write_str("MQTT 3.1.1 adapter requires MQTT 3.1.1"),
            Self::EmptyBuffer => {
                formatter.write_str("MQTT RX, TX, and replay buffers must not be empty")
            }
            Self::RxBufferTooSmall {
                required,
                available,
            } => write!(
                formatter,
                "MQTT RX buffer requires {required} bytes but only {available} were supplied"
            ),
            Self::InvalidCredentials => formatter.write_str("invalid MQTT credentials"),
        }
    }
}

impl core::error::Error for AdapterConfigError {}

/// MQTT 3.1.1 CONNACK return code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectReturnCode {
    /// Connection accepted.
    Accepted,
    /// Protocol version refused.
    UnacceptableProtocolVersion,
    /// Client identifier refused.
    IdentifierRejected,
    /// Broker unavailable.
    ServerUnavailable,
    /// Username or password refused.
    BadUsernameOrPassword,
    /// Identity is not authorized.
    NotAuthorized,
}

impl ConnectReturnCode {
    const fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Accepted),
            1 => Some(Self::UnacceptableProtocolVersion),
            2 => Some(Self::IdentifierRejected),
            3 => Some(Self::ServerUnavailable),
            4 => Some(Self::BadUsernameOrPassword),
            5 => Some(Self::NotAuthorized),
            _ => None,
        }
    }
}

/// MQTT session or transport failure.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error<E> {
    /// Another retained control operation is still in flight.
    NotReady,
    /// The active stream closed or the broker sent DISCONNECT.
    Disconnected,
    /// The requested operation had inconsistent or invalid arguments.
    InvalidRequest,
    /// Caller-owned storage cannot hold a complete packet.
    Capacity,
    /// A complete inbound frame violated MQTT 3.1.1 encoding rules.
    MalformedPacket,
    /// A valid packet was unexpected in the current client state.
    UnexpectedPacket,
    /// IoT Hub's supported QoS subset was exceeded.
    UnsupportedQos,
    /// The broker refused a requested subscription.
    SubscriptionRejected,
    /// A QoS 1 inbound publish must be accepted and acknowledged first.
    AcknowledgementRequired,
    /// Credentials were supplied to an unauthenticated plaintext stream.
    CredentialsRequireEncryption,
    /// The broker rejected the supplied identity or credential.
    AuthenticationRejected(ConnectReturnCode),
    /// The broker refused CONNECT for another reason.
    ConnectionRejected(ConnectReturnCode),
    /// The established byte stream failed.
    Transport(E),
    /// The byte stream reported a successful zero-byte write.
    WriteZero,
}

impl<E> Error<E> {
    /// Returns the stable portable failure category.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::NotReady | Self::AcknowledgementRequired => ErrorKind::NotReady,
            Self::Disconnected => ErrorKind::Disconnected,
            Self::InvalidRequest | Self::CredentialsRequireEncryption => ErrorKind::Configuration,
            Self::Capacity => ErrorKind::Capacity,
            Self::AuthenticationRejected(_) => ErrorKind::Authentication,
            Self::MalformedPacket
            | Self::UnexpectedPacket
            | Self::UnsupportedQos
            | Self::SubscriptionRejected
            | Self::ConnectionRejected(_) => ErrorKind::Protocol,
            Self::Transport(_) | Self::WriteZero => ErrorKind::Transport,
        }
    }

    fn from_codec(error: Error<Infallible>) -> Self {
        match error {
            Error::NotReady => Self::NotReady,
            Error::Disconnected => Self::Disconnected,
            Error::InvalidRequest => Self::InvalidRequest,
            Error::Capacity => Self::Capacity,
            Error::MalformedPacket => Self::MalformedPacket,
            Error::UnexpectedPacket => Self::UnexpectedPacket,
            Error::UnsupportedQos => Self::UnsupportedQos,
            Error::SubscriptionRejected => Self::SubscriptionRejected,
            Error::AcknowledgementRequired => Self::AcknowledgementRequired,
            Error::CredentialsRequireEncryption => Self::CredentialsRequireEncryption,
            Error::AuthenticationRejected(code) => Self::AuthenticationRejected(code),
            Error::ConnectionRejected(code) => Self::ConnectionRejected(code),
            Error::Transport(never) => match never {},
            Error::WriteZero => Self::WriteZero,
        }
    }
}

impl<E: fmt::Debug> fmt::Display for Error<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReady => formatter.write_str("MQTT operation is already in flight"),
            Self::Disconnected => formatter.write_str("MQTT stream disconnected"),
            Self::InvalidRequest => formatter.write_str("invalid MQTT operation"),
            Self::Capacity => formatter.write_str("MQTT packet exceeds caller-owned capacity"),
            Self::MalformedPacket => formatter.write_str("malformed MQTT 3.1.1 packet"),
            Self::UnexpectedPacket => formatter.write_str("unexpected MQTT 3.1.1 packet"),
            Self::UnsupportedQos => formatter.write_str("unsupported MQTT quality of service"),
            Self::SubscriptionRejected => formatter.write_str("MQTT subscription rejected"),
            Self::AcknowledgementRequired => {
                formatter.write_str("inbound MQTT QoS 1 acknowledgment required")
            }
            Self::CredentialsRequireEncryption => {
                formatter.write_str("MQTT credentials require an authenticated encrypted transport")
            }
            Self::AuthenticationRejected(code) => {
                write!(formatter, "MQTT authentication rejected: {code:?}")
            }
            Self::ConnectionRejected(code) => {
                write!(formatter, "MQTT connection rejected: {code:?}")
            }
            Self::Transport(error) => write!(formatter, "MQTT transport error: {error:?}"),
            Self::WriteZero => formatter.write_str("MQTT transport wrote zero bytes"),
        }
    }
}

impl<E: fmt::Debug> core::error::Error for Error<E> {}

/// Broker session disposition returned by CONNACK.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectEvent {
    /// The broker created a fresh session and subscriptions must be restored.
    FreshSession,
    /// The broker resumed its stored session.
    ResumedSession,
}

/// Retained outbound operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingOperation {
    /// No QoS/control operation is awaiting acknowledgment.
    None,
    /// A QoS 1 publish awaits PUBACK.
    Publish(OperationId),
    /// A subscription awaits SUBACK.
    Subscribe(OperationId),
}

/// Caller-owned packet buffers retained across reconnects.
pub struct Buffers<'a> {
    /// One complete inbound MQTT packet.
    pub rx: &'a mut [u8],
    /// Transient outbound packet encoding.
    pub tx: &'a mut [u8],
    /// One retained QoS 1 publish or subscription.
    pub replay: &'a mut [u8],
}

/// MQTT 3.1.1 session retaining bounded operation state across transports.
pub struct Client<'buf> {
    config: Config,
    rx: &'buf mut [u8],
    tx: &'buf mut [u8],
    replay: &'buf mut [u8],
    replay_len: usize,
    pending: PendingOperation,
    pending_inbound_ack: Option<u16>,
    next_packet_id: u16,
    snapshot: Snapshot,
}

impl<'buf> Client<'buf> {
    /// Validates protocol configuration and caller-owned packet buffers.
    pub fn new(config: &Config, buffers: Buffers<'buf>) -> Result<Self, AdapterConfigError> {
        if !matches!(config.session(), SessionConfig::V3_1_1(_)) {
            return Err(AdapterConfigError::ProtocolMismatch);
        }
        if buffers.rx.is_empty() || buffers.tx.is_empty() || buffers.replay.is_empty() {
            return Err(AdapterConfigError::EmptyBuffer);
        }
        let required = config.maximum_packet_size() as usize;
        if buffers.rx.len() < required {
            return Err(AdapterConfigError::RxBufferTooSmall {
                required,
                available: buffers.rx.len(),
            });
        }
        Ok(Self {
            config: *config,
            rx: buffers.rx,
            tx: buffers.tx,
            replay: buffers.replay,
            replay_len: 0,
            pending: PendingOperation::None,
            pending_inbound_ack: None,
            next_packet_id: 1,
            snapshot: Snapshot::default(),
        })
    }

    /// Returns lifecycle state and counters without backend or credential values.
    #[must_use]
    pub const fn snapshot(&self) -> Snapshot {
        self.snapshot
    }

    /// Returns the retained operation, if any.
    #[must_use]
    pub const fn pending_operation(&self) -> PendingOperation {
        self.pending
    }

    /// Establishes MQTT 3.1.1 over an already connected byte stream.
    pub async fn connect<IO>(
        &mut self,
        mut io: IO,
        security: TransportSecurity,
        credentials: Option<Credentials<'_>>,
    ) -> Result<Connection<'_, 'buf, IO>, Error<IO::Error>>
    where
        IO: Read + Write,
    {
        if security == TransportSecurity::PlaintextFixture && credentials.is_some() {
            return Err(Error::CredentialsRequireEncryption);
        }
        let SessionConfig::V3_1_1(session) = self.config.session() else {
            return Err(Error::InvalidRequest);
        };
        if session.clean_session() {
            self.clear_pending();
        }
        self.pending_inbound_ack = None;
        self.snapshot
            .transition(embedded_sdk_mqtt::ConnectionState::ConnectingSession);

        let credential_parts = credentials.map(|value| (value.username, value.password));
        let encoded = codec::encode_connect(
            self.config.client_id().as_str(),
            self.config.keep_alive_seconds(),
            session.clean_session(),
            credential_parts,
            self.tx,
        )
        .map_err(Error::from_codec)?;
        let sent = write_frame(&mut io, &self.tx[..encoded]).await;
        self.tx[..encoded].fill(0);
        if let Err(error) = sent {
            self.snapshot.record_failure(error.kind());
            return Err(error);
        }

        let received = match read_frame(&mut io, self.rx).await {
            Ok(received) => received,
            Err(error) => {
                self.snapshot.record_failure(error.kind());
                return Err(error);
            }
        };
        let packet = match codec::decode_control_packet(&self.rx[..received]) {
            Ok(packet) => packet,
            Err(error) => {
                let error = Error::from_codec(error);
                self.snapshot.record_failure(error.kind());
                return Err(error);
            }
        };
        let ControlPacket::Connack {
            session_present,
            code,
        } = packet
        else {
            self.snapshot.record_failure(ErrorKind::Protocol);
            return Err(Error::UnexpectedPacket);
        };
        if code != ConnectReturnCode::Accepted {
            let error = match code {
                ConnectReturnCode::BadUsernameOrPassword | ConnectReturnCode::NotAuthorized => {
                    Error::AuthenticationRejected(code)
                }
                _ => Error::ConnectionRejected(code),
            };
            self.snapshot.record_failure(error.kind());
            return Err(error);
        }
        if session.clean_session() && session_present {
            self.snapshot.record_failure(ErrorKind::Protocol);
            return Err(Error::MalformedPacket);
        }
        let event = if session_present {
            self.snapshot.record_resumed();
            ConnectEvent::ResumedSession
        } else {
            self.snapshot.record_connected();
            ConnectEvent::FreshSession
        };

        if self.replay_len != 0 {
            if matches!(self.pending, PendingOperation::Publish(_)) {
                self.replay[0] |= 0x08;
            }
            if let Err(error) = write_frame(&mut io, &self.replay[..self.replay_len]).await {
                self.snapshot.record_failure(error.kind());
                return Err(error);
            }
        }

        Ok(Connection {
            client: self,
            io,
            connect_event: event,
            connected: true,
        })
    }

    fn allocate_packet_id(&mut self) -> OperationId {
        let id = self.next_packet_id;
        self.next_packet_id = self.next_packet_id.wrapping_add(1);
        if self.next_packet_id == 0 {
            self.next_packet_id = 1;
        }
        OperationId::new(id).unwrap_or(OperationId::MIN)
    }

    fn clear_pending(&mut self) {
        self.replay[..self.replay_len].fill(0);
        self.replay_len = 0;
        self.pending = PendingOperation::None;
    }
}

/// Active MQTT 3.1.1 connection.
pub struct Connection<'client, 'buf, IO> {
    client: &'client mut Client<'buf>,
    io: IO,
    connect_event: ConnectEvent,
    connected: bool,
}

impl<IO> Connection<'_, '_, IO>
where
    IO: Read + Write,
{
    /// Returns whether the broker created or resumed the session.
    #[must_use]
    pub const fn connect_event(&self) -> ConnectEvent {
        self.connect_event
    }

    /// Returns current lifecycle and delivery counters.
    #[must_use]
    pub const fn snapshot(&self) -> Snapshot {
        self.client.snapshot
    }

    /// Returns the retained operation awaiting acknowledgment.
    #[must_use]
    pub const fn pending_operation(&self) -> PendingOperation {
        self.client.pending
    }

    /// Encodes and sends a publish. QoS 1 data remains in the replay buffer
    /// until [`Event::Published`] is returned.
    pub async fn publish(
        &mut self,
        topic: &TopicName,
        payload: &[u8],
        qos: QoS,
    ) -> Result<Option<OperationId>, Error<IO::Error>> {
        self.ensure_connected()?;
        if qos == QoS::AtLeastOnce && self.client.pending != PendingOperation::None {
            return Err(Error::NotReady);
        }
        let operation = (qos == QoS::AtLeastOnce).then(|| self.client.allocate_packet_id());
        let encoded = codec::encode_publish(
            topic.as_str(),
            payload,
            qos,
            operation.map(OperationId::get),
            false,
            self.client.tx,
        )
        .map_err(Error::from_codec)?;
        if let Some(operation) = operation {
            if self.client.replay.len() < encoded {
                return Err(Error::Capacity);
            }
            self.client.replay[..encoded].copy_from_slice(&self.client.tx[..encoded]);
            self.client.replay_len = encoded;
            self.client.pending = PendingOperation::Publish(operation);
        }
        self.write_tx(encoded).await?;
        if operation.is_none() {
            self.client.snapshot.record_publish();
        }
        Ok(operation)
    }

    /// Sends one subscription and retains it until [`Event::Subscribed`].
    pub async fn subscribe(
        &mut self,
        filter: &TopicFilter,
        qos: QoS,
    ) -> Result<OperationId, Error<IO::Error>> {
        self.ensure_connected()?;
        if self.client.pending != PendingOperation::None {
            return Err(Error::NotReady);
        }
        let operation = self.client.allocate_packet_id();
        let encoded =
            codec::encode_subscribe(filter.as_str(), qos, operation.get(), self.client.tx)
                .map_err(Error::from_codec)?;
        if self.client.replay.len() < encoded {
            return Err(Error::Capacity);
        }
        self.client.replay[..encoded].copy_from_slice(&self.client.tx[..encoded]);
        self.client.replay_len = encoded;
        self.client.pending = PendingOperation::Subscribe(operation);
        self.write_tx(encoded).await?;
        Ok(operation)
    }

    /// Waits for and classifies one complete MQTT packet.
    pub async fn poll(&mut self) -> Result<Event<'_>, Error<IO::Error>> {
        self.ensure_connected()?;
        if self.client.pending_inbound_ack.is_some() {
            return Err(Error::AcknowledgementRequired);
        }
        let received = match read_frame(&mut self.io, self.client.rx).await {
            Ok(received) => received,
            Err(error) => return Err(self.fail(error)),
        };
        if self.client.rx[0] >> 4 == 3 {
            let publication = match codec::decode_publish_packet(&self.client.rx[..received]) {
                Ok(publication) => publication,
                Err(error) => {
                    self.connected = false;
                    self.client.snapshot.record_failure(ErrorKind::Protocol);
                    return Err(Error::from_codec(error));
                }
            };
            self.client.pending_inbound_ack = publication.packet_id;
            self.client.snapshot.record_receive();
            return Ok(Event::Publish(InboundPublish::new(
                publication.topic,
                publication.payload,
                publication.qos,
                publication.retained,
                publication.packet_id.is_some(),
            )));
        }
        let packet = match codec::decode_control_packet(&self.client.rx[..received]) {
            Ok(packet) => packet,
            Err(error) => return Err(self.fail(Error::from_codec(error))),
        };
        match packet {
            ControlPacket::Puback(packet_id) => {
                let PendingOperation::Publish(operation) = self.client.pending else {
                    return Err(self.fail(Error::UnexpectedPacket));
                };
                if operation.get() != packet_id {
                    return Err(self.fail(Error::UnexpectedPacket));
                }
                self.client.clear_pending();
                self.client.snapshot.record_publish();
                Ok(Event::Published(operation))
            }
            ControlPacket::Suback {
                packet_id,
                granted_qos,
            } => {
                let PendingOperation::Subscribe(operation) = self.client.pending else {
                    return Err(self.fail(Error::UnexpectedPacket));
                };
                if operation.get() != packet_id {
                    return Err(self.fail(Error::UnexpectedPacket));
                }
                self.client.clear_pending();
                Ok(Event::Subscribed {
                    operation,
                    granted_qos,
                })
            }
            ControlPacket::Pingresp => Ok(Event::Progress),
            ControlPacket::Disconnect => Err(self.fail(Error::Disconnected)),
            ControlPacket::Connack { .. } => Err(self.fail(Error::UnexpectedPacket)),
        }
    }

    /// Acknowledges the last delivered inbound QoS 1 publication.
    ///
    /// Call this only after the owning application queue has accepted the
    /// message. Until it succeeds, [`poll`](Self::poll) refuses to read another
    /// packet.
    pub async fn acknowledge_received(&mut self) -> Result<(), Error<IO::Error>> {
        self.ensure_connected()?;
        let packet_id = self
            .client
            .pending_inbound_ack
            .ok_or(Error::InvalidRequest)?;
        let encoded = codec::encode_puback(packet_id, self.client.tx).map_err(Error::from_codec)?;
        self.write_tx(encoded).await?;
        self.client.pending_inbound_ack = None;
        Ok(())
    }

    /// Sends a keepalive PINGREQ.
    pub async fn ping(&mut self) -> Result<(), Error<IO::Error>> {
        self.ensure_connected()?;
        let encoded = codec::encode_ping(self.client.tx).map_err(Error::from_codec)?;
        self.write_tx(encoded).await
    }

    /// Sends a graceful MQTT DISCONNECT.
    pub async fn disconnect(&mut self) -> Result<(), Error<IO::Error>> {
        self.ensure_connected()?;
        let encoded = codec::encode_disconnect(self.client.tx).map_err(Error::from_codec)?;
        self.write_tx(encoded).await?;
        self.connected = false;
        self.client
            .snapshot
            .transition(embedded_sdk_mqtt::ConnectionState::WaitingForNetwork);
        Ok(())
    }

    /// Releases the underlying stream without sending DISCONNECT.
    #[must_use]
    pub fn into_transport(self) -> IO {
        self.io
    }

    fn ensure_connected(&self) -> Result<(), Error<IO::Error>> {
        if self.connected {
            Ok(())
        } else {
            Err(Error::Disconnected)
        }
    }

    async fn write_tx(&mut self, length: usize) -> Result<(), Error<IO::Error>> {
        match write_frame(&mut self.io, &self.client.tx[..length]).await {
            Ok(()) => Ok(()),
            Err(error) => Err(self.fail(error)),
        }
    }

    fn fail(&mut self, error: Error<IO::Error>) -> Error<IO::Error> {
        self.connected = false;
        self.client.snapshot.record_failure(error.kind());
        error
    }
}

impl<IO> MqttSession for Connection<'_, '_, IO>
where
    IO: Read + Write,
{
    type Error = Error<IO::Error>;

    fn capabilities(&self) -> SessionCapabilities {
        SessionCapabilities::MANUAL_INBOUND_ACK
            .union(SessionCapabilities::CORRELATED_PUBLISH_ACK)
            .union(SessionCapabilities::CORRELATED_SUBSCRIPTION_ACK)
    }

    fn classify_error(error: &Self::Error) -> ErrorKind {
        error.kind()
    }

    async fn subscribe(
        &mut self,
        filter: &TopicFilter,
        qos: QoS,
    ) -> Result<OperationId, Self::Error> {
        Connection::subscribe(self, filter, qos).await
    }

    async fn publish(
        &mut self,
        topic: &TopicName,
        payload: &[u8],
        qos: QoS,
    ) -> Result<Option<OperationId>, Self::Error> {
        Connection::publish(self, topic, payload, qos).await
    }

    async fn poll(&mut self) -> Result<Event<'_>, Self::Error> {
        Connection::poll(self).await
    }

    async fn acknowledge_received(&mut self) -> Result<(), Self::Error> {
        Connection::acknowledge_received(self).await
    }

    async fn disconnect(&mut self) -> Result<(), Self::Error> {
        Connection::disconnect(self).await
    }
}

async fn write_frame<IO>(io: &mut IO, mut frame: &[u8]) -> Result<(), Error<IO::Error>>
where
    IO: Write,
{
    while !frame.is_empty() {
        match io.write(frame).await {
            Ok(0) => return Err(Error::WriteZero),
            Ok(written) if written <= frame.len() => frame = &frame[written..],
            Ok(_) => return Err(Error::WriteZero),
            Err(error) => return Err(Error::Transport(error)),
        }
    }
    io.flush().await.map_err(Error::Transport)
}

async fn read_frame<IO>(io: &mut IO, output: &mut [u8]) -> Result<usize, Error<IO::Error>>
where
    IO: Read,
{
    if output.len() < 2 {
        return Err(Error::Capacity);
    }
    read_exact(io, &mut output[..1]).await?;
    let mut multiplier = 1_usize;
    let mut remaining = 0_usize;
    let mut header_len = 1_usize;
    loop {
        if header_len > 4 {
            return Err(Error::MalformedPacket);
        }
        let next = header_len.checked_add(1).ok_or(Error::Capacity)?;
        let byte_output = output.get_mut(header_len..next).ok_or(Error::Capacity)?;
        read_exact(io, byte_output).await?;
        let byte = output[header_len];
        remaining = remaining
            .checked_add(usize::from(byte & 0x7f) * multiplier)
            .ok_or(Error::MalformedPacket)?;
        header_len += 1;
        if byte & 0x80 == 0 {
            if header_len > 2 && byte == 0 {
                return Err(Error::MalformedPacket);
            }
            break;
        }
        multiplier = multiplier.checked_mul(128).ok_or(Error::MalformedPacket)?;
    }
    let total = header_len.checked_add(remaining).ok_or(Error::Capacity)?;
    if total > output.len() {
        return Err(Error::Capacity);
    }
    read_exact(io, &mut output[header_len..total]).await?;
    Ok(total)
}

async fn read_exact<IO>(io: &mut IO, mut output: &mut [u8]) -> Result<(), Error<IO::Error>>
where
    IO: Read,
{
    while !output.is_empty() {
        match io.read(output).await {
            Ok(0) => return Err(Error::Disconnected),
            Ok(read) if read <= output.len() => output = &mut output[read..],
            Ok(_) => return Err(Error::MalformedPacket),
            Err(error) => return Err(Error::Transport(error)),
        }
    }
    Ok(())
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
    use embedded_sdk_mqtt::{BrokerHostname, BrokerPort, ClientId};

    use super::*;

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
        async fn read(&mut self, output: &mut [u8]) -> Result<usize, Self::Error> {
            let Some(byte) = self.rx.pop_front() else {
                return core::future::pending().await;
            };
            output[0] = byte;
            Ok(1)
        }
    }

    impl Write for FragmentedIo {
        async fn write(&mut self, input: &[u8]) -> Result<usize, Self::Error> {
            let length = input.len().min(2);
            self.tx.extend_from_slice(&input[..length]);
            Ok(length)
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn config(clean_session: bool) -> Config {
        Config::new_v311(
            BrokerHostname::new("broker.example.test").unwrap(),
            BrokerPort::new(1883).unwrap(),
            ClientId::new("sensor-01").unwrap(),
            60,
            clean_session,
            128,
        )
        .unwrap()
    }

    #[test]
    fn rejects_wrong_protocol_small_buffers_and_plaintext_credentials() {
        let v5 = Config::new_v5(
            BrokerHostname::new("broker.example.test").unwrap(),
            BrokerPort::new(1883).unwrap(),
            ClientId::new("sensor-01").unwrap(),
            60,
            0,
            64,
        )
        .unwrap();
        let mut rx = [0; 128];
        let mut tx = [0; 128];
        let mut replay = [0; 128];
        assert!(matches!(
            Client::new(
                &v5,
                Buffers {
                    rx: &mut rx,
                    tx: &mut tx,
                    replay: &mut replay
                }
            ),
            Err(AdapterConfigError::ProtocolMismatch)
        ));

        let mut rx = [0; 127];
        let mut tx = [0; 128];
        let mut replay = [0; 128];
        assert!(matches!(
            Client::new(
                &config(false),
                Buffers {
                    rx: &mut rx,
                    tx: &mut tx,
                    replay: &mut replay
                }
            ),
            Err(AdapterConfigError::RxBufferTooSmall { .. })
        ));

        let mut rx = [0; 128];
        let mut tx = [0; 128];
        let mut replay = [0; 128];
        let mut client = Client::new(
            &config(false),
            Buffers {
                rx: &mut rx,
                tx: &mut tx,
                replay: &mut replay,
            },
        )
        .unwrap();
        let credentials = Credentials::new("user", b"secret").unwrap();
        let result = block_on(client.connect(
            FragmentedIo::new(&[]),
            TransportSecurity::PlaintextFixture,
            Some(credentials),
        ));
        assert!(matches!(result, Err(Error::CredentialsRequireEncryption)));
        assert_eq!(format!("{credentials:?}"), "Credentials(**REDACTED**)");
    }

    #[test]
    fn connects_subscribes_publishes_and_manually_acknowledges() {
        let inbound = [
            0x20, 0x02, 0x00, 0x00, 0x90, 0x03, 0x00, 0x01, 0x01, 0x40, 0x02, 0x00, 0x02, 0x32,
            0x0d, 0x00, 0x07, b'd', b'e', b'v', b'i', b'c', b'e', b's', 0x00, 0x2a, b'{', b'}',
        ];
        let mut rx = [0; 128];
        let mut tx = [0; 256];
        let mut replay = [0; 256];
        let mut client = Client::new(
            &config(false),
            Buffers {
                rx: &mut rx,
                tx: &mut tx,
                replay: &mut replay,
            },
        )
        .unwrap();
        let credentials = Credentials::new("hub/sensor", b"sas-token").unwrap();
        let mut connection = block_on(client.connect(
            FragmentedIo::new(&inbound),
            TransportSecurity::Encrypted,
            Some(credentials),
        ))
        .unwrap();
        assert_eq!(connection.connect_event(), ConnectEvent::FreshSession);

        let subscription = block_on(
            connection.subscribe(&TopicFilter::new("devices/#").unwrap(), QoS::AtLeastOnce),
        )
        .unwrap();
        assert_eq!(subscription.get(), 1);
        assert_eq!(
            block_on(connection.poll()).unwrap(),
            Event::Subscribed {
                operation: subscription,
                granted_qos: QoS::AtLeastOnce
            }
        );

        let publication = block_on(connection.publish(
            &TopicName::new("devices/a/messages/events/").unwrap(),
            b"{}",
            QoS::AtLeastOnce,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(publication.get(), 2);
        assert_eq!(
            block_on(connection.poll()).unwrap(),
            Event::Published(publication)
        );

        let event = block_on(connection.poll()).unwrap();
        let Event::Publish(message) = event else {
            panic!("expected publication")
        };
        assert_eq!(message.topic(), "devices");
        assert_eq!(message.payload(), b"{}");
        assert!(message.acknowledgement_required());
        assert_eq!(
            block_on(connection.poll()),
            Err(Error::AcknowledgementRequired)
        );
        block_on(connection.acknowledge_received()).unwrap();
        block_on(connection.disconnect()).unwrap();
        let io = connection.into_transport();
        assert!(
            io.tx
                .windows(4)
                .any(|value| value == [0x40, 0x02, 0x00, 0x2a])
        );
        assert!(io.tx.ends_with(&[0xe0, 0x00]));
        assert_eq!(client.snapshot().publishes, 1);
        assert_eq!(client.snapshot().received, 1);
    }

    #[test]
    fn replays_unacknowledged_qos1_publish_with_duplicate_flag() {
        let mut rx = [0; 128];
        let mut tx = [0; 256];
        let mut replay = [0; 256];
        let mut client = Client::new(
            &config(false),
            Buffers {
                rx: &mut rx,
                tx: &mut tx,
                replay: &mut replay,
            },
        )
        .unwrap();

        let first = FragmentedIo::new(&[0x20, 0x02, 0x00, 0x00]);
        let mut connection =
            block_on(client.connect(first, TransportSecurity::PlaintextFixture, None)).unwrap();
        let operation = block_on(connection.publish(
            &TopicName::new("telemetry").unwrap(),
            b"one",
            QoS::AtLeastOnce,
        ))
        .unwrap()
        .unwrap();
        let _first = connection.into_transport();
        assert_eq!(
            client.pending_operation(),
            PendingOperation::Publish(operation)
        );

        let second = FragmentedIo::new(&[0x20, 0x02, 0x01, 0x00, 0x40, 0x02, 0x00, 0x01]);
        let mut connection =
            block_on(client.connect(second, TransportSecurity::PlaintextFixture, None)).unwrap();
        assert_eq!(connection.connect_event(), ConnectEvent::ResumedSession);
        assert_eq!(
            block_on(connection.poll()).unwrap(),
            Event::Published(operation)
        );
        let second = connection.into_transport();
        assert!(second.tx.contains(&0x3a));
        assert_eq!(client.pending_operation(), PendingOperation::None);
        assert_eq!(client.snapshot().resumptions, 1);
    }

    #[test]
    fn rejects_a_variable_header_that_exceeds_the_receive_buffer() {
        let mut io = FragmentedIo::new(&[0x30, 0x80, 0x01]);
        let mut output = [0; 2];
        assert_eq!(
            block_on(read_frame(&mut io, &mut output)),
            Err(Error::Capacity)
        );
    }
}
