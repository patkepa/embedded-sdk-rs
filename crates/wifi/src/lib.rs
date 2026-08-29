#![no_std]
#![forbid(unsafe_code)]
#![doc = "Platform-independent Wi-Fi configuration and status types."]

use core::{fmt, str};
use zeroize::Zeroize;

/// Maximum IEEE 802.11 service-set identifier length, in bytes.
pub const MAX_SSID_LEN: usize = 32;
/// Maximum WPA personal passphrase or raw PSK length, in bytes.
pub const MAX_PASSPHRASE_LEN: usize = 64;

/// Error returned while constructing a Wi-Fi value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// A station cannot connect without an SSID.
    EmptySsid,
    /// The SSID exceeds [`MAX_SSID_LEN`].
    SsidTooLong,
    /// A WPA passphrase must be 8 through 63 bytes, or a 64-digit hexadecimal PSK.
    InvalidPassphrase,
    /// Reconnection delays must be non-zero, ordered, and keep jitter below the maximum.
    InvalidReconnectPolicy,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySsid => formatter.write_str("station SSID must not be empty"),
            Self::SsidTooLong => formatter.write_str("SSID exceeds 32 bytes"),
            Self::InvalidPassphrase => formatter
                .write_str("WPA passphrase must be 8-63 bytes, or a 64-digit hexadecimal PSK"),
            Self::InvalidReconnectPolicy => formatter
                .write_str("reconnect policy requires 0 < initial <= maximum and jitter < maximum"),
        }
    }
}

impl core::error::Error for ConfigError {}

/// An IEEE 802.11 service-set identifier stored without allocation.
///
/// An SSID is bytes rather than necessarily UTF-8. Use [`Self::as_str`] only
/// when a human-readable representation is required.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Ssid {
    bytes: [u8; MAX_SSID_LEN],
    len: u8,
}

impl Ssid {
    /// Creates an SSID from its exact over-the-air bytes.
    pub fn new(value: &[u8]) -> Result<Self, ConfigError> {
        if value.len() > MAX_SSID_LEN {
            return Err(ConfigError::SsidTooLong);
        }

        let mut bytes = [0; MAX_SSID_LEN];
        bytes[..value.len()].copy_from_slice(value);
        Ok(Self {
            bytes,
            len: value.len() as u8,
        })
    }

    /// Returns the exact SSID bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// Returns the SSID as UTF-8 when it has a textual representation.
    pub fn as_str(&self) -> Option<&str> {
        str::from_utf8(self.as_bytes()).ok()
    }

    /// Returns the SSID length in bytes.
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns whether the SSID is empty, as hidden networks may be in scans.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl TryFrom<&str> for Ssid {
    type Error = ConfigError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value.as_bytes())
    }
}

impl fmt::Debug for Ssid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_str() {
            Some(value) => formatter.debug_tuple("Ssid").field(&value).finish(),
            None => formatter
                .debug_struct("Ssid")
                .field("len", &self.len)
                .field("encoding", &"non-UTF-8")
                .finish(),
        }
    }
}

/// WPA personal passphrase stored without allocation.
///
/// Debug formatting is deliberately redacted and the backing storage is
/// cleared when dropped. Production applications should still prefer runtime
/// provisioning over compile-time credentials.
#[derive(Eq, PartialEq)]
pub struct Passphrase {
    bytes: [u8; MAX_PASSPHRASE_LEN],
    len: u8,
}

impl Passphrase {
    /// Validates and stores an ASCII/UTF-8 WPA passphrase or 64-digit PSK.
    pub fn new(value: &str) -> Result<Self, ConfigError> {
        let bytes = value.as_bytes();
        let valid = (8..MAX_PASSPHRASE_LEN).contains(&bytes.len())
            || (bytes.len() == MAX_PASSPHRASE_LEN && bytes.iter().all(u8::is_ascii_hexdigit));
        if !valid {
            return Err(ConfigError::InvalidPassphrase);
        }

        let mut stored = [0; MAX_PASSPHRASE_LEN];
        stored[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            bytes: stored,
            len: bytes.len() as u8,
        })
    }

    /// Returns the validated passphrase for the platform driver.
    pub fn as_str(&self) -> &str {
        // Construction only accepts bytes from a valid `str`.
        str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }

    /// Returns the passphrase length without exposing its contents.
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns whether the passphrase is empty.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for Passphrase {
    fn drop(&mut self) {
        self.bytes.zeroize();
        self.len.zeroize();
    }
}

impl fmt::Debug for Passphrase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Passphrase(**REDACTED**)")
    }
}

/// Personal Wi-Fi authentication supported by the portable station contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Authentication {
    /// An unsecured network.
    Open,
    /// WPA2 Personal.
    Wpa2Personal,
    /// WPA3 Personal.
    Wpa3Personal,
    /// WPA2/WPA3 transition mode.
    Wpa2Wpa3Personal,
}

/// Validated station-mode configuration.
#[derive(Debug)]
pub struct StationConfig {
    ssid: Ssid,
    authentication: Authentication,
    passphrase: Option<Passphrase>,
}

