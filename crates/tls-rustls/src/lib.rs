#![no_std]
#![forbid(unsafe_code)]
#![doc = "Experimental TLS 1.2 client adapter over async embedded I/O streams."]

extern crate alloc;

use alloc::{sync::Arc, vec};
use core::{cmp, fmt, time::Duration};

use embedded_io_async::{ErrorType, Read, Write};
use embedded_sdk_security::{TimeError, TrustedTime, UnixTime};
use rustls::{
    ClientConfig, RootCertStore,
    client::UnbufferedClientConnection,
    pki_types::{CertificateDer, ServerName},
    time_provider::TimeProvider,
    unbuffered::{ConnectionState, EncodeError, EncryptError},
    version::TLS12,
};

/// Smallest supported plaintext fragment limit.
pub const MIN_PLAINTEXT_FRAGMENT: usize = 32;
/// Largest TLS plaintext fragment allowed by the protocol.
pub const MAX_PLAINTEXT_FRAGMENT: usize = 16_384;
/// Conservative TLS 1.2 record overhead reserved beyond plaintext.
pub const TLS12_RECORD_OVERHEAD: usize = 64;

/// TLS client configuration failure detected before network I/O.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConfigError {
    /// No trust anchors were supplied.
    EmptyRootStore,
    /// A supplied DER certificate was not a valid trust anchor.
    InvalidRootCertificate(rustls::Error),
    /// A supplied PEM document was malformed or exceeded decode scratch.
    InvalidPemRoot(pem_rfc7468::Error),
    /// A PEM trust anchor did not use the `CERTIFICATE` label.
    UnexpectedPemLabel,
    /// The plaintext fragment policy was outside the supported range.
    InvalidFragmentSize,
    /// The selected provider and TLS 1.2 policy were incompatible.
    InvalidCryptoPolicy(rustls::Error),
    /// Trusted wall-clock time was unavailable for certificate validation.
    UntrustedTime(TimeError),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRootStore => formatter.write_str("TLS root store must not be empty"),
            Self::InvalidRootCertificate(_) => formatter.write_str("invalid TLS root certificate"),
            Self::InvalidPemRoot(_) => formatter.write_str("invalid PEM TLS root certificate"),
            Self::UnexpectedPemLabel => {
                formatter.write_str("TLS root PEM must use the CERTIFICATE label")
            }
            Self::InvalidFragmentSize => formatter.write_str("invalid TLS fragment size"),
            Self::InvalidCryptoPolicy(_) => formatter.write_str("invalid TLS crypto policy"),
            Self::UntrustedTime(_) => {
                formatter.write_str("trusted time unavailable for TLS verification")
            }
        }
    }
}

impl core::error::Error for ConfigError {}

/// Parsed TLS trust anchors owned independently from connection policy.
///
/// Firmware can validate and retain a versioned root bundle before trusted
/// wall-clock time becomes available. Building a [`TlsClientConfig`] still
/// fails closed unless its separate [`TrustedTime`] provider succeeds.
#[derive(Clone)]
pub struct TlsRootStore {
    inner: RootCertStore,
}

impl TlsRootStore {
    /// Parses DER-encoded root certificates into an owned trust store.
    pub fn from_der_roots<'a>(
        roots: impl IntoIterator<Item = &'a [u8]>,
    ) -> Result<Self, ConfigError> {
        let mut inner = RootCertStore::empty();
        for root in roots {
            inner
                .add(CertificateDer::from(root))
                .map_err(ConfigError::InvalidRootCertificate)?;
        }
        Self::from_root_store(inner)
    }

    /// Decodes RFC 7468 `CERTIFICATE` roots using reusable caller scratch.
    ///
    /// Scratch must hold the largest decoded certificate in the input bundle.
    /// It is reused for every root and is not retained by the returned store.
    pub fn from_pem_roots<'a>(
        roots: impl IntoIterator<Item = &'a [u8]>,
        decode_scratch: &mut [u8],
    ) -> Result<Self, ConfigError> {
        let mut inner = RootCertStore::empty();
        for root in roots {
            let (label, der) =
                pem_rfc7468::decode(root, decode_scratch).map_err(ConfigError::InvalidPemRoot)?;
            if label != "CERTIFICATE" {
                return Err(ConfigError::UnexpectedPemLabel);
            }
            inner
                .add(CertificateDer::from(der))
                .map_err(ConfigError::InvalidRootCertificate)?;
        }
        Self::from_root_store(inner)
    }

    /// Returns the number of independently trusted roots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns whether the trust set contains no roots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn from_root_store(inner: RootCertStore) -> Result<Self, ConfigError> {
        if inner.is_empty() {
            return Err(ConfigError::EmptyRootStore);
        }
        Ok(Self { inner })
    }
}

