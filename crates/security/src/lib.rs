#![no_std]
#![forbid(unsafe_code)]
#![doc = "Portable security capabilities and secret-lifetime primitives."]

use core::fmt;

use zeroize::Zeroize;

/// Maximum supported secret capacity.
pub const MAX_SECRET_CAPACITY: usize = u16::MAX as usize;

/// Error returned while constructing a bounded secret.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SecretError {
    /// The secret was empty.
    Empty,
    /// The secret exceeded the destination's fixed capacity.
    TooLong,
    /// The requested fixed capacity cannot be represented by this type.
    UnsupportedCapacity,
}

impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "secret must not be empty",
            Self::TooLong => "secret exceeds its fixed capacity",
            Self::UnsupportedCapacity => "secret capacity exceeds the supported limit",
        })
    }
}

impl core::error::Error for SecretError {}

/// An owned, fixed-capacity secret that zeroizes its complete storage on drop.
///
/// The value deliberately exposes no `AsRef`, `Display`, or ordinary slice
/// accessor. Code that must use the bytes does so inside [`with_secret`](Self::with_secret),
/// making secret access visible at the call site.
pub struct SecretBytes<const N: usize> {
    bytes: [u8; N],
    len: u16,
}

impl<const N: usize> SecretBytes<N> {
    /// Copies a non-empty secret into fixed-capacity storage.
    pub fn new(value: &[u8]) -> Result<Self, SecretError> {
        if N > MAX_SECRET_CAPACITY {
            return Err(SecretError::UnsupportedCapacity);
        }
        if value.is_empty() {
            return Err(SecretError::Empty);
        }
        if value.len() > N {
            return Err(SecretError::TooLong);
        }

        let mut bytes = [0; N];
        bytes[..value.len()].copy_from_slice(value);
        Ok(Self {
            bytes,
            len: value.len() as u16,
        })
    }

    /// Returns the number of live secret bytes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns whether the live secret is empty.
    ///
    /// Constructed values are never empty; this is provided for generic
    /// bounded-buffer inspection.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Temporarily exposes only the initialized bytes to a caller operation.
    pub fn with_secret<R>(&self, operation: impl FnOnce(&[u8]) -> R) -> R {
        operation(&self.bytes[..usize::from(self.len)])
    }
}

impl<const N: usize> fmt::Debug for SecretBytes<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretBytes(**REDACTED**)")
    }
}

impl<const N: usize> Drop for SecretBytes<N> {
    fn drop(&mut self) {
        self.bytes.zeroize();
        self.len.zeroize();
    }
}

/// Trusted Unix time in whole seconds.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct UnixTime(u64);

impl UnixTime {
    /// Constructs a trusted timestamp supplied by a trusted-time provider.
    #[must_use]
    pub const fn from_seconds(seconds: u64) -> Self {
        Self(seconds)
    }

    /// Returns seconds since the Unix epoch.
    #[must_use]
    pub const fn as_seconds(self) -> u64 {
        self.0
    }

    /// Adds a duration, returning `None` on overflow.
    #[must_use]
    pub const fn checked_add(self, seconds: u64) -> Option<Self> {
        match self.0.checked_add(seconds) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// Why trusted wall-clock time is unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TimeError {
    /// No authenticated or persisted lower-bound time is available yet.
    Untrusted,
    /// The time source failed while refreshing its trusted value.
    Unavailable,
    /// Persisted time moved backwards or failed integrity validation.
    InvalidLowerBound,
}

/// Source of wall-clock time suitable for certificate and credential checks.
pub trait TrustedTime {
    /// Returns trusted time, or fails closed while trust is unavailable.
    fn now(&self) -> Result<UnixTime, TimeError>;
}

/// Source of cryptographically secure random bytes.
pub trait SecureRandom {
    /// Provider-specific failure type.
    type Error;

    /// Fills the entire output or returns an error without claiming success.
    fn fill_bytes(&mut self, output: &mut [u8]) -> Result<(), Self::Error>;
}

/// Error returned for an invalid credential lifetime policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CredentialLeaseError {
    /// Expiry was not later than issuance.
    InvalidExpiry,
    /// The refresh margin reached before or at issuance.
    InvalidRefreshMargin,
}

/// Current action required for a time-bounded credential.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialState {
    /// The credential may be used without starting refresh yet.
    Active,
    /// Refresh should begin, but the credential has not expired.
    RefreshDue,
    /// The credential must no longer be used for a new connection.
    Expired,
}

/// Non-secret absolute lifetime and refresh policy for a credential.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CredentialLease {
    issued_at: UnixTime,
    refresh_at: UnixTime,
    expires_at: UnixTime,
}