impl StationConfig {
    /// Creates configuration for an open network.
    pub fn open(ssid: Ssid) -> Result<Self, ConfigError> {
        Self::validate_ssid(ssid)?;
        Ok(Self {
            ssid,
            authentication: Authentication::Open,
            passphrase: None,
        })
    }

    /// Creates configuration for a WPA personal network.
    pub fn personal(
        ssid: Ssid,
        passphrase: Passphrase,
        authentication: Authentication,
    ) -> Result<Self, ConfigError> {
        Self::validate_ssid(ssid)?;
        if authentication == Authentication::Open {
            return Err(ConfigError::InvalidPassphrase);
        }

        Ok(Self {
            ssid,
            authentication,
            passphrase: Some(passphrase),
        })
    }

    fn validate_ssid(ssid: Ssid) -> Result<(), ConfigError> {
        if ssid.is_empty() {
            Err(ConfigError::EmptySsid)
        } else {
            Ok(())
        }
    }

    /// Returns the target network SSID.
    pub const fn ssid(&self) -> &Ssid {
        &self.ssid
    }

    /// Returns the requested authentication mode.
    pub const fn authentication(&self) -> Authentication {
        self.authentication
    }

    /// Returns the passphrase when authentication requires one.
    pub const fn passphrase(&self) -> Option<&Passphrase> {
        self.passphrase.as_ref()
    }
}

/// Security advertised by a discovered or connected network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Security {
    /// No link-layer authentication.
    Open,
    /// Legacy WEP.
    Wep,
    /// WPA or WPA/WPA2 transition mode.
    Wpa,
    /// WPA2 Personal.
    Wpa2Personal,
    /// WPA3 Personal or WPA2/WPA3 transition mode.
    Wpa3Personal,
    /// Enterprise authentication.
    Enterprise,
    /// A driver-specific or unknown authentication mode.
    Unknown,
}

/// Portable information about a discovered access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPoint {
    /// Advertised SSID; it can be empty for a hidden network.
    pub ssid: Ssid,
    /// Basic service set identifier (radio MAC address).
    pub bssid: [u8; 6],
    /// Primary 2.4 GHz channel.
    pub channel: u8,
    /// Received signal strength in dBm.
    pub signal_strength_dbm: i8,
    /// Advertised security mode.
    pub security: Security,
}

/// Portable information returned after station association succeeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedStation {
    /// SSID of the associated network.
    pub ssid: Ssid,
    /// Basic service set identifier (radio MAC address).
    pub bssid: [u8; 6],
    /// Primary channel used by the association.
    pub channel: u8,
    /// Negotiated authentication mode.
    pub security: Security,
}

/// Aggregate scan information useful for health and telemetry reporting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanSummary {
    /// Number of access points observed.
    pub access_points: u16,
    /// Strongest received signal in dBm, when any AP was observed.
    pub strongest_signal_dbm: Option<i8>,
}

impl ScanSummary {
    /// Builds aggregate information without retaining network identities.
    pub fn from_access_points(access_points: &[AccessPoint]) -> Self {
        let strongest_signal_dbm = access_points
            .iter()
            .map(|access_point| access_point.signal_strength_dbm)
            .max();
        Self {
            access_points: u16::try_from(access_points.len()).unwrap_or(u16::MAX),
            strongest_signal_dbm,
        }
    }
}

/// High-level lifecycle state shared by platform Wi-Fi implementations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum WifiState {
    /// The radio is not initialized.
    #[default]
    Disabled,
    /// The driver is initializing.
    Starting,
    /// The radio is initialized and available for scanning or connection.
    Ready,
    /// A network scan is active.
    Scanning,
    /// Station association is active.
    Connecting,
    /// Station association succeeded; an IP address may still be pending.
    Connected,
    /// The station has disconnected after initialization.
    Disconnected,
    /// The driver encountered a terminal error.
    Failed,
}

/// Bounds for exponential station-reconnection delay and random jitter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectPolicy {
    initial_delay_ms: u32,
    maximum_delay_ms: u32,
    maximum_jitter_ms: u32,
}

impl ReconnectPolicy {
    /// Validates a reconnection policy.
    ///
    /// `maximum_delay_ms` bounds the complete delay, including jitter.
    pub const fn new(
        initial_delay_ms: u32,
        maximum_delay_ms: u32,
        maximum_jitter_ms: u32,
    ) -> Result<Self, ConfigError> {
        if initial_delay_ms == 0
            || initial_delay_ms > maximum_delay_ms
            || maximum_jitter_ms >= maximum_delay_ms
        {
            return Err(ConfigError::InvalidReconnectPolicy);
        }

        Ok(Self {
            initial_delay_ms,
            maximum_delay_ms,
            maximum_jitter_ms,
        })
    }

    /// Initial base delay before the first retry.
    pub const fn initial_delay_ms(&self) -> u32 {
        self.initial_delay_ms
    }

    /// Maximum complete delay, including jitter.
    pub const fn maximum_delay_ms(&self) -> u32 {
        self.maximum_delay_ms
    }