#[derive(Debug)]
struct SnapshotTime(UnixTime);

impl TimeProvider for SnapshotTime {
    fn current_time(&self) -> Option<rustls::pki_types::UnixTime> {
        Some(rustls::pki_types::UnixTime::since_unix_epoch(
            Duration::from_secs(self.0.as_seconds()),
        ))
    }
}

/// Server-authenticated TLS 1.2 policy shared by client connections.
///
/// This type owns heap-backed rustls configuration. Record buffers remain
/// caller-owned, but rustls and the RustCrypto provider require a global
/// allocator for handshake, certificate, and cryptographic state.
#[derive(Clone)]
pub struct TlsClientConfig {
    inner: Arc<ClientConfig>,
    max_plaintext_fragment: usize,
}

impl TlsClientConfig {
    /// Builds a TLS 1.2-only configuration from DER-encoded root certificates.
    ///
    /// The trusted-time snapshot is used for certificate validity checks.
    /// Callers must construct a fresh configuration after materially advancing
    /// or replacing their trusted wall-clock value.
    pub fn from_der_roots<'a>(
        roots: impl IntoIterator<Item = &'a [u8]>,
        trusted_time: &impl TrustedTime,
        max_plaintext_fragment: usize,
    ) -> Result<Self, ConfigError> {
        Self::from_trust_roots(
            TlsRootStore::from_der_roots(roots)?,
            trusted_time,
            max_plaintext_fragment,
        )
    }

    /// Builds a TLS 1.2-only configuration from PEM-encoded roots.
    ///
    /// The caller-owned decode scratch must hold the largest decoded root and
    /// may be reused after this function returns.
    pub fn from_pem_roots<'a>(
        roots: impl IntoIterator<Item = &'a [u8]>,
        decode_scratch: &mut [u8],
        trusted_time: &impl TrustedTime,
        max_plaintext_fragment: usize,
    ) -> Result<Self, ConfigError> {
        Self::from_trust_roots(
            TlsRootStore::from_pem_roots(roots, decode_scratch)?,
            trusted_time,
            max_plaintext_fragment,
        )
    }

    /// Builds a TLS configuration from a previously validated trust bundle.
    pub fn from_trust_roots(
        trust_roots: TlsRootStore,
        trusted_time: &impl TrustedTime,
        max_plaintext_fragment: usize,
    ) -> Result<Self, ConfigError> {
        Self::from_root_store(trust_roots.inner, trusted_time, max_plaintext_fragment)
    }

    /// Builds a TLS 1.2-only configuration from a prepared root store.
    pub fn from_root_store(
        root_store: RootCertStore,
        trusted_time: &impl TrustedTime,
        max_plaintext_fragment: usize,
    ) -> Result<Self, ConfigError> {
        if root_store.is_empty() {
            return Err(ConfigError::EmptyRootStore);
        }
        if !(MIN_PLAINTEXT_FRAGMENT..=MAX_PLAINTEXT_FRAGMENT).contains(&max_plaintext_fragment) {
            return Err(ConfigError::InvalidFragmentSize);
        }

        let now = trusted_time.now().map_err(ConfigError::UntrustedTime)?;
        let mut provider = rustls_rustcrypto::provider();
        provider.cipher_suites = vec![
            rustls_rustcrypto::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
            rustls_rustcrypto::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        ];

        let builder =
            ClientConfig::builder_with_details(Arc::new(provider), Arc::new(SnapshotTime(now)))
                .with_protocol_versions(&[&TLS12])
                .map_err(ConfigError::InvalidCryptoPolicy)?;

        let mut inner = builder
            .with_root_certificates(root_store)
            .with_no_client_auth();
        inner.max_fragment_size = Some(max_plaintext_fragment);
        inner.enable_sni = true;

        Ok(Self {
            inner: Arc::new(inner),
            max_plaintext_fragment,
        })
    }

    /// Returns the configured maximum plaintext bytes per TLS record.
    #[must_use]
    pub const fn max_plaintext_fragment(&self) -> usize {
        self.max_plaintext_fragment
    }
}