impl CredentialLease {
    /// Creates a lease whose refresh margin is measured backwards from expiry.
    pub const fn new(
        issued_at: UnixTime,
        expires_at: UnixTime,
        refresh_margin_seconds: u64,
    ) -> Result<Self, CredentialLeaseError> {
        if expires_at.0 <= issued_at.0 {
            return Err(CredentialLeaseError::InvalidExpiry);
        }
        if refresh_margin_seconds == 0 {
            return Err(CredentialLeaseError::InvalidRefreshMargin);
        }
        let Some(refresh_at) = expires_at.0.checked_sub(refresh_margin_seconds) else {
            return Err(CredentialLeaseError::InvalidRefreshMargin);
        };
        if refresh_at <= issued_at.0 {
            return Err(CredentialLeaseError::InvalidRefreshMargin);
        }
        Ok(Self {
            issued_at,
            refresh_at: UnixTime(refresh_at),
            expires_at,
        })
    }

    /// Returns the trusted issuance time.
    #[must_use]
    pub const fn issued_at(self) -> UnixTime {
        self.issued_at
    }

    /// Returns when controlled refresh should start.
    #[must_use]
    pub const fn refresh_at(self) -> UnixTime {
        self.refresh_at
    }

    /// Returns the hard expiry after which the credential is unusable.
    #[must_use]
    pub const fn expires_at(self) -> UnixTime {
        self.expires_at
    }

    /// Classifies the credential at trusted `now`.
    #[must_use]
    pub const fn state_at(self, now: UnixTime) -> CredentialState {
        if now.0 >= self.expires_at.0 {
            CredentialState::Expired
        } else if now.0 >= self.refresh_at.0 {
            CredentialState::RefreshDue
        } else {
            CredentialState::Active
        }
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write;

    use super::*;

    struct StackText<const N: usize> {
        bytes: [u8; N],
        len: usize,
    }

    impl<const N: usize> StackText<N> {
        const fn new() -> Self {
            Self {
                bytes: [0; N],
                len: 0,
            }
        }

        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.bytes[..self.len]).unwrap()
        }
    }

    impl<const N: usize> Write for StackText<N> {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            let end = self.len.checked_add(value.len()).ok_or(fmt::Error)?;
            let output = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
            output.copy_from_slice(value.as_bytes());
            self.len = end;
            Ok(())
        }
    }

    #[test]
    fn secret_access_is_explicit_and_debug_is_redacted() {
        let secret = SecretBytes::<16>::new(b"device-key").unwrap();
        assert_eq!(secret.len(), 10);
        assert_eq!(secret.with_secret(|value| value.len()), 10);

        let mut debug = StackText::<64>::new();
        write!(&mut debug, "{secret:?}").unwrap();
        assert_eq!(debug.as_str(), "SecretBytes(**REDACTED**)");
        assert!(!debug.as_str().contains("device-key"));
    }

    #[test]
    fn secret_bounds_are_checked() {
        assert!(matches!(
            SecretBytes::<4>::new(b""),
            Err(SecretError::Empty)
        ));
        assert!(matches!(
            SecretBytes::<4>::new(b"12345"),
            Err(SecretError::TooLong)
        ));
        assert!(matches!(
            SecretBytes::<70000>::new(b"x"),
            Err(SecretError::UnsupportedCapacity)
        ));
    }

    #[test]
    fn credential_lease_has_distinct_refresh_and_expiry_boundaries() {
        let issued = UnixTime::from_seconds(1_000);
        let lease = CredentialLease::new(issued, UnixTime::from_seconds(1_600), 120).unwrap();
        assert_eq!(lease.refresh_at().as_seconds(), 1_480);
        assert_eq!(
            lease.state_at(UnixTime::from_seconds(1_479)),
            CredentialState::Active
        );
        assert_eq!(
            lease.state_at(UnixTime::from_seconds(1_480)),
            CredentialState::RefreshDue
        );
        assert_eq!(
            lease.state_at(UnixTime::from_seconds(1_600)),
            CredentialState::Expired
        );
    }

    #[test]
    fn credential_lease_rejects_unsafe_policy() {
        let issued = UnixTime::from_seconds(100);
        assert_eq!(
            CredentialLease::new(issued, issued, 10),
            Err(CredentialLeaseError::InvalidExpiry)
        );
        assert_eq!(
            CredentialLease::new(issued, UnixTime::from_seconds(120), 20),
            Err(CredentialLeaseError::InvalidRefreshMargin)
        );
        assert_eq!(
            CredentialLease::new(issued, UnixTime::from_seconds(120), 0),
            Err(CredentialLeaseError::InvalidRefreshMargin)
        );
    }
}
