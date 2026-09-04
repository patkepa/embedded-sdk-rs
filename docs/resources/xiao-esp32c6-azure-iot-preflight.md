# XIAO ESP32C6 Azure IoT preflight resources

- Measurement date: 2026-09-04
- Git branch: `feat/cloud-iot-hub`
- Target: `riscv32imac-unknown-none-elf`
- Profile: workspace release profile with fat LTO and size optimization
- Scope: Wi-Fi, DNS, hardware RNG registration, two-root PEM parsing, MQTT
  3.1.1 buffers, and bounded telemetry staging

The clean release build completed successfully in 8 minutes 51 seconds on the
development host. Most of that time was release optimization of `p384` and the
alpha `rustls-rustcrypto` provider. This is build-cost evidence, not a runtime
benchmark.

`espflash save-image` reported:

```text
Application image: 146,352 bytes
Application partition: 4,128,768 bytes
Partition use: 3.54%
```

The ELF section snapshot reported:

| Section | Bytes |
| --- | ---: |
| `.rwtext` | 4,108 |
| `.rwtext.wifi` | 4,560 |
| `.data` | 1,960 |
| `.bss` | 101,344 |
| `.rodata` | 26,824 |
| `.text` | 80,736 |

The `.bss` value includes the configured 96 KiB global heap arena. Linker
`.stack` is the remainder of the ESP32-C6 RAM region, not measured peak stack
use.

This preflight does not construct a TLS client configuration or perform a TLS
handshake, so dead-code elimination means these numbers are not the final cloud
path budget. Repeat the measurement after the live trusted-time, TLS, SAS,
MQTT, and PUBACK supervisor is linked, and use HIL instrumentation for peak
heap, fragmentation, task high-water marks, handshake latency, and reconnect
behavior.