/// Caller-owned record and decrypted-data storage for one TLS connection.
pub struct TlsBuffers<'a> {
    /// Encrypted bytes received but not yet consumed by rustls.
    pub incoming_tls: &'a mut [u8],
    /// Complete encoded TLS records waiting to be sent.
    pub outgoing_tls: &'a mut [u8],
    /// Decrypted application bytes received while another operation is driven.
    pub plaintext: &'a mut [u8],
}

/// TLS stream failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error<E> {
    /// The server name was not a valid DNS name.
    InvalidServerName,
    /// A TLS protocol, certificate, hostname, or time validation failed.
    Tls(rustls::Error),
    /// A caller-owned buffer could not hold required data.
    Capacity {
        /// Required bytes when known.
        required: usize,
        /// Available bytes.
        available: usize,
    },
    /// The underlying transport failed.
    Transport(E),
    /// The transport closed before TLS shutdown completed.
    UnexpectedEof,
    /// A non-empty transport write reported zero progress.
    WriteZero,
    /// The peer completed an authenticated TLS shutdown.
    Closed,
    /// rustls exposed a state unsupported by a TLS 1.2 client.
    UnexpectedState,
}

impl<E: fmt::Debug> fmt::Display for Error<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidServerName => formatter.write_str("invalid TLS server name"),
            Self::Tls(_) => formatter.write_str("TLS protocol or verification failure"),
            Self::Capacity {
                required,
                available,
            } => write!(
                formatter,
                "TLS buffer requires {required} bytes but only {available} are available"
            ),
            Self::Transport(error) => write!(formatter, "TLS transport error: {error:?}"),
            Self::UnexpectedEof => formatter.write_str("TLS transport closed unexpectedly"),
            Self::WriteZero => formatter.write_str("TLS transport wrote zero bytes"),
            Self::Closed => formatter.write_str("TLS connection is closed"),
            Self::UnexpectedState => formatter.write_str("unexpected TLS connection state"),
        }
    }
}

impl<E: fmt::Debug> core::error::Error for Error<E> {}

impl<E: embedded_io_async::Error> embedded_io_async::Error for Error<E> {
    fn kind(&self) -> embedded_io_async::ErrorKind {
        match self {
            Self::Transport(error) => error.kind(),
            Self::Capacity { .. } => embedded_io_async::ErrorKind::OutOfMemory,
            Self::InvalidServerName => embedded_io_async::ErrorKind::InvalidInput,
            Self::Tls(_) | Self::UnexpectedState => embedded_io_async::ErrorKind::InvalidData,
            Self::UnexpectedEof | Self::Closed => embedded_io_async::ErrorKind::NotConnected,
            Self::WriteZero => embedded_io_async::ErrorKind::WriteZero,
        }
    }
}

/// Authenticated TLS 1.2 stream over an asynchronous byte transport.
///
/// `TlsStream` is not cancellation-safe while a transport `write_all` is in
/// progress, matching the underlying embedded I/O contract. Dropping the
/// stream and reconnecting is the only safe recovery after cancellation.
pub struct TlsStream<'a, IO> {
    io: IO,
    connection: UnbufferedClientConnection,
    incoming_tls: &'a mut [u8],
    incoming_used: usize,
    outgoing_tls: &'a mut [u8],
    outgoing_used: usize,
    plaintext: &'a mut [u8],
    plaintext_start: usize,
    plaintext_end: usize,
    max_plaintext_fragment: usize,
    peer_closed: bool,
}

