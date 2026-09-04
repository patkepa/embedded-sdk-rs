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

MQTT has a portable, version-aware bounded API, a `minimq` MQTT 5 adapter, and
an experimental allocation-free MQTT 3.1.1 adapter for the Azure work. The
existing XIAO firmware's plaintext transport remains a local fixture; neither
path is production support until the authenticated TLS and hardware validation
gates pass. See the [MQTT guide](docs/connectivity/mqtt.md).

The dedicated XIAO Azure IoT firmware now compile-checks public hub identity,
hardware entropy registration, DNS resolution, fixed MQTT replay storage, and
a bounded RAM telemetry queue. It intentionally stops before authentication
until trusted time, trust roots, and a runtime credential source are composed.
See its [firmware guide](firmware/seeed/xiao-esp32c6-azure-iot/README.md).

A dedicated non-connectable iBeacon firmware is also available for the XIAO
ESP32C6. See the [Beacon Guide](docs/connectivity/beacon.md) for deployment
configuration, flashing, calibration, and production boundaries.

The XIAO can also run as a dedicated BLE advertisement scanner that prints a
rolling device list over its USB serial connection. See the
[Beacon Scanner Guide](docs/connectivity/beacon-scanner.md).

802.15.4/OpenThread, production cloud support, board-specific storage backends,
and OTA remain planned. The Azure work now includes an experimental `no_std`,
allocator-backed TLS 1.2 stream with caller-owned record buffers; it is not
production support until its verification, resource, and live-service gates
pass. The complete scope is documented in the
[Azure IoT Hub Integration Plan](docs/plans/azure-iot-hub.md).

Portable trusted-time, credential-lifetime, secure-random, and zeroizing
secret contracts are available through the facade's `security` module. They
are foundations rather than a claim of a production TLS or credential backend.

Portable persistence is described in the [Storage Guide](docs/storage.md).

## License

Licensed under either Apache License 2.0 or the MIT license, at your option.
