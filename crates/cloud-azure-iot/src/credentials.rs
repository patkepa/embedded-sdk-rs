use core::{fmt, str};

use embedded_sdk_security::TrustedTime;
use zeroize::Zeroizing;

use crate::{HubConfig, SasError, SasToken, SymmetricKey, generate_device_sas};

/// Largest base64 encoding of a supported decoded symmetric key.
pub const MAX_BASE64_SYMMETRIC_KEY_LEN: usize = 88;

/// Source of one device-scoped IoT Hub symmetric key.
///
/// Implementations may read protected flash, a provisioning service, or a
/// secure peripheral. They write base64 key text into caller-owned storage and
/// return its initialized length. They must not log or retain that storage.
#[allow(async_fn_in_trait)]
pub trait SasKeySource {
    /// Backend-specific failure that must not contain key material.
    type Error;

    /// Loads one complete base64 device key into `output`.
    async fn load_base64_key(&mut self, output: &mut [u8]) -> Result<usize, Self::Error>;
}

/// Source of short-lived device-scoped SAS credentials.
#[allow(async_fn_in_trait)]
pub trait SasCredentialProvider {
    /// Provider-specific failure with secret-safe formatting.
    type Error;

    /// Acquires a fresh credential for this exact hub/device identity.
    async fn acquire(&mut self, config: &HubConfig) -> Result<SasToken, Self::Error>;
}

/// Invalid SAS refresh policy rejected before touching a key source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SasProviderConfigError {
    /// Validity must exceed the nonzero refresh margin.
    InvalidLifetime,
}

impl fmt::Display for SasProviderConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid Azure IoT SAS provider lifetime policy")
    }
}

impl core::error::Error for SasProviderConfigError {}

/// Failure while loading or generating a short-lived SAS credential.
#[non_exhaustive]
pub enum SasCredentialError<E> {
    /// The protected or injected key source failed.
    Source(E),
    /// The key source returned an empty or impossible initialized length.
    InvalidSourceLength,
    /// The key source returned bytes that were not UTF-8 text.
    InvalidSourceEncoding,
    /// Device-scoped SAS generation failed.
    Token(SasError),
}

impl<E> fmt::Debug for SasCredentialError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(_) => formatter.write_str("SasCredentialError::Source(**REDACTED**)"),
            Self::InvalidSourceLength => {
                formatter.write_str("SasCredentialError::InvalidSourceLength")
            }
            Self::InvalidSourceEncoding => {
                formatter.write_str("SasCredentialError::InvalidSourceEncoding")
            }
            Self::Token(error) => formatter
                .debug_tuple("SasCredentialError::Token")
                .field(error)
                .finish(),
        }
    }
}

impl<E> fmt::Display for SasCredentialError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Source(_) => "Azure IoT SAS key source failed",
            Self::InvalidSourceLength => "Azure IoT SAS key source returned an invalid length",
            Self::InvalidSourceEncoding => "Azure IoT SAS key source returned invalid UTF-8",
            Self::Token(_) => "Azure IoT SAS credential generation failed",
        })
    }
}

impl<E> core::error::Error for SasCredentialError<E> {}

/// Runtime SAS provider over an injected key source and trusted clock.
///
/// The provider retains the source, clock, and non-secret policy. Each
/// acquisition uses a temporary zeroizing base64 buffer and drops the decoded
/// key immediately after creating the short-lived [`SasToken`].
pub struct DeviceSasProvider<K, T> {
    key_source: K,
    time: T,
    validity_seconds: u64,
    refresh_margin_seconds: u64,
}

impl<K, T> DeviceSasProvider<K, T> {
    /// Validates refresh policy without loading secret material.
    pub fn new(
        key_source: K,
        time: T,
        validity_seconds: u64,
        refresh_margin_seconds: u64,
    ) -> Result<Self, SasProviderConfigError> {
        if refresh_margin_seconds == 0 || validity_seconds <= refresh_margin_seconds {
            return Err(SasProviderConfigError::InvalidLifetime);
        }
        Ok(Self {
            key_source,
            time,
            validity_seconds,
            refresh_margin_seconds,
        })
    }

    /// Returns mutable access for source-specific rotation or health policy.
    pub const fn key_source_mut(&mut self) -> &mut K {
        &mut self.key_source
    }