impl<'a, IO> TlsStream<'a, IO>
where
    IO: Read + Write,
{
    /// Performs a server-authenticated TLS handshake.
    ///
    /// The DNS server name is used for both SNI and certificate hostname
    /// verification. IP-address identities are intentionally rejected.
    pub async fn connect(
        io: IO,
        config: &TlsClientConfig,
        server_name: &str,
        buffers: TlsBuffers<'a>,
    ) -> Result<Self, Error<IO::Error>> {
        if buffers.incoming_tls.is_empty()
            || buffers.outgoing_tls.is_empty()
            || buffers.plaintext.is_empty()
        {
            return Err(Error::Capacity {
                required: 1,
                available: 0,
            });
        }
        let required_outgoing = config
            .max_plaintext_fragment
            .saturating_add(TLS12_RECORD_OVERHEAD);
        if buffers.outgoing_tls.len() < required_outgoing {
            return Err(Error::Capacity {
                required: required_outgoing,
                available: buffers.outgoing_tls.len(),
            });
        }

        let owned_name = ServerName::try_from(server_name)
            .map_err(|_| Error::InvalidServerName)?
            .to_owned();
        if !matches!(owned_name, ServerName::DnsName(_)) {
            return Err(Error::InvalidServerName);
        }
        let connection = UnbufferedClientConnection::new(config.inner.clone(), owned_name)
            .map_err(Error::Tls)?;

        let mut stream = Self {
            io,
            connection,
            incoming_tls: buffers.incoming_tls,
            incoming_used: 0,
            outgoing_tls: buffers.outgoing_tls,
            outgoing_used: 0,
            plaintext: buffers.plaintext,
            plaintext_start: 0,
            plaintext_end: 0,
            max_plaintext_fragment: config.max_plaintext_fragment,
            peer_closed: false,
        };
        stream.drive_handshake().await?;
        Ok(stream)
    }

    /// Returns a shared reference to the underlying byte transport.
    #[must_use]
    pub const fn get_ref(&self) -> &IO {
        &self.io
    }

    /// Returns a mutable reference to the underlying byte transport.
    #[must_use]
    pub fn get_mut(&mut self) -> &mut IO {
        &mut self.io
    }

    /// Consumes the TLS layer and returns the underlying transport.
    #[must_use]
    pub fn into_inner(self) -> IO {
        self.io
    }

    /// Sends TLS `close_notify` and flushes the underlying transport.
    pub async fn close(&mut self) -> Result<(), Error<IO::Error>> {
        if self.peer_closed {
            return Ok(());
        }

        loop {
            let status = self
                .connection
                .process_tls_records(&mut self.incoming_tls[..self.incoming_used]);
            let mut discard = status.discard;
            let mut completed = false;
            match status.state.map_err(Error::Tls)? {
                ConnectionState::WriteTraffic(mut state) => {
                    self.outgoing_used = state
                        .queue_close_notify(self.outgoing_tls)
                        .map_err(|error| map_encrypt_error(error, self.outgoing_tls.len()))?;
                    write_all(&mut self.io, &self.outgoing_tls[..self.outgoing_used]).await?;
                    self.io.flush().await.map_err(Error::Transport)?;
                    self.outgoing_used = 0;
                    completed = true;
                }
                ConnectionState::EncodeTlsData(mut state) => {
                    let available = self.outgoing_tls.len() - self.outgoing_used;
                    let written = state
                        .encode(&mut self.outgoing_tls[self.outgoing_used..])
                        .map_err(|error| map_encode_error(error, available))?;
                    self.outgoing_used += written;
                }
                ConnectionState::TransmitTlsData(state) => {
                    write_all(&mut self.io, &self.outgoing_tls[..self.outgoing_used]).await?;
                    self.io.flush().await.map_err(Error::Transport)?;
                    self.outgoing_used = 0;
                    state.done();
                }
                ConnectionState::ReadTraffic(mut state) => {
                    discard += stage_records(
                        &mut state,
                        self.plaintext,
                        &mut self.plaintext_start,
                        &mut self.plaintext_end,
                    )?;
                }
                ConnectionState::PeerClosed | ConnectionState::Closed => {
                    self.peer_closed = true;
                    completed = true;
                }
                ConnectionState::BlockedHandshake => self.receive_tls().await?,
                _ => return Err(Error::UnexpectedState),
            }
            self.discard_incoming(discard);
            if completed {
                return Ok(());
            }
        }
    }

    async fn drive_handshake(&mut self) -> Result<(), Error<IO::Error>> {
        loop {
            let status = self
                .connection
                .process_tls_records(&mut self.incoming_tls[..self.incoming_used]);
            let mut discard = status.discard;
            let mut connected = false;
            match status.state.map_err(Error::Tls)? {
                ConnectionState::EncodeTlsData(mut state) => {
                    let available = self.outgoing_tls.len() - self.outgoing_used;
                    let written = state
                        .encode(&mut self.outgoing_tls[self.outgoing_used..])
                        .map_err(|error| map_encode_error(error, available))?;
                    self.outgoing_used += written;
                }
                ConnectionState::TransmitTlsData(state) => {
                    write_all(&mut self.io, &self.outgoing_tls[..self.outgoing_used]).await?;
                    self.outgoing_used = 0;
                    state.done();
                }
                ConnectionState::BlockedHandshake => self.receive_tls().await?,
                ConnectionState::ReadTraffic(mut state) => {
                    discard += stage_records(
                        &mut state,
                        self.plaintext,
                        &mut self.plaintext_start,
                        &mut self.plaintext_end,
                    )?;
                }
                ConnectionState::WriteTraffic(_) => connected = true,
                ConnectionState::PeerClosed | ConnectionState::Closed => {
                    return Err(Error::UnexpectedEof);
                }
                _ => return Err(Error::UnexpectedState),
            }
            self.discard_incoming(discard);
            if connected {
                return Ok(());
            }
        }
    }

    async fn receive_tls(&mut self) -> Result<(), Error<IO::Error>> {
        if self.incoming_used == self.incoming_tls.len() {
            return Err(Error::Capacity {
                required: self.incoming_used.saturating_add(1),
                available: self.incoming_tls.len(),
            });
        }
        let read = self
            .io
            .read(&mut self.incoming_tls[self.incoming_used..])
            .await
            .map_err(Error::Transport)?;
        if read == 0 {
            return Err(Error::UnexpectedEof);
        }
        self.incoming_used += read;
        Ok(())
    }

    fn discard_incoming(&mut self, discard: usize) {
        debug_assert!(discard <= self.incoming_used);
        if discard != 0 {
            self.incoming_tls
                .copy_within(discard..self.incoming_used, 0);
            self.incoming_used -= discard;
        }
    }

    fn copy_staged(&mut self, output: &mut [u8]) -> usize {
        let available = self.plaintext_end - self.plaintext_start;
        let copied = cmp::min(available, output.len());
        output[..copied]
            .copy_from_slice(&self.plaintext[self.plaintext_start..self.plaintext_start + copied]);
        self.plaintext_start += copied;
        if self.plaintext_start == self.plaintext_end {
            self.plaintext_start = 0;
            self.plaintext_end = 0;
        }
        copied
    }
}