    /// Maximum random jitter added to a base delay.
    pub const fn maximum_jitter_ms(&self) -> u32 {
        self.maximum_jitter_ms
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay_ms: 1_000,
            maximum_delay_ms: 60_000,
            maximum_jitter_ms: 500,
        }
    }
}

/// Stateful exponential-backoff calculator for station reconnection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectBackoff {
    policy: ReconnectPolicy,
    next_base_delay_ms: u32,
    attempts: u32,
}

impl ReconnectBackoff {
    /// Creates a backoff calculator at its first-attempt state.
    pub const fn new(policy: ReconnectPolicy) -> Self {
        Self {
            next_base_delay_ms: policy.initial_delay_ms,
            policy,
            attempts: 0,
        }
    }

    /// Returns the next bounded delay using caller-supplied random entropy.
    pub fn next_delay_ms(&mut self, entropy: u32) -> u32 {
        let maximum_base = self
            .policy
            .maximum_delay_ms
            .saturating_sub(self.policy.maximum_jitter_ms);
        let base = self.next_base_delay_ms.min(maximum_base);
        let jitter = if self.policy.maximum_jitter_ms == 0 {
            0
        } else {
            entropy % (self.policy.maximum_jitter_ms + 1)
        };

        self.next_base_delay_ms = base.saturating_mul(2).min(maximum_base);
        self.attempts = self.attempts.saturating_add(1);
        base.saturating_add(jitter)
            .min(self.policy.maximum_delay_ms)
    }

    /// Resets the sequence after a successful association.
    pub fn reset(&mut self) {
        self.next_base_delay_ms = self.policy.initial_delay_ms;
        self.attempts = 0;
    }

    /// Number of retry delays issued since construction or reset.
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self::new(ReconnectPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::{
        AccessPoint, Authentication, ConfigError, Passphrase, ReconnectBackoff, ReconnectPolicy,
        ScanSummary, Security, Ssid, StationConfig,
    };

    #[test]
    fn validates_station_identity_and_secret_lengths() {
        let ssid = Ssid::try_from("factory-network").unwrap();
        let passphrase = Passphrase::new("commission-me").unwrap();
        let station =
            StationConfig::personal(ssid, passphrase, Authentication::Wpa2Wpa3Personal).unwrap();

        assert_eq!(station.ssid().as_str(), Some("factory-network"));
        assert_eq!(station.passphrase().map(Passphrase::len), Some(13));
        assert_eq!(
            Passphrase::new("short"),
            Err(ConfigError::InvalidPassphrase)
        );
        assert_eq!(
            StationConfig::open(Ssid::try_from("").unwrap()).unwrap_err(),
            ConfigError::EmptySsid
        );
    }

    #[test]
    fn secrets_are_redacted_from_debug_output() {
        let passphrase = Passphrase::new("do-not-log-this").unwrap();

        let rendered = std::format!("{passphrase:?}");
        assert_eq!(rendered, "Passphrase(**REDACTED**)");
        assert!(!rendered.contains("do-not-log-this"));
    }

    #[test]
    fn summarizes_scan_without_network_identity() {
        let access_points = [
            AccessPoint {
                ssid: Ssid::try_from("one").unwrap(),
                bssid: [0; 6],
                channel: 1,
                signal_strength_dbm: -72,
                security: Security::Wpa2Personal,
            },
            AccessPoint {
                ssid: Ssid::try_from("two").unwrap(),
                bssid: [1; 6],
                channel: 6,
                signal_strength_dbm: -48,
                security: Security::Wpa3Personal,
            },
        ];

        assert_eq!(
            ScanSummary::from_access_points(&access_points),
            ScanSummary {
                access_points: 2,
                strongest_signal_dbm: Some(-48),
            }
        );
    }

    #[test]
    fn reconnect_backoff_is_exponential_bounded_and_resettable() {
        let policy = ReconnectPolicy::new(1_000, 5_000, 500).unwrap();
        let mut backoff = ReconnectBackoff::new(policy);

        assert_eq!(backoff.next_delay_ms(250), 1_250);
        assert_eq!(backoff.next_delay_ms(0), 2_000);
        assert_eq!(backoff.next_delay_ms(500), 4_500);
        assert_eq!(backoff.next_delay_ms(500), 5_000);
        assert_eq!(backoff.attempts(), 4);

        backoff.reset();
        assert_eq!(backoff.attempts(), 0);
        assert_eq!(backoff.next_delay_ms(0), 1_000);
    }

    #[test]
    fn reconnect_policy_rejects_invalid_bounds() {
        assert_eq!(
            ReconnectPolicy::new(0, 1_000, 0),
            Err(ConfigError::InvalidReconnectPolicy)
        );
        assert_eq!(
            ReconnectPolicy::new(2_000, 1_000, 0),
            Err(ConfigError::InvalidReconnectPolicy)
        );
        assert_eq!(
            ReconnectPolicy::new(1_000, 1_000, 1_000),
            Err(ConfigError::InvalidReconnectPolicy)
        );
    }
}
