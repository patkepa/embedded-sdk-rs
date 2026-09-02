# Bluetooth Low Energy

## Implemented scope

The initial Bluetooth slice provides:

- Allocation-free portable device-name, static-random-address, peripheral
  configuration, and lifecycle types in `embedded-sdk-bluetooth`.
- An ESP32-C6 BLE controller adapter backed by `esp-radio`.
- A TrouBLE 0.6 host in the XIAO ESP32C6 reference firmware.
- Connectable legacy advertising under the name `XIAO ESP32C6 SDK`.
- One concurrent central connection and a small custom GATT service with a
  readable and notifiable status byte.
- A dedicated, configurable, non-connectable iBeacon firmware for the XIAO
  ESP32C6.
- A dedicated XIAO ESP32C6 central-role scanner that reports nearby BLE
  advertisers over USB serial.
- Wi-Fi/BLE radio coexistence in the ESP32-C6 port.

The ESP32-C6 supports Bluetooth Low Energy, not Bluetooth Classic. The SDK uses
BLE terminology where the distinction matters.

The beacon firmware is documented separately in the
[XIAO ESP32C6 iBeacon Guide](beacon.md). It does not expose the bring-up GATT
service or start Wi-Fi.

The scanner firmware is documented in the
[XIAO ESP32C6 Beacon Scanner Guide](beacon-scanner.md). It actively scans but
does not connect to observed devices.

## Portable contract

Application-facing code can depend on `embedded-sdk-bluetooth` without pulling
in an MCU HAL, controller driver, or host stack. `DeviceName` validates the
22-byte limit used by the current legacy GAP profile. `StaticRandomAddress`
normalizes the Bluetooth address type bits, rejects the two degenerate bit
patterns, and provides explicit canonical and HCI byte order.

The XIAO firmware derives its static random address from the chip's unique
Bluetooth interface MAC. This avoids the shared fixed address commonly found in
examples. The resulting address remains stable across resets, so it should not
be treated as a privacy-preserving rotating address.

## XIAO reference peripheral

After flashing, scan with a BLE client such as nRF Connect or LightBlue and
connect to:

```text
XIAO ESP32C6 SDK
```

The reference service uses these UUIDs:

```text
Service: 7a1e1000-4c2a-4f66-a1d4-3f55b55a1000
Status:  7a1e1001-4c2a-4f66-a1d4-3f55b55a1000
```

The status characteristic supports reads and notifications. Its value advances
every five seconds while connected. The firmware returns to advertising after
the central disconnects. Serial logs report lifecycle transitions but do not
print the device address or peer identity.

## Ownership and concurrency

The firmware starts `esp-rtos` before either radio. It owns the XIAO RF-switch
pins for the lifetime of both radios, initializes the ESP BLE controller, and
spawns the long-running TrouBLE host task. The existing Wi-Fi controller then
runs independently. The ESP32-C6 port enables the `esp-radio` `coex` feature so
the vendor radio scheduler arbitrates Wi-Fi and BLE airtime.

The Bluetooth task owns one connection slot, two L2CAP channel slots (signaling
and ATT), and a bounded packet pool of eight 64-byte packets. Advertising and
host-runner failures are logged and retried after one second. There is currently
no watchdog vote for this task.

## Security boundary

The reference GATT service is intentionally a bring-up service. It does not
enable pairing, encryption, bonding, authorization, or persistent keys. Do not
put credentials, commands, or sensitive telemetry in this service.

Secure provisioning requires a separate service and threat model covering
authenticated pairing, key storage, replay protection, credential lifecycle,
factory reset, and rate limiting. Those are follow-up work rather than implied
by connectable advertising.

## Verification

Host tests cover name bounds, UUID parsing, iBeacon payload encoding,
advertising interval validation, address derivation and byte order, facade
exports, and XIAO capability metadata. All XIAO firmware binaries are
cross-compiled for `riscv32imac-unknown-none-elf`. Hardware interoperability,
RF, power, and coexistence tests remain required before promoting this support
beyond Tier 2.