impl<IO> ErrorType for TlsStream<'_, IO>
where
    IO: ErrorType,
    IO::Error: embedded_io_async::Error,
{
    type Error = Error<IO::Error>;
}

impl<IO> Read for TlsStream<'_, IO>
where
    IO: Read + Write,
    IO::Error: embedded_io_async::Error,
{
    async fn read(&mut self, output: &mut [u8]) -> Result<usize, Self::Error> {
        if output.is_empty() {
            return Ok(0);
        }
        let staged = self.copy_staged(output);
        if staged != 0 {
            return Ok(staged);
        }
        if self.peer_closed {
            return Ok(0);
        }

        loop {
            let status = self
                .connection
                .process_tls_records(&mut self.incoming_tls[..self.incoming_used]);
            let mut discard = status.discard;
            let mut result = None;
            match status.state.map_err(Error::Tls)? {
                ConnectionState::ReadTraffic(mut state) => {
                    if let Some(record) = state.next_record() {
                        let record = record.map_err(Error::Tls)?;
                        discard += record.discard;
                        let capacity = output.len().saturating_add(self.plaintext.len());
                        if record.payload.len() > capacity {
                            return Err(Error::Capacity {
                                required: record.payload.len(),
                                available: capacity,
                            });
                        }
                        let direct = cmp::min(output.len(), record.payload.len());
                        output[..direct].copy_from_slice(&record.payload[..direct]);
                        let remainder = &record.payload[direct..];
                        self.plaintext[..remainder.len()].copy_from_slice(remainder);
                        self.plaintext_start = 0;
                        self.plaintext_end = remainder.len();
                        if direct != 0 {
                            result = Some(direct);
                        }
                    }
                }
                ConnectionState::EncodeTlsData(mut state) => {
                    let available = self.outgoing_tls.len() - self.outgoing_used;
                    let written = state
                        .encode(&mut self.outgoing_tls[self.outgoing_used..])
                        .map_err(|error| map_encode_error(error, available))?;
                    self.outgoing_used += written;
                }
                ConnectionState::TransmitTlsData(state) => {
                    write_all(&mut self.io, &self.outgoing_tls[..self.outgoing_used]).await?;
                    self.outgoing_used = 0;
                    state.done();
                }
                ConnectionState::BlockedHandshake | ConnectionState::WriteTraffic(_) => {
                    self.receive_tls().await?;
                }
                ConnectionState::PeerClosed | ConnectionState::Closed => {
                    self.peer_closed = true;
                    result = Some(0);
                }
                _ => return Err(Error::UnexpectedState),
            }
            self.discard_incoming(discard);
            if let Some(read) = result {
                return Ok(read);
            }
        }
    }
}

