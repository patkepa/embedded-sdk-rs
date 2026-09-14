# Platform Compatibility

| Board | Chip | Target | Tier | Host build | Firmware build | Wi-Fi | IP networking | MQTT | Bluetooth LE | HIL |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Seeed Studio XIAO ESP32C6 | ESP32-C6 | `riscv32imac-unknown-none-elf` | 2 | Yes | Yes | Scan and station association | DHCPv4, DNS, and TCP compile-tested; hardware validation pending | Not supported; plaintext fixture compile-tested only | Connectable GATT peripheral and non-connectable iBeacon | Planned |
| DFRobot Beetle ESP32-C6 (DFR1117) | ESP32-C6 | `riscv32imac-unknown-none-elf` | 3 | Yes | Yes | Passive diagnostic scanner; compile-tested | Not enabled in scanner | Not enabled in scanner | Not enabled in scanner | [Pending](../../tests/hil/beetle-esp32c6-wifi-scanner.md) |

## Tier definitions

- Tier 1: release-gating hardware tests and documented product support.
- Tier 2: continuous compile coverage and periodic hardware validation.
- Tier 3: best-effort community support.

Capabilities not listed as implemented in a board's porting guide must not be
inferred from the chip's radio or peripheral hardware alone.
