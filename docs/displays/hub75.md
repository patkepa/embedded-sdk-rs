# HUB75 RGB Matrix Displays

## Status

- Portable panel and topology configuration: implemented and host-tested
- Original Xtensa ESP32/ESP32-WROOM I2S-parallel DMA backend: implemented
  behind the port's `hub75` feature
- SmartSign connector mapping from `project-anvil`: reference only, not yet a
  hardware-validated board package in this repository
- Xtensa cross-build: configured as a dedicated CI quality gate
- Hardware-in-the-loop refresh, brightness, and ghosting validation: pending

## Architecture

HUB75 is a display transport and scanout concern, not an application UI API.
The SDK therefore keeps these responsibilities separate:

| Layer | Owns |
| --- | --- |
| `embedded-sdk-display` | Logical size, brightness, orientation, presentation policy |
| `embedded-sdk-display-hub75` | Panel scan rate, bitplanes, direct/latched drive, panel grid |
| Original ESP32 port `hub75` feature | I2S parallel, DMA descriptors, GPIO token types, concrete framebuffers |
| Board package | Verified connector-to-GPIO mapping and electrical notes |
| Firmware | Static framebuffer allocation, scenes/assets, drawing, refresh and swap policy |

This follows [ADR 0004](../adr/0004-display-and-hub75-boundaries.md).

## Portable configuration

The panel used by `project-anvil` is a 64x32 panel with 1/16 scanning and four
color bitplanes. It can be described without a vendor HAL:

```rust
use embedded_sdk::display::{
    Brightness, DisplaySize,
    hub75::{ChainOrder, ColorDepth, Config, DriveMode, PanelGrid, PanelSpec, ScanRate},
};

let panel = PanelSpec::new(
    DisplaySize::new(64, 32).unwrap(),
    ScanRate::ONE_SIXTEENTH,
).unwrap();
let grid = PanelGrid::new(panel, 1, 1, ChainOrder::Progressive).unwrap();
let display = Config::new(
    grid,
    ColorDepth::new(4).unwrap(),
    DriveMode::Direct,
    Brightness::from_percent(30).unwrap(),
);
```

`PanelGrid` also describes horizontal chains and two-dimensional progressive
or serpentine layouts. The platform framebuffer or tiling adapter remains
responsible for the exact coordinate remapping.

## ESP32-WROOM integration

Enable HUB75 only in firmware that owns a suitable connector mapping:

```toml
[dependencies]
embedded-sdk-platform-esp32 = {
    workspace = true,
    default-features = false,
    features = ["hub75"],
}
```

The adapter is available as
`embedded_sdk_platform_esp32::hub75`. It re-exports the safe
`esp-hub75` controller, direct and latched pin groups, DMA descriptor macro,
color type, and framebuffer modules. A firmware package chooses compile-time
framebuffer dimensions and allocates the framebuffer and descriptor table in
static memory, then passes owned I2S, DMA channel, and pin tokens to the
controller.

The backend uses `esp-hub75` 0.15 because it supports the original ESP32 on the
SDK's Rust 1.88 minimum and `esp-hal` 1.1 line. Bitplane framebuffers are the
default recommendation because their RAM usage scales linearly with color
depth.

The original ESP32 is an Xtensa target. Install Espressif's Rust fork with
`espup`, activate the generated environment, and build for
`xtensa-esp32-none-elf`; the standard Rust toolchain used for host and ESP32-C6
checks cannot compile this target.

## Adding a HUB75 device

1. Add or extend a board package with a named HUB75 connector mapping. Keep
   GPIO numbers there, not in the portable crates or platform port.
2. Record the panel size, scan divisor, drive mode, voltage/level-shifting
   requirement, and any GPIO conflicts in board documentation.
3. Add firmware for that board. It owns the concrete const-generic framebuffer,
   static DMA descriptors, render tasks, and application assets.
4. Draw through the framebuffer's `embedded-graphics` implementation. Keep
   reusable widgets in an application UI crate only when more than one
   firmware product needs them.
5. Add HIL coverage for a stable refresh rate, correct row/color mapping,
   blanking/ghosting, frame swaps, and coexistence with enabled radios.

## Electrical and runtime requirements

HUB75 panels commonly use 5 V power and can draw several amps. Power the panel
from a supply sized for its worst-case content; do not source panel power from
an MCU board. A 3.3 V-to-5 V logic level shifter such as a 74HCT245 is strongly
recommended. Pin count differs by drive mode: direct drive includes row address
lines, while latched drive requires compatible external address-latch
hardware.

On the original ESP32, display refresh consumes an I2S peripheral, one DMA
channel, GPIOs, and a significant static RAM budget. The firmware must account
for those resources alongside Wi-Fi/Bluetooth buffers. Refresh and framebuffer
swap behavior must be validated under the firmware's real radio load.

## Reference mapping

The original `project-anvil` SmartSign mapping targets the original ESP32 and
uses I2S parallel output:

| HUB75 | GPIO | HUB75 | GPIO |
| --- | ---: | --- | ---: |
| R1 | 25 | R2 | 14 |
| G1 | 26 | G2 | 12 |
| B1 | 27 | B2 | 13 |
| A | 33 | B | 32 |
| C | 23 | D | 22 |
| E | 18 | LAT | 15 |
| OE | 2 | CLK | 21 |

This table is migration input, not an SDK board definition. Add it to a board
package only after the board identity, schematic/electrical design, Xtensa
toolchain support, and HIL procedure are available.
