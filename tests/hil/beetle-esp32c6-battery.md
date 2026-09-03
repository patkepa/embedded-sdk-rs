# DFRobot Beetle ESP32-C6 Battery-Monitoring HIL Procedure

## Equipment

- DFRobot Beetle ESP32-C6 revision 1.0 and revision 1.1.
- Current-limited programmable supply or protected single-cell Li-ion/LiPo
  battery.
- Calibrated digital multimeter connected at `BAT` and `GND`.
- USB cable for flashing and serial monitoring.

## Procedure

1. Build and flash `beetle-esp32c6-battery` with
   `cargo xtask run-beetle-esp32c6-battery`.
2. Confirm the boot log identifies `beetle-esp32c6` and states that charge
   status is unavailable.
3. With USB removed, apply 3.3 V, 3.6 V, 3.9 V, and 4.2 V at `BAT`. At each
   point, wait for the supply and meter to stabilize and record at least ten
   firmware samples.
4. Verify that `voltage_mv` tracks the multimeter monotonically. Record offset,
   gain error, sample spread, board revision, ambient temperature, and supply
   current. Do not assign a production accuracy bound until this evidence is
   reviewed.
5. Connect a supported rechargeable cell, apply USB power, and repeat the
   voltage comparison while charging. Verify that firmware continues to report
   `charge_state=Unknown` rather than inferring LED state.
6. Remove the battery. Confirm the firmware remains operational on USB and
   document the observed voltage; do not interpret that value as reliable
   battery-presence detection.
7. Repeat the full procedure for both PCB revisions.

## Pass criteria

- Firmware remains responsive throughout all supported input conditions.
- Reported voltage is monotonic and produces no impossible arithmetic wrap.
- State-of-charge output stays within 0 through 100 percent.
- Charge state remains `Unknown` for this board implementation.
- Measured error data is attached to the validation record before changing the
  compatibility tier or documenting an accuracy guarantee.
