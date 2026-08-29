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

IP addressing and sockets, BLE, IEEE 802.15.4/OpenThread, storage, and OTA are
not part of this bring-up.

## Toolchain

ESP32-C6 uses the standard Rust `riscv32imac-unknown-none-elf` target. The
repository pins its Rust toolchain in `rust-toolchain.toml`.

Install the flashing tool if it is not already present:

```sh
cargo install espflash
```

Validate the development environment:

```sh
cargo xtask doctor
```

## Build

```sh
cargo xtask build-xiao-esp32c6
```

The resulting ELF is written below:

```text
target/riscv32imac-unknown-none-elf/release/xiao-esp32c6-firmware
```

## Flash and monitor

Connect the board over USB and run:

```sh
cargo xtask run-xiao-esp32c6
```

The target runner invokes `espflash flash --monitor`. The firmware prints its
board and chip identity, performs one Wi-Fi scan, then toggles the user LED and
prints a heartbeat every second. Scan logs intentionally contain only the AP
count and strongest signal; they do not reveal nearby SSIDs or BSSIDs.

For development-only association with a WPA2/WPA3 network:

```sh
WIFI_SSID='network' WIFI_PASSWORD='passphrase' cargo xtask run-xiao-esp32c6
```

For an open network, set only `WIFI_SSID`. Omit both variables for the default
scan-only behavior. See the [Wi-Fi guide](../connectivity/wifi.md) for the
security and networking boundaries of this mechanism.

## Ownership boundaries

- `ports/espressif/esp32c6` integrates ESP32-C6 runtime and Wi-Fi primitives
  with the portable SDK contracts.
- `boards/seeed/xiao-esp32c6` defines physical-board identity and
  board-specific constants.
- `firmware/seeed/xiao-esp32c6` owns chip initialization, peripheral
  allocation, the panic implementation, executable metadata, and product tasks.

The firmware drives the XIAO user LED on GPIO15 and deliberately passes `TIMG0`
and `SW_INTERRUPT` to the platform runtime explicitly. Platform code must not
acquire peripheral singletons behind the application's back.

Wi-Fi additionally reserves GPIO3 to enable the XIAO RF switch and drives
GPIO14 low to select the on-board antenna. The firmware owns and retains both
outputs for as long as the radio is active.

## Dependency baseline

The initial target pins the stable Espressif release family:

- `esp-hal` 1.1.1
- `esp-rtos` 0.3.0
- `esp-backtrace` 0.19.0
- `esp-println` 0.17.0
- `esp-bootloader-esp-idf` 0.5.0
- `esp-radio` 0.18.0
- `esp-alloc` 0.10.0

These versions remain centralized in the workspace manifest. Platform-specific
dependencies must not be added to portable crates.

## Next platform milestones

1. Establish an ESP32-C6 hardware-in-the-loop fixture.
2. Add GPIO and board-revision tests.
3. Add `embassy-net` DHCP, DNS, and sockets on the implemented Wi-Fi link.
4. Add BLE provisioning using a controller/host boundary.
5. Add IEEE 802.15.4 and isolate OpenThread FFI behind its platform layer.
6. Define flash partitions before persistent configuration or OTA is added.
