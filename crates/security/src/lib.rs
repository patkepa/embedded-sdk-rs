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

/// Explicit scoped borrow of secret bytes.
///
/// This wrapper makes secret exposure visible at an async call site while
/// keeping formatting redacted and preventing the borrow from outliving its
/// owning [`SecretBytes`].
pub struct ExposedSecret<'a>(&'a [u8]);

impl ExposedSecret<'_> {
    /// Returns the borrowed bytes for immediate use by a security consumer.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8] {
        self.0
    }
}

impl fmt::Debug for ExposedSecret<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExposedSecret(**REDACTED**)")
    }
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

    /// Borrows a redacted view suitable for an asynchronous security call.
    #[must_use]
    pub fn expose_secret(&self) -> ExposedSecret<'_> {
        ExposedSecret(&self.bytes[..usize::from(self.len)])
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

/// Monotonic elapsed-time source used to advance a trusted wall-clock anchor.
///
/// Implementations must not move backwards during one boot. This trait does
/// not establish wall-clock trust by itself.
pub trait MonotonicClock {
    /// Returns milliseconds since an implementation-defined boot-local epoch.
    fn now_millis(&self) -> u64;
}

/// Trusted wall clock advanced from a trusted snapshot and monotonic uptime.
///
/// The caller is responsible for authenticating or integrity-checking every
/// Unix-time anchor before construction or refresh. An unauthenticated network
/// time response must not be promoted to trusted merely by passing it here.
pub struct AnchoredTrustedTime<C> {
    clock: C,
    unix_anchor: UnixTime,
    monotonic_anchor_ms: u64,
}

impl<C: MonotonicClock> AnchoredTrustedTime<C> {
    /// Anchors trusted Unix time to the clock's current monotonic instant.
    #[must_use]
    pub fn new(clock: C, trusted_now: UnixTime) -> Self {
        let monotonic_anchor_ms = clock.now_millis();
        Self {
            clock,
            unix_anchor: trusted_now,
            monotonic_anchor_ms,
        }
    }

    /// Replaces the anchor after the caller obtains a newer trusted snapshot.
    ///
    /// A snapshot older than the currently derived time is rejected without
    /// changing the active anchor.
    pub fn advance(&mut self, trusted_now: UnixTime) -> Result<(), TimeError> {
        let monotonic_now_ms = self.clock.now_millis();
        let current = self.time_at(monotonic_now_ms)?;
        if trusted_now < current {
            return Err(TimeError::InvalidLowerBound);
        }
        self.unix_anchor = trusted_now;
        self.monotonic_anchor_ms = monotonic_now_ms;
        Ok(())
    }

    /// Returns the wrapped boot-local monotonic source.
    #[must_use]
    pub const fn clock(&self) -> &C {
        &self.clock
    }

    /// Releases the wrapped boot-local monotonic source.
    #[must_use]
    pub fn into_clock(self) -> C {
        self.clock
    }

    fn time_at(&self, monotonic_now_ms: u64) -> Result<UnixTime, TimeError> {
        let elapsed_ms = monotonic_now_ms
            .checked_sub(self.monotonic_anchor_ms)
            .ok_or(TimeError::InvalidLowerBound)?;
        self.unix_anchor
            .checked_add(elapsed_ms / 1_000)
            .ok_or(TimeError::Unavailable)
    }
}

impl<C: MonotonicClock> TrustedTime for AnchoredTrustedTime<C> {
    fn now(&self) -> Result<UnixTime, TimeError> {
        self.time_at(self.clock.now_millis())
    }
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
    use core::{cell::Cell, fmt::Write};

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

    struct TestMonotonicClock(Cell<u64>);

    impl MonotonicClock for TestMonotonicClock {
        fn now_millis(&self) -> u64 {
            self.0.get()
        }
    }

    #[test]
    fn anchored_time_advances_only_from_monotonic_elapsed_time() {
        let clock = TestMonotonicClock(Cell::new(25_000));
        let time = AnchoredTrustedTime::new(clock, UnixTime::from_seconds(1_700_000_000));
        time.clock().0.set(27_999);
        assert_eq!(time.now().unwrap().as_seconds(), 1_700_000_002);
    }

    #[test]
    fn anchored_time_rejects_backward_clock_and_stale_refresh() {
        let clock = TestMonotonicClock(Cell::new(5_000));
        let mut time = AnchoredTrustedTime::new(clock, UnixTime::from_seconds(100));
        time.clock().0.set(9_000);
        assert_eq!(
            time.advance(UnixTime::from_seconds(103)),
            Err(TimeError::InvalidLowerBound)
        );
        assert_eq!(time.now().unwrap(), UnixTime::from_seconds(104));

        time.advance(UnixTime::from_seconds(110)).unwrap();
        time.clock().0.set(10_500);
        assert_eq!(time.now().unwrap(), UnixTime::from_seconds(111));
        time.clock().0.set(8_999);
        assert_eq!(time.now(), Err(TimeError::InvalidLowerBound));
    }

    #[test]
    fn secret_access_is_explicit_and_debug_is_redacted() {
        let secret = SecretBytes::<16>::new(b"device-key").unwrap();
        assert_eq!(secret.len(), 10);
        assert_eq!(secret.with_secret(|value| value.len()), 10);
        assert_eq!(secret.expose_secret().as_bytes().len(), 10);
        let mut exposed_debug = StackText::<64>::new();
        write!(&mut exposed_debug, "{:?}", secret.expose_secret()).unwrap();
        assert_eq!(exposed_debug.as_str(), "ExposedSecret(**REDACTED**)");

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