impl<IO> Write for TlsStream<'_, IO>
where
    IO: Read + Write,
    IO::Error: embedded_io_async::Error,
{
    async fn write(&mut self, input: &[u8]) -> Result<usize, Self::Error> {
        if input.is_empty() {
            return Ok(0);
        }
        if self.peer_closed {
            return Err(Error::Closed);
        }

        loop {
            let status = self
                .connection
                .process_tls_records(&mut self.incoming_tls[..self.incoming_used]);
            let mut discard = status.discard;
            let mut written_plaintext = None;
            match status.state.map_err(Error::Tls)? {
                ConnectionState::WriteTraffic(mut state) => {
                    let plaintext_len = cmp::min(input.len(), self.max_plaintext_fragment);
                    self.outgoing_used = state
                        .encrypt(&input[..plaintext_len], self.outgoing_tls)
                        .map_err(|error| map_encrypt_error(error, self.outgoing_tls.len()))?;
                    write_all(&mut self.io, &self.outgoing_tls[..self.outgoing_used]).await?;
                    self.outgoing_used = 0;
                    written_plaintext = Some(plaintext_len);
                }
                ConnectionState::ReadTraffic(mut state) => {
                    discard += stage_records(
                        &mut state,
                        self.plaintext,
                        &mut self.plaintext_start,
                        &mut self.plaintext_end,
                    )?;
                }
                ConnectionState::EncodeTlsData(mut state) => {
                    let available = self.outgoing_tls.len() - self.outgoing_used;
                    let written = state
                        .encode(&mut self.outgoing_tls[self.outgoing_used..])
                        .map_err(|error| map_encode_error(error, available))?;
                    self.outgoing_used += written;
                }
                ConnectionState::TransmitTlsData(state) => {
                    write_all(&mut self.io, &self.outgoing_tls[..self.outgoing_used]).await?;
                    self.outgoing_used = 0;
                    state.done();
                }
                ConnectionState::BlockedHandshake => self.receive_tls().await?,
                ConnectionState::PeerClosed | ConnectionState::Closed => {
                    self.peer_closed = true;
                    return Err(Error::Closed);
                }
                _ => return Err(Error::UnexpectedState),
            }
            self.discard_incoming(discard);
            if let Some(written) = written_plaintext {
                return Ok(written);
            }
        }
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.io.flush().await.map_err(Error::Transport)
    }
}

