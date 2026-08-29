# Embedded SDK

An Embassy-first, `no_std` Rust workspace for building connected embedded
devices across hardware platforms.

The first supported target is the Seeed Studio XIAO ESP32C6. The current
foundation includes portable identity, capability, configuration, service
health, and telemetry crates; an ESP32-C6 platform port; board metadata; host
integration tests; and runnable Embassy firmware.

## Repository layout

```text
crates/       portable SDK libraries
ports/        chip-family and runtime integration
boards/       physical board support packages
firmware/     deployable product and reference binaries
tests/        host, integration, and hardware test suites
tools/xtask/  repository automation
docs/         architecture, porting, and support documentation
```

See [Repository Architecture](docs/architecture/repository-structure.md) for
the intended long-term structure and dependency rules.

## Host development

Run all host-side checks:

```sh
cargo xtask check
```

Run only tests:

```sh
cargo xtask test
```

## ESP32-C6

Verify the local toolchain:

```sh
cargo xtask doctor
```

Build the reference firmware:

```sh
cargo xtask build-xiao-esp32c6
```

Flash a connected XIAO ESP32C6 and open its serial monitor:

```sh
cargo xtask run-xiao-esp32c6
```

Detailed setup and design notes are in the
[ESP32-C6 porting guide](docs/porting/esp32c6.md).

## Project status

The workspace foundation and ESP32-C6 Embassy bring-up are implemented.
Wi-Fi, BLE, IEEE 802.15.4/OpenThread, networking, cloud connectivity, storage,
and OTA are planned capabilities and are not yet implemented.

## License

Licensed under either Apache License 2.0 or the MIT license, at your option.
