//! ESP32-C6 hardware random-number integration.

use core::convert::Infallible;

use embedded_sdk_security::SecureRandom;
use esp_hal::rng::Rng;

/// ESP32-C6 random source for portable security consumers.
///
/// Espressif documents the output as true random only while an entropy source
/// such as the RF subsystem or supported ADC entropy source is active. Cloud
/// supervisors must construct and use this adapter only after Wi-Fi or
/// Bluetooth has started, and must stop creating TLS sessions when that
/// condition no longer holds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Esp32c6HardwareRandom {
    inner: Rng,
}

impl Esp32c6HardwareRandom {
    /// Creates an adapter after the caller has established the RF entropy
    /// precondition described on this type.
    #[must_use]
    pub fn after_radio_started() -> Self {
        Self { inner: Rng::new() }
    }
}

impl SecureRandom for Esp32c6HardwareRandom {
    type Error = Infallible;

    fn fill_bytes(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
        self.inner.read(output);
        Ok(())
    }
}

/// Fills the custom `getrandom` 0.2 backend used by the experimental TLS
/// provider.
///
/// Register this function exactly once from the final firmware binary with
/// `getrandom::register_custom_getrandom!`. The same RF entropy precondition as
/// [`Esp32c6HardwareRandom`] applies; the TLS supervisor must not start a
/// handshake before radio initialization.
pub fn fill_getrandom_after_radio_started(output: &mut [u8]) -> Result<(), getrandom::Error> {
    Rng::new().read(output);
    Ok(())
}
