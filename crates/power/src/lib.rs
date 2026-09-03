#![no_std]
#![forbid(unsafe_code)]
#![doc = "Portable battery measurement and state-of-charge estimation contracts."]

use core::fmt;

/// Battery terminal voltage expressed in millivolts.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Millivolts(u32);

impl Millivolts {
    /// Creates a voltage value from millivolts.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the voltage in millivolts.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// A validated percentage from zero through one hundred.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Percentage(u8);

impl Percentage {
    /// Creates a percentage when `value` is at most one hundred.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value <= 100 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the percentage as an integer from zero through one hundred.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Charging state reported by the available hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ChargeState {
    /// The hardware cannot determine charging state.
    Unknown,
    /// Energy is being added to the battery.
    Charging,
    /// The charger reports that charging has completed.
    Full,
    /// The battery is supplying the system.
    Discharging,
    /// The battery is present but neither charging nor discharging.
    Idle,
}

/// One point in a voltage-to-state-of-charge profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoltagePoint {
    millivolts: Millivolts,
    percentage: u8,
}

impl VoltagePoint {
    /// Creates a profile point.
    ///
    /// [`VoltageCurve::new`] validates the percentage and ordering when the
    /// point is added to a curve.
    #[must_use]
    pub const fn new(millivolts: u32, percentage: u8) -> Self {
        Self {
            millivolts: Millivolts::new(millivolts),
            percentage,
        }
    }

    /// Returns the point voltage.
    #[must_use]
    pub const fn millivolts(self) -> Millivolts {
        self.millivolts
    }

    /// Returns the point percentage before curve validation.
    #[must_use]
    pub const fn percentage(self) -> u8 {
        self.percentage
    }
}

/// Error returned while validating a voltage-to-state-of-charge curve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CurveError {
    /// At least two points are required for interpolation.
    TooFewPoints,
    /// A point contains a percentage greater than one hundred.
    InvalidPercentage {
        /// Index of the invalid point.
        index: usize,
    },
    /// Voltages must be strictly increasing.
    VoltageNotIncreasing {
        /// Index of the invalid point.
        index: usize,
    },
    /// Percentages must not decrease as voltage increases.
    PercentageDecreases {
        /// Index of the invalid point.
        index: usize,
    },
}

impl fmt::Display for CurveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewPoints => {
                formatter.write_str("a voltage curve requires at least two points")
            }
            Self::InvalidPercentage { index } => {
                write!(formatter, "voltage curve point {index} exceeds 100 percent")
            }
            Self::VoltageNotIncreasing { index } => write!(
                formatter,
                "voltage curve point {index} does not increase in voltage"
            ),
            Self::PercentageDecreases { index } => write!(
                formatter,
                "voltage curve point {index} decreases in percentage"
            ),
        }
    }
}

impl core::error::Error for CurveError {}

/// A validated, allocation-free piecewise-linear voltage profile.
///
/// The profile estimates state of charge from terminal voltage. Applications
/// should supply points characterized for their cell, load, and temperature.
/// A voltage-derived estimate is not equivalent to a fuel-gauge measurement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoltageCurve<'a> {
    points: &'a [VoltagePoint],
}

impl<'a> VoltageCurve<'a> {
    /// Validates and borrows an ordered set of profile points.
    pub const fn new(points: &'a [VoltagePoint]) -> Result<Self, CurveError> {
        if points.len() < 2 {
            return Err(CurveError::TooFewPoints);
        }

        let mut index = 0;
        while index < points.len() {
            if points[index].percentage > 100 {
                return Err(CurveError::InvalidPercentage { index });
            }
            if index > 0 {
                if points[index].millivolts.0 <= points[index - 1].millivolts.0 {
                    return Err(CurveError::VoltageNotIncreasing { index });
                }
                if points[index].percentage < points[index - 1].percentage {
                    return Err(CurveError::PercentageDecreases { index });
                }
            }
            index += 1;
        }

        Ok(Self { points })
    }

    /// Estimates state of charge, clamping values outside the curve endpoints.
    #[must_use]
    pub fn estimate(self, voltage: Millivolts) -> StateOfChargeEstimate {
        let first = self.points[0];
        if voltage <= first.millivolts {
            return StateOfChargeEstimate::terminal_voltage(first.percentage);
        }

        let mut index = 1;
        while index < self.points.len() {
            let upper = self.points[index];
            if voltage <= upper.millivolts {
                let lower = self.points[index - 1];
                let voltage_span = upper.millivolts.0 - lower.millivolts.0;
                let voltage_offset = voltage.0 - lower.millivolts.0;
                let voltage_span = u64::from(voltage_span);
                let voltage_offset = u64::from(voltage_offset);
                let percentage_span = u64::from(upper.percentage - lower.percentage);
                let interpolated = u64::from(lower.percentage)
                    + (voltage_offset * percentage_span + voltage_span / 2) / voltage_span;
                return StateOfChargeEstimate::terminal_voltage(interpolated as u8);
            }
            index += 1;
        }

        StateOfChargeEstimate::terminal_voltage(self.points[self.points.len() - 1].percentage)
    }

