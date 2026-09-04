use core::fmt;

use base64ct::{Base64, Encoding};
use embedded_sdk_security::{
    CredentialLease, CredentialLeaseError, CredentialState, SecretBytes, TimeError, TrustedTime,
    UnixTime,
};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::{HubConfig, encode::write_decimal};

/// Maximum decoded device symmetric-key length supported by the provider.
pub const MAX_SYMMETRIC_KEY_LEN: usize = 64;
/// Fixed capacity reserved for a device-scoped IoT Hub SAS token.
pub const MAX_SAS_TOKEN_LEN: usize = 896;

const MAX_ENCODED_RESOURCE_LEN: usize = 650;
const MAX_BASE64_SIGNATURE_LEN: usize = 44;

/// A decoded device-scoped Azure IoT Hub symmetric key.
pub struct SymmetricKey(SecretBytes<MAX_SYMMETRIC_KEY_LEN>);

impl SymmetricKey {
    /// Decodes the base64 key returned for an IoT Hub device identity.
    pub fn from_base64(encoded: &str) -> Result<Self, SasError> {
        let mut decoded = Zeroizing::new([0_u8; MAX_SYMMETRIC_KEY_LEN]);
        Base64::decode(encoded, &mut decoded[..])
            .map_err(|_| SasError::InvalidSymmetricKey)
            .and_then(|value| {
                SecretBytes::new(value)
                    .map(Self)
                    .map_err(|_| SasError::InvalidSymmetricKey)
            })
    }

    fn with_key<R>(&self, operation: impl FnOnce(&[u8]) -> R) -> R {
        self.0.with_secret(operation)
    }
}

impl fmt::Debug for SymmetricKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SymmetricKey(**REDACTED**)")
    }
}

/// Error returned while creating a device-scoped SAS credential.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SasError {
    /// The device symmetric key was malformed or exceeded the supported bound.
    InvalidSymmetricKey,
    /// Trusted wall-clock time is not currently available.
    Time(TimeError),
    /// Expiry calculation overflowed Unix time.
    ExpiryOverflow,
    /// Validity and refresh policy did not create a safe credential lease.
    InvalidLease(CredentialLeaseError),
    /// An internal fixed-capacity encoding bound was insufficient.
    Capacity,
}

impl fmt::Display for SasError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSymmetricKey => "invalid Azure IoT device symmetric key",
            Self::Time(_) => "trusted time is unavailable for Azure IoT SAS authentication",
            Self::ExpiryOverflow => "Azure IoT SAS expiry overflowed Unix time",
            Self::InvalidLease(_) => "invalid Azure IoT SAS lifetime policy",
            Self::Capacity => "Azure IoT SAS token exceeded its fixed capacity",
        })
    }
}

impl core::error::Error for SasError {}

/// An owned, redacted, device-scoped SAS token and its refresh policy.
pub struct SasToken {
    password: SecretBytes<MAX_SAS_TOKEN_LEN>,
    lease: CredentialLease,
}

impl SasToken {
    /// Returns the non-secret lifetime metadata.
    #[must_use]
    pub const fn lease(&self) -> CredentialLease {
        self.lease
    }

    /// Returns the credential state at trusted `now`.
    #[must_use]
    pub const fn state_at(&self, now: UnixTime) -> CredentialState {
        self.lease.state_at(now)
    }

    /// Temporarily exposes the MQTT password bytes to connection setup.
    pub fn with_password<R>(&self, operation: impl FnOnce(&[u8]) -> R) -> R {
        self.password.with_secret(operation)
    }
}

impl fmt::Debug for SasToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SasToken")
            .field("password", &"**REDACTED**")
            .field("lease", &self.lease)
            .finish()
    }
}

/// Generates a device-scoped IoT Hub SAS token using trusted wall-clock time.
///
/// Device identity tokens omit the shared-policy-name (`skn`) field. The
/// resource URI is lower-cased and percent encoded before HMAC-SHA256 signing.
pub fn generate_device_sas(
    config: &HubConfig,
    key: &SymmetricKey,
    time: &impl TrustedTime,
    validity_seconds: u64,
    refresh_margin_seconds: u64,
) -> Result<SasToken, SasError> {
    let issued_at = time.now().map_err(SasError::Time)?;
    let expires_at = issued_at
        .checked_add(validity_seconds)
        .ok_or(SasError::ExpiryOverflow)?;
    let lease = CredentialLease::new(issued_at, expires_at, refresh_margin_seconds)
        .map_err(SasError::InvalidLease)?;

    let mut resource = Zeroizing::new([0_u8; MAX_ENCODED_RESOURCE_LEN]);
    let resource_len = write_resource_uri(config, &mut resource[..])?;

    let mut expiry = Zeroizing::new([0_u8; 20]);
    let expiry_len = write_decimal(&mut expiry[..], 0, expires_at.as_seconds());

    type HmacSha256 = Hmac<Sha256>;
    let mut calculated_signature = key.with_key(|key| {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts every key length");
        mac.update(&resource[..resource_len]);
        mac.update(b"\n");
        mac.update(&expiry[..expiry_len]);
        mac.finalize().into_bytes()
    });
    let mut signature = Zeroizing::new([0_u8; 32]);
    signature.copy_from_slice(&calculated_signature[..]);
    calculated_signature[..].zeroize();

    let mut base64_signature = Zeroizing::new([0_u8; MAX_BASE64_SIGNATURE_LEN]);
    let encoded_signature = Base64::encode(&signature[..], &mut base64_signature[..])
        .map_err(|_| SasError::Capacity)?;
    let mut token = Zeroizing::new([0_u8; MAX_SAS_TOKEN_LEN]);
    let token_len = write_token(
        &resource[..resource_len],
        encoded_signature,
        &expiry[..expiry_len],
        &mut token[..],
    )?;
    let password = SecretBytes::new(&token[..token_len]).map_err(|_| SasError::Capacity)?;

    Ok(SasToken { password, lease })
}

