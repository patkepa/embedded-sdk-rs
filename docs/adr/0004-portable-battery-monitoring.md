# ADR 0004: Separate Battery Measurements from State-of-Charge Estimates

## Status

Accepted

## Context

Boards expose substantially different battery information. Some provide only
an ADC-connected voltage divider, while fuel-gauge ICs can additionally report
current, temperature, learned capacity, state of health, and state of charge.
Treating a voltage-derived percentage as a measured fact would hide important
accuracy limitations caused by load, charging, temperature, chemistry, and
cell age.

The DFRobot Beetle ESP32-C6 revisions 1.0 and 1.1 connect the battery to GPIO0
through a 1 MOhm / 1 MOhm divider with a 100 nF filter capacitor. Its TP4057
charger drives an indicator LED, but its status outputs are not routed to the
ESP32-C6.

## Decision

The portable `embedded-sdk-power` crate separates direct observations from
derived estimates:

- `BatteryMonitor` produces a `BatteryMeasurement` containing terminal voltage
  and the charging state actually available from hardware.
- `VoltageCurve` produces a `StateOfChargeEstimate` with an explicit
  `TerminalVoltage` basis.
- A board that cannot observe charger status reports `ChargeState::Unknown`.
- Voltage profiles are supplied by the product and are not embedded in a board
  package. This keeps cell chemistry and application load policy out of board
  wiring definitions.
- `Capabilities::BATTERY_VOLTAGE_MONITORING` means firmware can observe battery
  terminal voltage. It does not imply charge-state detection or fuel-gauge
  accuracy.

The DFRobot board package owns its divider, ADC pin, sample count, and
ESP32-C6-specific calibrated ADC adapter. Its reference firmware owns the
generic demonstration profile and reporting interval.

## Consequences

Applications can consume a consistent measurement API without confusing an
estimated percentage with a measured quantity. A future coulomb-counting or
model-based fuel-gauge driver can implement `BatteryMonitor` and add richer
portable contracts without changing the Beetle implementation.

The Beetle firmware cannot reliably report whether the battery is charging,
full, absent, or discharging. Its generic voltage profile is suitable for
bring-up only; production firmware must characterize the selected cell under
representative loads and temperatures.
