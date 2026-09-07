# Original ESP32 / ESP32-WROOM port

## Status

- Chip metadata: implemented and host-tested
- HUB75 I2S-parallel/DMA adapter: implemented behind `hub75`
- Wi-Fi, Bluetooth, Embassy runtime, and storage adapters: not yet implemented
- ESP32-WROOM carrier-board packages: not yet implemented
- Xtensa cross-build: configured as a dedicated CI quality gate
- Hardware-in-the-loop validation: pending

`ESP32-WROOM` names a family of modules based on the original Xtensa ESP32. A
module does not define a carrier board's connector wiring, flash size, power
design, or peripherals, so this port contains chip integration only. Those
details belong in a board package.

## Toolchain

The original ESP32 needs Espressif's Xtensa-enabled Rust fork. Install and
activate it using `espup`, then use the `xtensa-esp32-none-elf` target. See the
[Rust on ESP toolchain guide](https://docs.espressif.com/projects/rust/book/getting-started/toolchain.html)
for current installation instructions.

The platform package can then be checked with:

```text
cargo +esp check \
  -p embedded-sdk-platform-esp32 \
  --target xtensa-esp32-none-elf \
  --no-default-features \
  --features hub75
```

## HUB75

The `hub75` feature selects the original ESP32 backend of `esp-hub75`, which
uses I2S parallel output and DMA. Portable configuration lives in
`embedded-sdk-display-hub75`; pin ownership and driver types are exposed from
`embedded_sdk_platform_esp32::hub75`.

See the [HUB75 guide](../displays/hub75.md) for the dependency boundaries,
reference SmartSign pin map, and the steps required for a supported board and
firmware image.
