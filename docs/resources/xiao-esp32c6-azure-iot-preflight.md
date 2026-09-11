# XIAO ESP32C6 Azure IoT X.509 resources

- Measurement date: 2026-09-11
- Git branch: `feat/cloud-iot-hub`
- Target: `riscv32imac-unknown-none-elf`
- Profile: workspace release profile with fat LTO and size optimization
- Scope: Wi-Fi, DNS/TCP, hardware RNG registration, two-root validation,
  software X.509 mutual TLS, MQTT 3.1.1, Azure subscription/twin setup,
  reported properties, method and telemetry queues, and reconnect composition

Both the safe-default build and a credential-configured build using a generated
throwaway P-256 test identity completed successfully. The configured warm
release build took 36.46 seconds. This is build-cost evidence, not a runtime
benchmark or live Azure evidence.

`espflash save-image` reported:

```text
Application image: 1,442,880 bytes
Application partition: 4,128,768 bytes
Partition use: 34.95%
```

The ELF section snapshot reported:

| Section | Bytes |
| --- | ---: |
| `.rwtext` | 4,184 |
| `.rwtext.wifi` | 55,060 |
| `.data` | 10,364 |
| `.data.wifi` | 480 |
| `.bss` | 159,616 |
| `.rodata` | 135,200 |
| `.rodata.wifi` | 40,696 |
| `.text` | 1,194,932 |
| `.stack` | 220,800 |

The `.bss` value includes the configured 96 KiB global heap arena. Linker
`.stack` is the remainder of the ESP32-C6 RAM region, not measured peak stack
use.

The full TLS and cloud path is linked, so this replaces the earlier DNS-only
preflight snapshot. It still does not measure runtime peak heap, fragmentation,
task high-water marks, handshake latency, or reconnect behavior. Those require
HIL instrumentation with a dedicated IoT Hub identity.
