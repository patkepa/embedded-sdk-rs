use core::fmt;

use embedded_sdk_power::{BatteryMeasurement, BatteryMonitor, ChargeState, Millivolts};
use esp_hal::{
    Blocking,
    analog::adc::{Adc, AdcCalCurve, AdcConfig, AdcPin, Attenuation},
    peripherals::{ADC1, GPIO0},
};

use crate::{BATTERY_DIVIDER_DENOMINATOR, BATTERY_DIVIDER_NUMERATOR, BATTERY_SAMPLE_COUNT};

/// Failure returned while sampling the Beetle battery ADC input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryMonitorError {
    /// The ESP32-C6 ADC did not complete a one-shot conversion.
    Adc,
}

impl fmt::Display for BatteryMonitorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Adc => formatter.write_str("ESP32-C6 battery ADC conversion failed"),
        }
    }
}

impl core::error::Error for BatteryMonitorError {}

/// Battery-voltage monitor for DFRobot Beetle ESP32-C6 revisions 1.0 and 1.1.
///
/// GPIO0 is connected to the midpoint of a 1 MOhm / 1 MOhm divider across the
/// battery. The TP4057 status outputs are not connected to the MCU, so every
/// measurement reports [`ChargeState::Unknown`].
pub struct BeetleBatteryMonitor<'d> {
    adc: Adc<'d, ADC1<'d>, Blocking>,
    pin: AdcPin<GPIO0<'d>, ADC1<'d>, AdcCalCurve<ADC1<'d>>>,
}

impl<'d> BeetleBatteryMonitor<'d> {
    /// Configures ADC1 and GPIO0 for calibrated battery-voltage measurements.
    #[must_use]
    pub fn new(adc: ADC1<'d>, pin: GPIO0<'d>) -> Self {
        let mut config = AdcConfig::new();
        let pin = config.enable_pin_with_cal::<_, AdcCalCurve<_>>(pin, Attenuation::_11dB);
        let adc = Adc::new(adc, config);
        Self { adc, pin }
    }

    fn read_divider_millivolts(&mut self) -> Result<u16, BatteryMonitorError> {
        nb::block!(self.adc.read_oneshot(&mut self.pin)).map_err(|()| BatteryMonitorError::Adc)
    }
}

impl BatteryMonitor for BeetleBatteryMonitor<'_> {
    type Error = BatteryMonitorError;

    fn measure(&mut self) -> Result<BatteryMeasurement, Self::Error> {
        // The divider has a 100 nF filter capacitor. Discard one conversion so
        // ADC channel acquisition does not bias the averaged result.
        let _ = self.read_divider_millivolts()?;

        let mut total = 0_u32;
        for _ in 0..BATTERY_SAMPLE_COUNT {
            total += u32::from(self.read_divider_millivolts()?);
        }
        let divider_mv = total / BATTERY_SAMPLE_COUNT as u32;
        let battery_mv = divider_mv * BATTERY_DIVIDER_NUMERATOR / BATTERY_DIVIDER_DENOMINATOR;

        Ok(BatteryMeasurement::new(
            Millivolts::new(battery_mv),
            ChargeState::Unknown,
        ))
    }
}
