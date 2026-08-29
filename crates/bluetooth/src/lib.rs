#![no_std]
#![forbid(unsafe_code)]
#![doc = "Platform-independent Bluetooth Low Energy identity and lifecycle types."]

use core::{fmt, str};

/// Maximum device-name length supported by the SDK's legacy GAP profile.
///
/// The bound leaves room in a 31-byte legacy advertising packet for the
/// discoverability flags and one 16-bit service UUID.
pub const MAX_DEVICE_NAME_LEN: usize = 22;

/// Error returned while constructing a Bluetooth value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// A discoverable peripheral must have a non-empty name.
    EmptyDeviceName,
    /// The name exceeds [`MAX_DEVICE_NAME_LEN`].
    DeviceNameTooLong,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyDeviceName => formatter.write_str("Bluetooth device name must not be empty"),
            Self::DeviceNameTooLong => {
                formatter.write_str("Bluetooth device name exceeds 22 bytes")
            }
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
    use super::{ConfigError, DeviceName, MAX_DEVICE_NAME_LEN, StaticRandomAddress};

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
}