fn stage_records<E>(
    state: &mut rustls::unbuffered::ReadTraffic<'_, '_, rustls::client::ClientConnectionData>,
    plaintext: &mut [u8],
    plaintext_start: &mut usize,
    plaintext_end: &mut usize,
) -> Result<usize, Error<E>> {
    let mut discard = 0;
    while let Some(record) = state.next_record() {
        let record = record.map_err(Error::Tls)?;
        let live = *plaintext_end - *plaintext_start;
        if live != 0 && *plaintext_start != 0 {
            plaintext.copy_within(*plaintext_start..*plaintext_end, 0);
            *plaintext_start = 0;
            *plaintext_end = live;
        }
        let required = live.saturating_add(record.payload.len());
        if required > plaintext.len() {
            return Err(Error::Capacity {
                required,
                available: plaintext.len(),
            });
        }
        discard += record.discard;
        plaintext[*plaintext_end..required].copy_from_slice(record.payload);
        *plaintext_end = required;
    }
    Ok(discard)
}

fn map_encode_error<E>(error: EncodeError, available: usize) -> Error<E> {
    match error {
        EncodeError::InsufficientSize(size) => Error::Capacity {
            required: size.required_size,
            available,
        },
        EncodeError::AlreadyEncoded => Error::UnexpectedState,
    }
}

fn map_encrypt_error<E>(error: EncryptError, available: usize) -> Error<E> {
    match error {
        EncryptError::InsufficientSize(size) => Error::Capacity {
            required: size.required_size,
            available,
        },
        EncryptError::EncryptExhausted => Error::Tls(rustls::Error::EncryptError),
    }
}

async fn write_all<IO>(io: &mut IO, mut bytes: &[u8]) -> Result<(), Error<IO::Error>>
where
    IO: Write,
{
    while !bytes.is_empty() {
        let written = io.write(bytes).await.map_err(Error::Transport)?;
        if written == 0 {
            return Err(Error::WriteZero);
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: UnixTime = UnixTime::from_seconds(1_700_000_000);

    struct FixedTime(Result<UnixTime, TimeError>);

    impl TrustedTime for FixedTime {
        fn now(&self) -> Result<UnixTime, TimeError> {
            self.0
        }
    }

    #[test]
    fn rejects_empty_root_store() {
        assert!(matches!(
            TlsClientConfig::from_root_store(RootCertStore::empty(), &FixedTime(Ok(NOW)), 1024),
            Err(ConfigError::EmptyRootStore)
        ));
    }

    #[test]
    fn rejects_empty_and_wrongly_labeled_pem_root_bundles() {
        let mut scratch = [0_u8; 16];
        let empty: [&[u8]; 0] = [];
        assert!(matches!(
            TlsRootStore::from_pem_roots(empty, &mut scratch),
            Err(ConfigError::EmptyRootStore)
        ));

        let private_key = b"-----BEGIN PRIVATE KEY-----\nMAA=\n-----END PRIVATE KEY-----\n";
        assert!(matches!(
            TlsRootStore::from_pem_roots([private_key.as_slice()], &mut scratch),
            Err(ConfigError::UnexpectedPemLabel)
        ));
    }

    #[test]
    fn rejects_invalid_fragment_sizes() {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS
                .iter()
                .take(1)
                .cloned()
                .collect(),
        };
        assert!(matches!(
            TlsClientConfig::from_root_store(roots.clone(), &FixedTime(Ok(NOW)), 31),
            Err(ConfigError::InvalidFragmentSize)
        ));
        assert!(matches!(
            TlsClientConfig::from_root_store(roots, &FixedTime(Ok(NOW)), 16_385),
            Err(ConfigError::InvalidFragmentSize)
        ));
    }

    #[test]
    fn builds_restricted_tls12_policy() {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS
                .iter()
                .take(1)
                .cloned()
                .collect(),
        };
        let config = TlsClientConfig::from_root_store(roots, &FixedTime(Ok(NOW)), 1024).unwrap();
        assert_eq!(config.max_plaintext_fragment(), 1024);
        assert_eq!(config.inner.crypto_provider().cipher_suites.len(), 2);
    }

    #[test]
    fn fails_closed_without_trusted_time() {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS
                .iter()
                .take(1)
                .cloned()
                .collect(),
        };
        assert!(matches!(
            TlsClientConfig::from_root_store(roots, &FixedTime(Err(TimeError::Untrusted)), 1024),
            Err(ConfigError::UntrustedTime(TimeError::Untrusted))
        ));
    }
}
