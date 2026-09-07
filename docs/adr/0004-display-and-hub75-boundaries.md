# ADR 0004: Display and HUB75 boundaries

## Status

- Status: Accepted
- Date: 2026-09-07
- Scope: Display APIs, HUB75 panels, and platform display adapters

## Context

The display implementation in `project-anvil` combines four concerns in one
module:

- the physical 64x32, 1/16-scan panel description;
- the SmartSign GPIO assignment;
- the ESP32 I2S/DMA driver and its static buffers;
- application rendering such as images, text, and animation.

That is appropriate for one firmware image, but copying that module into this
SDK would make portable application code depend on one ESP32 variant and one
board pinout. It would also prevent another panel, panel chain, or platform
driver from using the common configuration.

The Rust embedded ecosystem already has `embedded-graphics` for drawing. The
SDK should not introduce another pixel-drawing trait.

## Decision

Display support is split across the existing repository layers:

```text
firmware: scenes, text, images, animation, refresh supervision
    |
boards: connector pin assignment and electrical constraints
    |
ports: peripheral ownership, GPIO tokens, DMA and concrete framebuffer
    |
portable crates: geometry, brightness, HUB75 panel and chain configuration
```

`embedded-sdk-display` owns protocol-neutral values such as display size,
brightness, orientation, and presentation mode.

`embedded-sdk-display-hub75` owns validated panel geometry, scan rate, color
depth, direct-versus-latched drive mode, and multi-panel topology. It remains
`no_std`, allocation-free, and independent of a vendor HAL.

The original ESP32 port used by ESP32-WROOM modules exposes an optional
`hub75` module backed by `esp-hub75`. The module owns the I2S-parallel/DMA
boundary and re-exports the concrete framebuffer, pin, controller, and
descriptor types required by firmware. The dependency is optional so devices
without a display do not pay its compile-time or binary cost.

Board packages must define named connector mappings only when a supported
physical board or shield actually provides that wiring. A platform port must
not contain product pin numbers. Firmware retains ownership of static DMA
buffer allocation because its panel dimensions and color depth determine the
memory budget.

Application drawing continues to use `embedded-graphics` against the concrete
framebuffer. Image assets and product-specific rendering helpers stay in
firmware or a separate application UI crate rather than in the driver layer.

## Consequences

- The 64x32, 1/16-scan, four-bit panel from `project-anvil` is representable
  without importing its ESP32 or SmartSign pin assumptions.
- Chained and tiled panels can share a portable configuration while each
  backend chooses a compatible framebuffer/remapper.
- ESP32-WROOM firmware can opt into the current DMA driver without making the
  portable SDK depend on `esp-hal`.
- The original ESP32 has a real platform port, while a SmartSign board package
  and hardware-in-the-loop test remain required before that physical product
  is represented as verified.
- Brightness is a portable policy value. A backend must document whether it
  implements it through output-enable timing, pixel scaling, or another
  mechanism.
