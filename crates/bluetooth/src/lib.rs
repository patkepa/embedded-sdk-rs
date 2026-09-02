#![no_std]
#![forbid(unsafe_code)]
#![doc = "Platform-independent Bluetooth Low Energy identity and lifecycle types."]

use core::{fmt, str};

/// Maximum device-name length supported by the SDK's legacy GAP profile.
///
/// The bound leaves room in a 31-byte legacy advertising packet for the
/// discoverability flags and one 16-bit service UUID.
pub const MAX_DEVICE_NAME_LEN: usize = 22;

/// Bluetooth SIG company identifier used by Apple's iBeacon frame.
pub const IBEACON_COMPANY_IDENTIFIER: u16 = 0x004c;

/// Length of the manufacturer payload following the company identifier.
pub const IBEACON_PAYLOAD_LEN: usize = 23;

/// Error returned while constructing a Bluetooth value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// A discoverable peripheral must have a non-empty name.
    EmptyDeviceName,
    /// The name exceeds [`MAX_DEVICE_NAME_LEN`].
    DeviceNameTooLong,
    /// A beacon UUID is not in canonical 8-4-4-4-12 hexadecimal form.
    InvalidBeaconUuid,
    /// A legacy advertising interval must be between 20 ms and 10.24 seconds.
    AdvertisingIntervalOutOfRange,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDeviceName => formatter.write_str("Bluetooth device name must not be empty"),
            Self::DeviceNameTooLong => {
                formatter.write_str("Bluetooth device name exceeds 22 bytes")
            }
            Self::InvalidBeaconUuid => {
                formatter.write_str("beacon UUID must use canonical 8-4-4-4-12 hexadecimal form")
            }
            Self::AdvertisingIntervalOutOfRange => formatter
                .write_str("advertising interval must be between 20 and 10240 milliseconds"),
        }
    }
}

impl core::error::Error for ConfigError {}

/// A validated, allocation-free BLE GAP device name.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct DeviceName {
    bytes: [u8; MAX_DEVICE_NAME_LEN],
    len: u8,
}

impl DeviceName {
    /// Validates and stores a UTF-8 BLE device name.
    pub fn new(value: &str) -> Result<Self, ConfigError> {
        if value.is_empty() {
            return Err(ConfigError::EmptyDeviceName);
        }
        if value.len() > MAX_DEVICE_NAME_LEN {
            return Err(ConfigError::DeviceNameTooLong);
        }

        let mut bytes = [0; MAX_DEVICE_NAME_LEN];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Ok(Self {
            bytes,
            len: value.len() as u8,
        })
    }

    /// Returns the validated name.
    pub fn as_str(&self) -> &str {
        // Construction only accepts bytes copied from a valid `str`.
        str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }

    /// Returns the name length in bytes.
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns whether the name is empty.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl TryFrom<&str> for DeviceName {
    type Error = ConfigError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Debug for DeviceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DeviceName")
            .field(&self.as_str())
            .finish()
    }
}

/// A 128-bit beacon proximity UUID in network byte order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BeaconUuid([u8; 16]);

impl BeaconUuid {
    /// Parses a canonical UUID such as `7a1e1000-4c2a-4f66-a1d4-3f55b55a1000`.
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        let source = value.as_bytes();
        if source.len() != 36
            || source[8] != b'-'
            || source[13] != b'-'
            || source[18] != b'-'
            || source[23] != b'-'
        {
            return Err(ConfigError::InvalidBeaconUuid);
        }

        let mut bytes = [0_u8; 16];
        let mut source_index = 0;
        let mut byte_index = 0;
        while source_index < source.len() {
            if matches!(source_index, 8 | 13 | 18 | 23) {
                source_index += 1;
                continue;
            }

            let high = decode_hex(source[source_index])?;
            let low = decode_hex(source[source_index + 1])?;
            bytes[byte_index] = (high << 4) | low;
            source_index += 2;
            byte_index += 1;
        }

        Ok(Self(bytes))
    }

    /// Creates a beacon UUID from its network-order bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Returns the UUID in network byte order.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl TryFrom<&str> for BeaconUuid {
    type Error = ConfigError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl fmt::Display for BeaconUuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                formatter.write_str("-")?;
            }
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

const fn decode_hex(value: u8) -> Result<u8, ConfigError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(ConfigError::InvalidBeaconUuid),
    }
}

/// Validated interval for legacy BLE advertising.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingInterval(u16);

