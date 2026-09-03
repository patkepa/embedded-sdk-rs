# DFRobot Beetle ESP32-C6 Platform Guide

## Support status

The DFRobot Beetle ESP32-C6 battery-monitoring target is Tier 3. Its host tests
and firmware build are continuous-integration gates; calibrated voltage accuracy
still requires validation on both board revisions with real cells and lab
equipment.

The implementation supports:

- Bare-metal ESP32-C6 startup with Embassy.
- Battery terminal-voltage measurement through GPIO0 / ADC1 channel 0.
- ESP32-C6 curve-calibrated ADC conversion at 11 dB attenuation.
- One discarded settling conversion followed by an eight-sample average.
- A product-owned piecewise-linear state-of-charge estimate.
- Serial reporting every 30 seconds.

It does not support charging-state detection, battery-presence detection,
current measurement, state of health, or time-to-empty calculation. The TP4057
status signals are not connected to the MCU on this board.

## Build

```sh
cargo xtask build-beetle-esp32c6-battery
```

The resulting ELF is written to:

```text
target/riscv32imac-unknown-none-elf/release/beetle-esp32c6-battery
```

## Flash and monitor

Connect the board over USB and run:

```sh
cargo xtask run-beetle-esp32c6-battery
```

Example output:

```text
embedded-sdk boot: board=beetle-esp32c6, chip=esp32c6
battery state-of-charge is a terminal-voltage estimate; charge status is unavailable
battery: voltage_mv=3870, estimated_percent=60, charge_state=Unknown
```

The firmware profile is intentionally generic. Replace
`BATTERY_PROFILE_POINTS` with characterization data for the selected single-cell
Li-ion or LiPo battery and representative product load.

## Hardware validation

Before promoting the board beyond Tier 3:

1. Validate PCB revisions 1.0 and 1.1 independently.
2. Compare reported voltage against a calibrated meter from 3.3 V through
   4.2 V, both on battery power and while charging.
3. Exercise deep-sleep wakeup and the first-sample discard path.
4. Measure error while Wi-Fi, BLE, and IEEE 802.15.4 create transient loads.
5. Characterize the production cell across expected temperature and load.
6. Confirm low-battery policy against regulator dropout and the cell protection
   cutoff; do not derive safety thresholds from the example percentage curve.

## Ownership boundaries

- `crates/power` owns portable measurements and estimation types.
- `ports/espressif/esp32c6` owns runtime initialization shared by ESP32-C6
  targets.
- `boards/dfrobot/beetle-esp32c6` owns the divider, GPIO assignment, sampling,
  and calibrated ADC adapter.
- `firmware/dfrobot/beetle-esp32c6-battery` owns the cell profile, sampling
  interval, and serial reporting policy.