    /// Returns the trusted-time provider.
    pub const fn time(&self) -> &T {
        &self.time
    }

    /// Releases the owned key source and trusted-time provider.
    pub fn into_parts(self) -> (K, T) {
        (self.key_source, self.time)
    }
}

impl<K, T> SasCredentialProvider for DeviceSasProvider<K, T>
where
    K: SasKeySource,
    T: TrustedTime,
{
    type Error = SasCredentialError<K::Error>;

    async fn acquire(&mut self, config: &HubConfig) -> Result<SasToken, Self::Error> {
        let mut encoded = Zeroizing::new([0_u8; MAX_BASE64_SYMMETRIC_KEY_LEN]);
        let len = self
            .key_source
            .load_base64_key(&mut encoded[..])
            .await
            .map_err(SasCredentialError::Source)?;
        let encoded = encoded
            .get(..len)
            .filter(|value| !value.is_empty())
            .ok_or(SasCredentialError::InvalidSourceLength)?;
        let encoded =
            str::from_utf8(encoded).map_err(|_| SasCredentialError::InvalidSourceEncoding)?;
        let key = SymmetricKey::from_base64(encoded).map_err(SasCredentialError::Token)?;
        generate_device_sas(
            config,
            &key,
            &self.time,
            self.validity_seconds,
            self.refresh_margin_seconds,
        )
        .map_err(SasCredentialError::Token)
    }
}

#[cfg(test)]
mod tests {
    use core::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    use embedded_sdk_security::{TimeError, UnixTime};

    use super::*;
    use crate::{DeviceId, HubHostname};

    const KEY: &[u8] = b"MDEyMzQ1Njc4OWFiY2RlZg==";

    struct FixedTime;

    impl TrustedTime for FixedTime {
        fn now(&self) -> Result<UnixTime, TimeError> {
            Ok(UnixTime::from_seconds(1_700_000_000))
        }
    }

    struct MemoryKeySource(Result<usize, &'static str>);

    impl SasKeySource for MemoryKeySource {
        type Error = &'static str;

        async fn load_base64_key(&mut self, output: &mut [u8]) -> Result<usize, Self::Error> {
            let len = self.0?;
            if len <= KEY.len() && len <= output.len() {
                output[..len].copy_from_slice(&KEY[..len]);
            }
            Ok(len)
        }
    }

    fn hub() -> HubConfig {
        HubConfig::new(
            HubHostname::new("unit.azure-devices.net").unwrap(),
            DeviceId::new("sensor-01").unwrap(),
            60,
            1024,
        )
        .unwrap()
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = core::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn provider_loads_a_runtime_key_and_returns_a_short_lived_token() {
        let mut provider =
            DeviceSasProvider::new(MemoryKeySource(Ok(KEY.len())), FixedTime, 3_600, 300).unwrap();
        let token = block_on(provider.acquire(&hub())).unwrap();
        assert_eq!(token.lease().issued_at().as_seconds(), 1_700_000_000);
        assert_eq!(token.lease().expires_at().as_seconds(), 1_700_003_600);
        assert!(
            token
                .expose_password()
                .as_bytes()
                .starts_with(b"SharedAccessSignature ")
        );
    }

    #[test]
    fn provider_rejects_bad_policy_and_source_results_without_leaking_errors() {
        assert!(matches!(
            DeviceSasProvider::new(MemoryKeySource(Ok(KEY.len())), FixedTime, 300, 300),
            Err(SasProviderConfigError::InvalidLifetime)
        ));

        let mut failed =
            DeviceSasProvider::new(MemoryKeySource(Err("raw-key-secret")), FixedTime, 600, 60)
                .unwrap();
        let error = block_on(failed.acquire(&hub())).unwrap_err();
        let debug = std::format!("{error:?}");
        assert_eq!(debug, "SasCredentialError::Source(**REDACTED**)");
        assert!(!debug.contains("raw-key-secret"));

        let mut oversized = DeviceSasProvider::new(
            MemoryKeySource(Ok(MAX_BASE64_SYMMETRIC_KEY_LEN + 1)),
            FixedTime,
            600,
            60,
        )
        .unwrap();
        assert!(matches!(
            block_on(oversized.acquire(&hub())),
            Err(SasCredentialError::InvalidSourceLength)
        ));
    }
}
