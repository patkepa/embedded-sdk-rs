# XIAO ESP32C6 Platform Guide

## Support status

The Seeed Studio XIAO ESP32C6 is the first maintained target in this
repository. It is currently Tier 2: the firmware is compile-tested, while
automated hardware-in-the-loop release testing has not yet been established.

The initial firmware proves:

- Bare-metal ESP32-C6 startup with `esp-hal`.
- The Embassy executor and time driver through `esp-rtos`.
- ESP-IDF-compatible application metadata.
- Serial diagnostic output and an asynchronous heartbeat.
- Reuse of platform-neutral board and capability metadata.
- Wi-Fi discovery through the portable SDK contract and `esp-radio` adapter.
- Optional station association using development build credentials.
- Disconnect monitoring and automatic association retry with bounded backoff.
- DHCPv4 lease acquisition, DNS resolution, and a controlled TCP connectivity
  probe through `embassy-net`.
- A compile-tested MQTT 5 plaintext fixture with bounded packet buffers,
  queueing, session recovery, and independent reconnect behavior.
- Connectable BLE advertising and a reference GATT status service through
  TrouBLE.
- Dedicated non-connectable iBeacon advertising with deployment-configurable
  identity, interval, calibrated RSSI, and controller TX power.
- Concurrent Wi-Fi/BLE radio operation through Espressif coexistence support.

Secure BLE provisioning and bonding, IEEE 802.15.4/OpenThread, authenticated
TLS/MQTT, production cloud protocols, and OTA are not part of this bring-up.

## Toolchain

ESP32-C6 uses the standard Rust `riscv32imac-unknown-none-elf` target. The
repository pins its Rust toolchain in `rust-toolchain.toml`.

Install the flashing tool if it is not already present:

```sh
cargo install espflash
```

Validate the development environment:

```sh
cargo xtask doctor xiao-esp32c6
```

## Build

```sh
cargo xtask build xiao-esp32c6
```

Build the dedicated beacon firmware with:

```sh
cargo xtask build xiao-esp32c6/beacon
```

Build the dedicated BLE scanner firmware with:

```sh
cargo xtask build xiao-esp32c6/beacon-scanner
```

The resulting ELF is written below:

```text
target/riscv32imac-unknown-none-elf/release/xiao-esp32c6-firmware
```

The beacon ELF is written to
`target/riscv32imac-unknown-none-elf/release/xiao-esp32c6-beacon`.
The scanner ELF is written to
`target/riscv32imac-unknown-none-elf/release/xiao-esp32c6-beacon-scanner`.

## Flash and monitor

Connect the board over USB and run:

```sh
cargo xtask run xiao-esp32c6
```

The target runner invokes `espflash flash --monitor`. The firmware prints its
board and chip identity, starts the `XIAO ESP32C6 SDK` BLE peripheral, performs
one Wi-Fi scan, then toggles the user LED and prints a heartbeat every second.
Scan logs intentionally contain only the AP count and strongest signal; they do
not reveal nearby SSIDs or BSSIDs. Bluetooth logs omit local and peer addresses.

To flash the non-connectable beacon instead, run:

```sh
cargo xtask run xiao-esp32c6/beacon
```

See the [Beacon Guide](../connectivity/beacon.md) before assigning deployment
identifiers or making RF range and battery-life assumptions.

To flash the BLE scanner and monitor its rolling device list, run:

```sh
cargo xtask run xiao-esp32c6/beacon-scanner
```

See the [Beacon Scanner Guide](../connectivity/beacon-scanner.md) for output
fields and address-privacy considerations.

For development-only association with a WPA2/WPA3 network:

```sh
WIFI_SSID='network' WIFI_PASSWORD='passphrase' cargo xtask run xiao-esp32c6
```

For an open network, set only `WIFI_SSID`. Omit both variables for the default
scan-only behavior. See the [Wi-Fi guide](../connectivity/wifi.md) for the
security and networking boundaries of this mechanism.

## Ownership boundaries

- `ports/espressif/esp32c6` integrates ESP32-C6 runtime, Wi-Fi primitives, and
  the BLE controller with the portable SDK contracts.
- `boards/seeed/xiao-esp32c6` defines physical-board identity and
  board-specific constants.
- `firmware/seeed/xiao-esp32c6` owns chip initialization, peripheral
  allocation, the panic implementation, executable metadata, and product tasks.
- `firmware/seeed/xiao-esp32c6-beacon` owns the independent, non-connectable
  beacon product image and its deployment settings.
- `firmware/seeed/xiao-esp32c6-beacon-scanner` owns the independent BLE
  observation and USB-serial diagnostic image.

The firmware drives the XIAO user LED on GPIO15 and deliberately passes `TIMG0`
and `SW_INTERRUPT` to the platform runtime explicitly. Platform code must not
acquire peripheral singletons behind the application's back.

Wireless operation reserves GPIO3 to enable the XIAO RF switch and drives
GPIO14 low to select the on-board antenna. The firmware owns and retains both
outputs for as long as Wi-Fi or BLE is active. The port enables `esp-radio`
coexistence so both protocols can run concurrently.

## Dependency baseline

The initial target pins the stable Espressif release family:

- `esp-hal` 1.1.1
- `esp-rtos` 0.3.0
- `esp-backtrace` 0.19.0
- `esp-println` 0.17.0
- `esp-bootloader-esp-idf` 0.5.0
- `esp-radio` 0.18.0
- `esp-alloc` 0.10.0
- `trouble-host` 0.6.0
- `embassy-net` 0.9.1
- `minimq` 0.13.0

These versions remain centralized in the workspace manifest. Platform-specific
dependencies must not be added to portable crates.

## Next platform milestones

1. Establish an ESP32-C6 hardware-in-the-loop fixture.
2. Add GPIO and board-revision tests.
3. Validate DHCP, DNS, TCP, AP-loss recovery, and BLE coexistence on hardware.
4. Add authenticated BLE provisioning and persistent bonding.
5. Add IEEE 802.15.4 and isolate OpenThread FFI behind its platform layer.
6. Define flash partitions before persistent configuration or OTA is added.