impl AdvertisingInterval {
    /// Minimum interval permitted by legacy undirected advertising.
    pub const MIN_MILLIS: u16 = 20;
    /// Maximum interval permitted by legacy advertising.
    pub const MAX_MILLIS: u16 = 10_240;

    /// Validates an interval expressed in milliseconds.
    pub const fn from_millis(milliseconds: u16) -> Result<Self, ConfigError> {
        if milliseconds < Self::MIN_MILLIS || milliseconds > Self::MAX_MILLIS {
            return Err(ConfigError::AdvertisingIntervalOutOfRange);
        }
        Ok(Self(milliseconds))
    }

    /// Returns the interval in milliseconds.
    pub const fn as_millis(self) -> u16 {
        self.0
    }
}

impl Default for AdvertisingInterval {
    fn default() -> Self {
        Self(250)
    }
}

/// An iBeacon-compatible proximity frame.
///
/// `measured_power` is the calibrated RSSI expected one metre from the beacon;
/// it is not the controller's radio transmit-power setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IBeacon {
    uuid: BeaconUuid,
    major: u16,
    minor: u16,
    measured_power: i8,
}

impl IBeacon {
    /// Creates an iBeacon proximity frame.
    pub const fn new(uuid: BeaconUuid, major: u16, minor: u16, measured_power: i8) -> Self {
        Self {
            uuid,
            major,
            minor,
            measured_power,
        }
    }

    /// Returns the proximity UUID.
    pub const fn uuid(self) -> BeaconUuid {
        self.uuid
    }

    /// Returns the deployment-level region identifier.
    pub const fn major(self) -> u16 {
        self.major
    }

    /// Returns the individual beacon identifier within a major region.
    pub const fn minor(self) -> u16 {
        self.minor
    }

    /// Returns the calibrated one-metre RSSI value.
    pub const fn measured_power(self) -> i8 {
        self.measured_power
    }

    /// Encodes the bytes carried after the Apple company identifier.
    pub const fn manufacturer_payload(self) -> [u8; IBEACON_PAYLOAD_LEN] {
        let uuid = self.uuid.0;
        let major = self.major.to_be_bytes();
        let minor = self.minor.to_be_bytes();
        [
            0x02,
            0x15,
            uuid[0],
            uuid[1],
            uuid[2],
            uuid[3],
            uuid[4],
            uuid[5],
            uuid[6],
            uuid[7],
            uuid[8],
            uuid[9],
            uuid[10],
            uuid[11],
            uuid[12],
            uuid[13],
            uuid[14],
            uuid[15],
            major[0],
            major[1],
            minor[0],
            minor[1],
            self.measured_power as u8,
        ]
    }
}

/// A stable random BLE device address in canonical, most-significant-byte-first order.
///
/// Static random addresses have their two most significant bits set. The SDK
/// derives one from a platform-unique radio address so a board does not ship
/// with a shared example identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StaticRandomAddress([u8; 6]);

impl StaticRandomAddress {
    /// Derives a valid static random address from six stable seed bytes.
    pub const fn from_seed(mut seed: [u8; 6]) -> Self {
        seed[0] |= 0b1100_0000;

        // The random portion must not be all zeroes or all ones. Preserve the
        // seed in normal cases and make either degenerate input valid.
        let all_zeroes = seed[0] == 0b1100_0000
            && seed[1] == 0
            && seed[2] == 0
            && seed[3] == 0
            && seed[4] == 0
            && seed[5] == 0;
        let all_ones = seed[0] == 0xff
            && seed[1] == 0xff
            && seed[2] == 0xff
            && seed[3] == 0xff
            && seed[4] == 0xff
            && seed[5] == 0xff;
        if all_zeroes || all_ones {
            seed[5] ^= 1;
        }

        Self(seed)
    }

    /// Returns the address in canonical display/network order.
    pub const fn as_bytes(&self) -> &[u8; 6] {
        &self.0
    }

    /// Returns the address in the little-endian byte order used by Bluetooth HCI.
    pub const fn to_hci_bytes(self) -> [u8; 6] {
        [
            self.0[5], self.0[4], self.0[3], self.0[2], self.0[1], self.0[0],
        ]
    }
}

impl fmt::Display for StaticRandomAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

/// Validated settings for a discoverable BLE peripheral.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConfig {
    name: DeviceName,
    address: StaticRandomAddress,
}

impl PeripheralConfig {
    /// Creates peripheral settings from validated identity values.
    pub const fn new(name: DeviceName, address: StaticRandomAddress) -> Self {
        Self { name, address }
    }

