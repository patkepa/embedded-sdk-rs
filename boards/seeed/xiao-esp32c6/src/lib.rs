#![no_std]
#![forbid(unsafe_code)]
#![doc = "Board support metadata for the Seeed Studio XIAO ESP32C6."]

use embedded_sdk_core::{BoardSupport, HardwareDescriptor};
use embedded_sdk_platform_esp32c6 as platform;

/// Stable board support type for the Seeed Studio XIAO ESP32C6.
pub struct XiaoEsp32c6;

impl BoardSupport for XiaoEsp32c6 {
    const HARDWARE: HardwareDescriptor = HardwareDescriptor {
        board: "xiao-esp32c6",
        chip: platform::CHIP,
        manufacturer: "Seeed Studio",
        architecture: platform::ARCHITECTURE,
        capabilities: platform::CAPABILITIES,
    };
}

/// Static descriptor for the XIAO ESP32C6.
pub const HARDWARE: HardwareDescriptor = XiaoEsp32c6::HARDWARE;

/// Default BLE GAP name used by the reference firmware.
pub const BLUETOOTH_DEVICE_NAME: &str = "XIAO ESP32C6 SDK";

/// Version of the checked-in 4 MiB flash partition layout.
pub const PARTITION_LAYOUT_VERSION: u16 = 1;
/// Physical internal flash capacity fitted to the supported XIAO ESP32C6.
pub const FLASH_CAPACITY_BYTES: u32 = 0x40_0000;
/// ESP-IDF partition table offset used by the bootloader and application.
pub const PARTITION_TABLE_OFFSET: u32 = 0x8000;
/// Start of the factory application partition.
pub const FACTORY_APP_OFFSET: u32 = 0x1_0000;
/// Capacity of each factory and future OTA application slot.
pub const APP_PARTITION_BYTES: u32 = 0x14_0000;
/// Start of the board-owned provisioning storage partition.
pub const PROVISIONING_STORAGE_OFFSET: u32 = 0x3d_0000;
/// Capacity of the board-owned provisioning storage partition.
pub const PROVISIONING_STORAGE_BYTES: u32 = 0x2_0000;
/// Physical erase sector used to align every data partition.
pub const FLASH_ERASE_BYTES: u32 = 0x1000;

/// On-board yellow user LED.
pub const USER_LED_GPIO: u8 = 15;
/// Boot button, active low when pressed.
pub const BOOT_BUTTON_GPIO: u8 = 9;
/// D0 / A0.
pub const D0_GPIO: u8 = 0;
/// D1 / A1.
pub const D1_GPIO: u8 = 1;
/// D2 / A2.
pub const D2_GPIO: u8 = 2;
/// D3 / general-purpose digital pin.
pub const D3_GPIO: u8 = 21;
/// D4 / I2C SDA.
pub const I2C_SDA_GPIO: u8 = 22;
/// D5 / I2C SCL.
pub const I2C_SCL_GPIO: u8 = 23;
/// D6 / UART TX.
pub const UART_TX_GPIO: u8 = 16;
/// D7 / UART RX.
pub const UART_RX_GPIO: u8 = 17;
/// D8 / SPI clock.
pub const SPI_SCK_GPIO: u8 = 19;
/// D9 / SPI MISO.
pub const SPI_MISO_GPIO: u8 = 20;
/// D10 / SPI MOSI.
pub const SPI_MOSI_GPIO: u8 = 18;
/// Enables RF-switch control when driven low.
pub const RF_SWITCH_ENABLE_GPIO: u8 = 3;
/// Selects the on-board antenna when low and the U.FL antenna when high.
pub const RF_SWITCH_SELECT_GPIO: u8 = 14;

#[cfg(test)]
mod tests {
    use embedded_sdk_core::Capabilities;

    use super::{
        APP_PARTITION_BYTES, BLUETOOTH_DEVICE_NAME, FACTORY_APP_OFFSET, FLASH_CAPACITY_BYTES,
        FLASH_ERASE_BYTES, HARDWARE, I2C_SCL_GPIO, I2C_SDA_GPIO, PROVISIONING_STORAGE_BYTES,
        PROVISIONING_STORAGE_OFFSET, USER_LED_GPIO,
    };

    #[test]
    fn board_exposes_esp32c6_radios() {
        assert!(HARDWARE.capabilities.contains(Capabilities::WIFI));
        assert!(HARDWARE.capabilities.contains(Capabilities::BLE));
        assert!(HARDWARE.capabilities.contains(Capabilities::IEEE_802_15_4));
    }

    #[test]
    fn board_pin_contract_matches_xiao() {
        assert_eq!(USER_LED_GPIO, 15);
        assert_eq!((I2C_SDA_GPIO, I2C_SCL_GPIO), (22, 23));
    }

    #[test]
    fn board_bluetooth_name_fits_legacy_gap_data() {
        assert!(BLUETOOTH_DEVICE_NAME.len() <= embedded_sdk_bluetooth::MAX_DEVICE_NAME_LEN);
    }

    #[test]
    fn partition_contract_is_aligned_and_inside_the_supported_flash() {
        assert_eq!(FACTORY_APP_OFFSET % 0x1_0000, 0);
        assert_eq!(APP_PARTITION_BYTES % FLASH_ERASE_BYTES, 0);
        assert_eq!(PROVISIONING_STORAGE_OFFSET % FLASH_ERASE_BYTES, 0);
        assert_eq!(PROVISIONING_STORAGE_BYTES % FLASH_ERASE_BYTES, 0);
        const {
            assert!(
                PROVISIONING_STORAGE_OFFSET + PROVISIONING_STORAGE_BYTES <= FLASH_CAPACITY_BYTES
            );
        }
    }
}
