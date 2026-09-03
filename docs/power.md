# Battery Monitoring

## Portable API

`embedded-sdk-power` is an allocation-free, `no_std` crate. It keeps directly
observable battery facts separate from derived estimates:

- `BatteryMonitor` captures a `BatteryMeasurement`.
- `BatteryMeasurement` contains terminal voltage and an explicitly reported
  `ChargeState`.
- `VoltageCurve` validates an ordered set of voltage/percentage points and uses
  piecewise-linear interpolation.
- `StateOfChargeEstimate` records `EstimateBasis::TerminalVoltage`, preventing
  a voltage-derived percentage from being presented as a fuel-gauge reading.

Curves clamp below and above their endpoint percentages. They use integer
arithmetic and borrow caller-owned points, so estimating state of charge does
not allocate.

## Accuracy boundary

Terminal voltage changes with load, charging, temperature, chemistry, internal
resistance, and cell age. A voltage curve is most meaningful after the cell has
rested and when its profile has been characterized for the actual product.

Applications must not use a generic curve for safety cutoffs. Use conservative
voltage thresholds derived from the cell and regulator specifications. Products
that require accurate state of charge, current, state of health, or time to
empty need a suitable hardware fuel gauge.

## DFRobot Beetle ESP32-C6

The Beetle adapter uses ESP32-C6 ADC1 channel 0 with 11 dB attenuation and curve
calibration. It discards the first sample, averages eight readings, and applies
the board's 2:1 divider ratio. Both documented PCB revisions use the same
battery measurement circuit.

The TP4057 charge-status signal drives the on-board LED and is not available to
the MCU. The adapter therefore reports `ChargeState::Unknown`; it does not infer
charging state from voltage or voltage trend.

See the [Beetle ESP32-C6 Platform Guide](porting/dfrobot-beetle-esp32c6.md) for
building, flashing, and hardware validation.

## References

- [DFRobot Beetle ESP32-C6 documentation](https://wiki.dfrobot.com/dfr1117/)
- [DFRobot Beetle ESP32-C6 revision 1.0 and 1.1 schematics](https://dfimg.dfrobot.com/wiki/19405/DFR1117_beetle-esp32-c6_schematics_V1.1.zip)
- [Analog Devices: interpreting an open-circuit-voltage fuel gauge](https://www.analog.com/en/resources/technical-articles/interpreting-the-opencircuitvoltage-ocv-fuel-gauge-of-the-ds2786.html)