fn write_resource_uri(config: &HubConfig, output: &mut [u8]) -> Result<usize, SasError> {
    let mut offset = 0;
    for segment in [
        config.mqtt().hostname().as_str(),
        "/devices/",
        config.device_id().as_str(),
    ] {
        for byte in segment.bytes() {
            let byte = byte.to_ascii_lowercase();
            if is_sas_safe(byte) {
                write_byte(output, &mut offset, byte)?;
            } else {
                write_byte(output, &mut offset, b'%')?;
                write_byte(output, &mut offset, hex_digit(byte >> 4))?;
                write_byte(output, &mut offset, hex_digit(byte & 0x0f))?;
            }
        }
    }
    Ok(offset)
}

fn write_token(
    resource: &[u8],
    signature: &str,
    expiry: &[u8],
    output: &mut [u8],
) -> Result<usize, SasError> {
    let mut offset = 0;
    write_bytes(output, &mut offset, b"SharedAccessSignature sr=")?;
    write_bytes(output, &mut offset, resource)?;
    write_bytes(output, &mut offset, b"&sig=")?;
    for byte in signature.bytes() {
        if is_sas_safe(byte) {
            write_byte(output, &mut offset, byte)?;
        } else {
            write_byte(output, &mut offset, b'%')?;
            write_byte(output, &mut offset, hex_digit(byte >> 4))?;
            write_byte(output, &mut offset, hex_digit(byte & 0x0f))?;
        }
    }
    write_bytes(output, &mut offset, b"&se=")?;
    write_bytes(output, &mut offset, expiry)?;
    Ok(offset)
}

fn write_bytes(output: &mut [u8], offset: &mut usize, value: &[u8]) -> Result<(), SasError> {
    let end = offset.checked_add(value.len()).ok_or(SasError::Capacity)?;
    let destination = output.get_mut(*offset..end).ok_or(SasError::Capacity)?;
    destination.copy_from_slice(value);
    *offset = end;
    Ok(())
}

fn write_byte(output: &mut [u8], offset: &mut usize, value: u8) -> Result<(), SasError> {
    let destination = output.get_mut(*offset).ok_or(SasError::Capacity)?;
    *destination = value;
    *offset += 1;
    Ok(())
}

const fn is_sas_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

const fn hex_digit(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        _ => b'A' + value - 10,
    }
}

#[cfg(test)]
mod tests {
    use core::str;

    use embedded_sdk_security::{TimeError, TrustedTime};

    use crate::{DeviceId, HubHostname};

    use super::*;

    struct FixedTime(Result<UnixTime, TimeError>);

    impl TrustedTime for FixedTime {
        fn now(&self) -> Result<UnixTime, TimeError> {
            self.0
        }
    }

    fn config() -> HubConfig {
        HubConfig::new(
            HubHostname::new("Contoso.azure-devices.net").unwrap(),
            DeviceId::new("Sensor-01").unwrap(),
            240,
            1024,
        )
        .unwrap()
    }

    #[test]
    fn generates_device_scoped_redacted_sas_token() {
        let key =
            SymmetricKey::from_base64("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=").unwrap();
        let token = generate_device_sas(
            &config(),
            &key,
            &FixedTime(Ok(UnixTime::from_seconds(1_700_000_000))),
            3_600,
            300,
        )
        .unwrap();

        token.with_password(|password| {
            assert_eq!(
                str::from_utf8(password).unwrap(),
                "SharedAccessSignature sr=contoso.azure-devices.net%2Fdevices%2Fsensor-01&sig=AfLCEHwOjCbtLyKwa9rrJGKG4QJ%2BKJlYuZKWrlnw5Cg%3D&se=1700003600"
            );
        });
        assert_eq!(token.lease().expires_at().as_seconds(), 1_700_003_600);
        assert_eq!(token.lease().refresh_at().as_seconds(), 1_700_003_300);
        assert_eq!(
            std::format!("{token:?}"),
            "SasToken { password: \"**REDACTED**\", lease: CredentialLease { issued_at: UnixTime(1700000000), refresh_at: UnixTime(1700003300), expires_at: UnixTime(1700003600) } }"
        );
        assert_eq!(std::format!("{key:?}"), "SymmetricKey(**REDACTED**)");
    }

    #[test]
    fn rejects_bad_keys_untrusted_time_and_unsafe_lifetimes() {
        assert!(matches!(
            SymmetricKey::from_base64("not base64"),
            Err(SasError::InvalidSymmetricKey)
        ));
        let key = SymmetricKey::from_base64("a2V5").unwrap();
        assert!(matches!(
            generate_device_sas(
                &config(),
                &key,
                &FixedTime(Err(TimeError::Untrusted)),
                3_600,
                300
            ),
            Err(SasError::Time(TimeError::Untrusted))
        ));
        assert!(matches!(
            generate_device_sas(
                &config(),
                &key,
                &FixedTime(Ok(UnixTime::from_seconds(10))),
                300,
                300
            ),
            Err(SasError::InvalidLease(_))
        ));
    }
}
