use esp_hal::{
    gpio::{Level, Output, OutputConfig},
    peripherals::GPIO15,
};

/// Software-controlled blue status LED fitted to the Beetle ESP32-C6.
///
/// The LED is wired active-high from GPIO15 through a 2.7 kOhm resistor.
pub struct BeetleStatusLed<'d> {
    output: Output<'d>,
}

impl<'d> BeetleStatusLed<'d> {
    /// Configures GPIO15 as an output with the LED initially off.
    #[must_use]
    pub fn new(pin: GPIO15<'d>) -> Self {
        Self {
            output: Output::new(pin, Level::Low, OutputConfig::default()),
        }
    }

    /// Illuminates the status LED.
    pub fn on(&mut self) {
        self.output.set_high();
    }

    /// Extinguishes the status LED.
    pub fn off(&mut self) {
        self.output.set_low();
    }
}
