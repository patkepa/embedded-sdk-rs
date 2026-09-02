//! ESP32-C6 Bluetooth Low Energy controller adapter.

use core::fmt;

use embedded_sdk_bluetooth::{BluetoothState, StaticRandomAddress};
use esp_hal::efuse::{self, InterfaceMacAddress};
use esp_radio::ble::controller::{BleConnector as EspBleConnector, BleInitError};

/// ESP32-C6 Bluetooth controller configuration.
pub use esp_radio::ble::Config as ControllerConfig;
/// ESP32-C6 Bluetooth controller transmit-power setting.
pub use esp_radio::ble::TxPower as ControllerTxPower;

/// ESP32-C6 HCI transport consumed by a Bluetooth host stack.
pub type BluetoothConnector<'d> = EspBleConnector<'d>;

/// Error reported while initializing the ESP32-C6 BLE controller.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The Espressif controller rejected its configuration.
    Controller(BleInitError),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Controller(error) => write!(formatter, "ESP BLE controller init failed: {error}"),
        }
    }
}

impl core::error::Error for Error {}

impl From<BleInitError> for Error {
    fn from(value: BleInitError) -> Self {
        Self::Controller(value)
    }
}

/// Owned ESP32-C6 BLE controller transport.
///
/// A host stack such as TrouBLE consumes the connector returned by
/// [`Self::into_connector`] and owns advertising, connections, and GATT.
pub struct Esp32c6Bluetooth<'d> {
    connector: BluetoothConnector<'d>,
    state: BluetoothState,
}

impl<'d> Esp32c6Bluetooth<'d> {
    /// Initializes the BLE controller.
    ///
    /// A global allocator and the `esp-rtos` scheduler must already be running.
    pub fn new(device: esp_hal::peripherals::BT<'d>) -> Result<Self, Error> {
        Self::new_with_config(device, ControllerConfig::default())
    }

    /// Initializes the BLE controller with platform-specific radio settings.
    ///
    /// A global allocator and the `esp-rtos` scheduler must already be running.
    pub fn new_with_config(
        device: esp_hal::peripherals::BT<'d>,
        config: ControllerConfig,
    ) -> Result<Self, Error> {
        let connector = BluetoothConnector::new(device, config)?;
        Ok(Self {
            connector,
            state: BluetoothState::Ready,
        })
    }

    /// Returns the adapter's high-level lifecycle state.
    pub const fn state(&self) -> BluetoothState {
        self.state
    }

    /// Releases the HCI transport for use by a BLE host stack.
    pub fn into_connector(self) -> BluetoothConnector<'d> {
        self.connector
    }
}

/// Derives a stable random BLE identity from the chip's unique Bluetooth MAC.
pub fn static_random_address() -> StaticRandomAddress {
    let mac = efuse::interface_mac_address(InterfaceMacAddress::Bluetooth);
    let mut seed = [0; 6];
    seed.copy_from_slice(mac.as_bytes());
    StaticRandomAddress::from_seed(seed)
}
