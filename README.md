# Embedded SDK

An Embassy-first, `no_std` Rust workspace for building connected embedded
devices across hardware platforms.

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

Wi-Fi station operation, DHCPv4/DNS/TCP networking, and Bluetooth Low Energy
peripheral operation are implemented on the Seeed Studio XIAO ESP32C6
reference target. See the [Wi-Fi](docs/connectivity/wifi.md),
[IP Networking](docs/connectivity/networking.md), and
[Bluetooth](docs/connectivity/bluetooth.md) guides for their current support
boundaries.

MQTT 5 has a portable bounded API, a `minimq` adapter, and an explicit
plaintext local-fixture path in the XIAO firmware. It is not production support
until the authenticated TLS and hardware validation gates pass. See the
[MQTT guide](docs/connectivity/mqtt.md).

A dedicated non-connectable iBeacon firmware is also available for the XIAO
ESP32C6. See the [Beacon Guide](docs/connectivity/beacon.md) for deployment
configuration, flashing, calibration, and production boundaries.

The XIAO can also run as a dedicated BLE advertisement scanner that prints a
rolling device list over its USB serial connection. See the
[Beacon Scanner Guide](docs/connectivity/beacon-scanner.md).

Portable battery measurement and explicitly approximate voltage-curve state of
charge estimation are implemented by the DFRobot Beetle ESP32-C6 reference
target. See the [Power Guide](docs/power.md) and
[Beetle ESP32-C6 Platform Guide](docs/porting/dfrobot-beetle-esp32c6.md).

802.15.4/OpenThread, secure cloud connectivity, board-specific storage
backends, and OTA remain planned.

Portable persistence is described in the [Storage Guide](docs/storage.md).

## License

Licensed under either Apache License 2.0 or the MIT license, at your option.