    /// Returns the validated profile points.
    #[must_use]
    pub const fn points(self) -> &'a [VoltagePoint] {
        self.points
    }
}

/// The physical input used to estimate state of charge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EstimateBasis {
    /// The estimate is derived only from battery terminal voltage.
    TerminalVoltage,
}

/// An explicitly approximate state-of-charge result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StateOfChargeEstimate {
    percentage: Percentage,
    basis: EstimateBasis,
}

impl StateOfChargeEstimate {
    const fn terminal_voltage(percentage: u8) -> Self {
        Self {
            percentage: Percentage(percentage),
            basis: EstimateBasis::TerminalVoltage,
        }
    }

    /// Returns the estimated percentage.
    #[must_use]
    pub const fn percentage(self) -> Percentage {
        self.percentage
    }

    /// Returns the measurement basis for this estimate.
    #[must_use]
    pub const fn basis(self) -> EstimateBasis {
        self.basis
    }
}

/// A snapshot of facts directly observable by a battery monitor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatteryMeasurement {
    voltage: Millivolts,
    charge_state: ChargeState,
}

impl BatteryMeasurement {
    /// Creates a battery measurement.
    #[must_use]
    pub const fn new(voltage: Millivolts, charge_state: ChargeState) -> Self {
        Self {
            voltage,
            charge_state,
        }
    }

    /// Returns measured battery terminal voltage.
    #[must_use]
    pub const fn voltage(self) -> Millivolts {
        self.voltage
    }

    /// Returns the charging state available from the monitor.
    #[must_use]
    pub const fn charge_state(self) -> ChargeState {
        self.charge_state
    }
}

/// A source of battery measurements.
pub trait BatteryMonitor {
    /// Monitor-specific measurement failure.
    type Error;

    /// Captures one battery measurement.
    fn measure(&mut self) -> Result<BatteryMeasurement, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::{CurveError, EstimateBasis, Millivolts, VoltageCurve, VoltagePoint};

    const POINTS: [VoltagePoint; 3] = [
        VoltagePoint::new(3_300, 0),
        VoltagePoint::new(3_700, 20),
        VoltagePoint::new(4_200, 100),
    ];

    #[test]
    fn voltage_curve_interpolates_and_clamps() {
        let curve = VoltageCurve::new(&POINTS).unwrap();

        assert_eq!(curve.estimate(Millivolts::new(3_000)).percentage().get(), 0);
        assert_eq!(
            curve.estimate(Millivolts::new(3_500)).percentage().get(),
            10
        );
        assert_eq!(
            curve.estimate(Millivolts::new(3_950)).percentage().get(),
            60
        );
        assert_eq!(
            curve.estimate(Millivolts::new(4_300)).percentage().get(),
            100
        );
        assert_eq!(
            curve.estimate(Millivolts::new(3_700)).basis(),
            EstimateBasis::TerminalVoltage
        );
    }

    #[test]
    fn voltage_curve_rejects_invalid_profiles() {
        assert_eq!(VoltageCurve::new(&[]), Err(CurveError::TooFewPoints));
        assert_eq!(
            VoltageCurve::new(&[VoltagePoint::new(3_300, 0), VoltagePoint::new(3_200, 20),]),
            Err(CurveError::VoltageNotIncreasing { index: 1 })
        );
        assert_eq!(
            VoltageCurve::new(&[VoltagePoint::new(3_300, 101), VoltagePoint::new(4_200, 100),]),
            Err(CurveError::InvalidPercentage { index: 0 })
        );
        assert_eq!(
            VoltageCurve::new(&[VoltagePoint::new(3_300, 20), VoltagePoint::new(4_200, 10),]),
            Err(CurveError::PercentageDecreases { index: 1 })
        );
    }

    #[test]
    fn interpolation_handles_the_full_voltage_type_range() {
        let points = [VoltagePoint::new(0, 0), VoltagePoint::new(u32::MAX, 100)];
        let estimate = VoltageCurve::new(&points)
            .unwrap()
            .estimate(Millivolts::new(u32::MAX / 2));

        assert_eq!(estimate.percentage().get(), 50);
    }
}