    /// Returns the advertised GAP name.
    pub const fn name(&self) -> &DeviceName {
        &self.name
    }

    /// Returns the stable random device address.
    pub const fn address(&self) -> StaticRandomAddress {
        self.address
    }
}

/// High-level lifecycle state shared by platform BLE implementations.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum BluetoothState {
    /// The controller is not initialized.
    #[default]
    Disabled,
    /// The controller and host are ready.
    Ready,
    /// The peripheral is advertising.
    Advertising,
    /// A central is connected.
    Connected,
    /// The host or controller encountered a terminal error.
    Failed,
}

#[cfg(test)]
mod tests {
    extern crate std;

    use self::std::string::ToString;
    use super::{
        AdvertisingInterval, BeaconUuid, ConfigError, DeviceName, IBeacon, MAX_DEVICE_NAME_LEN,
        StaticRandomAddress,
    };

    #[test]
    fn device_name_is_bounded() {
        assert_eq!(DeviceName::new(""), Err(ConfigError::EmptyDeviceName));
        assert_eq!(
            DeviceName::new("12345678901234567890123"),
            Err(ConfigError::DeviceNameTooLong)
        );

        let name = DeviceName::new("XIAO ESP32C6 SDK").unwrap();
        assert_eq!(name.as_str(), "XIAO ESP32C6 SDK");
        assert!(name.len() <= MAX_DEVICE_NAME_LEN);
    }

    #[test]
    fn static_random_address_sets_type_bits_and_hci_order() {
        let address = StaticRandomAddress::from_seed([0x10, 0x20, 0x30, 0x40, 0x50, 0x60]);

        assert_eq!(address.as_bytes(), &[0xd0, 0x20, 0x30, 0x40, 0x50, 0x60]);
        assert_eq!(address.to_hci_bytes(), [0x60, 0x50, 0x40, 0x30, 0x20, 0xd0]);
    }

    #[test]
    fn degenerate_static_random_addresses_are_avoided() {
        let zeroes = StaticRandomAddress::from_seed([0; 6]);
        let ones = StaticRandomAddress::from_seed([0xff; 6]);

        assert_eq!(zeroes.as_bytes(), &[0xc0, 0, 0, 0, 0, 1]);
        assert_eq!(ones.as_bytes(), &[0xff, 0xff, 0xff, 0xff, 0xff, 0xfe]);
    }

    #[test]
    fn beacon_uuid_parses_and_formats_canonical_text() {
        let uuid = BeaconUuid::parse("7A1E1000-4C2A-4F66-A1D4-3F55B55A1000").unwrap();

        assert_eq!(
            uuid.as_bytes(),
            &[
                0x7a, 0x1e, 0x10, 0x00, 0x4c, 0x2a, 0x4f, 0x66, 0xa1, 0xd4, 0x3f, 0x55, 0xb5, 0x5a,
                0x10, 0x00,
            ]
        );
        assert_eq!(uuid.to_string(), "7a1e1000-4c2a-4f66-a1d4-3f55b55a1000");
        assert_eq!(
            BeaconUuid::parse("7a1e10004c2a4f66a1d43f55b55a1000"),
            Err(ConfigError::InvalidBeaconUuid)
        );
    }

    #[test]
    fn ibeacon_payload_uses_network_byte_order() {
        let uuid = BeaconUuid::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
        let payload = IBeacon::new(uuid, 0x1234, 0xabcd, -59).manufacturer_payload();

        assert_eq!(&payload[..2], &[0x02, 0x15]);
        assert_eq!(&payload[2..18], uuid.as_bytes());
        assert_eq!(&payload[18..22], &[0x12, 0x34, 0xab, 0xcd]);
        assert_eq!(payload[22], 0xc5);
    }

    #[test]
    fn advertising_interval_enforces_legacy_bounds() {
        assert_eq!(
            AdvertisingInterval::from_millis(20).unwrap().as_millis(),
            20
        );
        assert_eq!(
            AdvertisingInterval::from_millis(10_240)
                .unwrap()
                .as_millis(),
            10_240
        );
        assert_eq!(
            AdvertisingInterval::from_millis(19),
            Err(ConfigError::AdvertisingIntervalOutOfRange)
        );
        assert_eq!(
            AdvertisingInterval::from_millis(10_241),
            Err(ConfigError::AdvertisingIntervalOutOfRange)
        );
    }
}
