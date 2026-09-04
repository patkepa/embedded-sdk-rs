//! Internal-flash integration using the ecosystem NOR traits.

use embedded_sdk_storage::BlockingFlash;

/// Blocking ESP32-C6 flash wrapped for the SDK's asynchronous storage engine.
///
/// Flash erase and write operations remain synchronous and can stall radio and
/// executor progress. Firmware must measure those stalls before advertising
/// persistent storage as a supported board capability.
pub type InternalFlash<'d> = BlockingFlash<esp_storage::FlashStorage<'d>>;

/// Takes ownership of the chip's internal flash peripheral.
///
/// The resulting flash covers the complete physical device. Product firmware
/// must restrict it to the board-owned provisioning partition before creating
/// a key-value store.
#[must_use]
pub fn internal_flash(flash: esp_hal::peripherals::FLASH<'static>) -> InternalFlash<'static> {
    BlockingFlash::new(esp_storage::FlashStorage::new(flash))
}
