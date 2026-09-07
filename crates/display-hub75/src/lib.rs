#![no_std]
#![forbid(unsafe_code)]
#![doc = "Portable configuration for HUB75 RGB matrix panels."]

use core::fmt;

use embedded_sdk_display::{Brightness, DisplaySize};

/// Error returned while constructing HUB75 configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// HUB75 row scanning requires a power-of-two divisor from 2 through 32.
    InvalidScanDivisor,
    /// The panel height is not divisible by the selected scan divisor.
    IncompatibleScanRate,
    /// RGB color depth must be between one and eight bitplanes per channel.
    InvalidColorDepth,
    /// A panel grid must contain at least one panel in each direction.
    EmptyPanelGrid,
    /// The composed canvas dimensions overflow the portable display size.
    CanvasTooLarge,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidScanDivisor => {
                formatter.write_str("HUB75 scan divisor must be 2, 4, 8, 16, or 32")
            }
            Self::IncompatibleScanRate => {
                formatter.write_str("panel height must be divisible by the HUB75 scan divisor")
            }
            Self::InvalidColorDepth => {
                formatter.write_str("HUB75 color depth must be between 1 and 8 bitplanes")
            }
            Self::EmptyPanelGrid => formatter.write_str("HUB75 panel grid must be non-empty"),
            Self::CanvasTooLarge => {
                formatter.write_str("HUB75 panel grid exceeds the maximum display size")
            }
        }
    }
}

impl core::error::Error for ConfigError {}

/// Physical multiplexing rate of a HUB75 panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanRate(u8);

impl ScanRate {
    /// Creates a scan rate from the denominator in `1/N`.
    pub const fn from_divisor(divisor: u8) -> Result<Self, ConfigError> {
        if divisor < 2 || divisor > 32 || !divisor.is_power_of_two() {
            return Err(ConfigError::InvalidScanDivisor);
        }

        Ok(Self(divisor))
    }

    /// Common `1/8` scan rate.
    pub const ONE_EIGHTH: Self = Self(8);
    /// Common `1/16` scan rate.
    pub const ONE_SIXTEENTH: Self = Self(16);
    /// Common `1/32` scan rate.
    pub const ONE_THIRTY_SECOND: Self = Self(32);

    /// Returns the denominator in `1/N`.
    #[must_use]
    pub const fn divisor(self) -> u8 {
        self.0
    }

    /// Returns the number of row-address bits required by this scan rate.
    #[must_use]
    pub const fn address_bits(self) -> u8 {
        self.0.ilog2() as u8
    }
}

/// Number of binary color planes retained per RGB channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorDepth(u8);

impl ColorDepth {
    /// Creates a color depth supported by RGB888-backed HUB75 framebuffers.
    pub const fn new(bitplanes: u8) -> Result<Self, ConfigError> {
        if bitplanes == 0 || bitplanes > 8 {
            return Err(ConfigError::InvalidColorDepth);
        }

        Ok(Self(bitplanes))
    }

    /// Returns the number of bitplanes per RGB channel.
    #[must_use]
    pub const fn bitplanes(self) -> u8 {
        self.0
    }

    /// Returns the number of brightness levels per channel.
    #[must_use]
    pub const fn levels(self) -> u16 {
        1_u16 << self.0
    }
}

/// Electrical interface between the MCU and the HUB75 connector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DriveMode {
    /// Six RGB, row address, latch, blank, and clock lines are driven directly.
    Direct,
    /// Row addressing is retained by an external latch circuit.
    Latched,
}

/// Coordinate traversal used when panels form a two-dimensional grid.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum ChainOrder {
    /// Every panel row is chained in the same left-to-right direction.
    #[default]
    Progressive,
    /// Alternate panel rows reverse direction to shorten ribbon-cable runs.
    Serpentine,
}

/// Description of one physical HUB75 panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelSpec {
    size: DisplaySize,
    scan_rate: ScanRate,
}

impl PanelSpec {
    /// Creates a panel description and validates its row multiplexing.
    pub const fn new(size: DisplaySize, scan_rate: ScanRate) -> Result<Self, ConfigError> {
        if !size.height().is_multiple_of(scan_rate.divisor() as u16) {
            return Err(ConfigError::IncompatibleScanRate);
        }

        Ok(Self { size, scan_rate })
    }

    /// Returns the physical panel dimensions.
    #[must_use]
    pub const fn size(self) -> DisplaySize {
        self.size
    }

    /// Returns the physical panel scan rate.
    #[must_use]
    pub const fn scan_rate(self) -> ScanRate {
        self.scan_rate
    }
}

/// Layout of one or more identical, daisy-chained HUB75 panels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanelGrid {
    panel: PanelSpec,
    columns: u8,
    rows: u8,
    order: ChainOrder,
    canvas: DisplaySize,
}

impl PanelGrid {
    /// Creates a physical panel grid and calculates its logical canvas.
    pub const fn new(
        panel: PanelSpec,
        columns: u8,
        rows: u8,
        order: ChainOrder,
    ) -> Result<Self, ConfigError> {
        if columns == 0 || rows == 0 {
            return Err(ConfigError::EmptyPanelGrid);
        }

        let canvas = match panel.size.checked_scale(columns, rows) {
            Ok(size) => size,
            Err(_) => return Err(ConfigError::CanvasTooLarge),
        };

        Ok(Self {
            panel,
            columns,
            rows,
            order,
            canvas,
        })
    }

    /// Returns the physical panel type repeated by this grid.
    #[must_use]
    pub const fn panel(self) -> PanelSpec {
        self.panel
    }

    /// Returns the number of panels chained horizontally.
    #[must_use]
    pub const fn columns(self) -> u8 {
        self.columns
    }

    /// Returns the number of panel rows.
    #[must_use]
    pub const fn rows(self) -> u8 {
        self.rows
    }

    /// Returns the physical chain traversal.
    #[must_use]
    pub const fn order(self) -> ChainOrder {
        self.order
    }

    /// Returns the combined logical drawing surface.
    #[must_use]
    pub const fn canvas(self) -> DisplaySize {
        self.canvas
    }
}

/// Complete portable configuration consumed by a HUB75 platform adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    grid: PanelGrid,
    color_depth: ColorDepth,
    drive_mode: DriveMode,
    brightness: Brightness,
}

impl Config {
    /// Creates a validated HUB75 configuration.
    #[must_use]
    pub const fn new(
        grid: PanelGrid,
        color_depth: ColorDepth,
        drive_mode: DriveMode,
        brightness: Brightness,
    ) -> Self {
        Self {
            grid,
            color_depth,
            drive_mode,
            brightness,
        }
    }

    /// Returns the panel topology.
    #[must_use]
    pub const fn grid(self) -> PanelGrid {
        self.grid
    }

    /// Returns the framebuffer color depth.
    #[must_use]
    pub const fn color_depth(self) -> ColorDepth {
        self.color_depth
    }

    /// Returns the electrical drive mode.
    #[must_use]
    pub const fn drive_mode(self) -> DriveMode {
        self.drive_mode
    }

    /// Returns the requested brightness policy.
    #[must_use]
    pub const fn brightness(self) -> Brightness {
        self.brightness
    }
}

#[cfg(test)]
mod tests {
    use embedded_sdk_display::{Brightness, DisplaySize};

    use super::{ChainOrder, ColorDepth, Config, DriveMode, PanelGrid, PanelSpec, ScanRate};

    #[test]
    fn project_anvil_panel_is_representable() {
        let panel =
            PanelSpec::new(DisplaySize::new(64, 32).unwrap(), ScanRate::ONE_SIXTEENTH).unwrap();
        let grid = PanelGrid::new(panel, 1, 1, ChainOrder::Progressive).unwrap();
        let config = Config::new(
            grid,
            ColorDepth::new(4).unwrap(),
            DriveMode::Direct,
            Brightness::MAX,
        );

        assert_eq!(
            (
                config.grid().canvas().width(),
                config.grid().canvas().height()
            ),
            (64, 32)
        );
        assert_eq!(config.color_depth().levels(), 16);
        assert_eq!(config.grid().panel().scan_rate().address_bits(), 4);
    }

    #[test]
    fn panel_grid_exposes_tiled_canvas() {
        let panel =
            PanelSpec::new(DisplaySize::new(64, 32).unwrap(), ScanRate::ONE_SIXTEENTH).unwrap();
        let grid = PanelGrid::new(panel, 2, 2, ChainOrder::Serpentine).unwrap();

        assert_eq!((grid.canvas().width(), grid.canvas().height()), (128, 64));
        assert_eq!(grid.order(), ChainOrder::Serpentine);
    }

    #[test]
    fn incompatible_scan_rate_is_rejected() {
        let result = PanelSpec::new(DisplaySize::new(64, 30).unwrap(), ScanRate::ONE_SIXTEENTH);

        assert_eq!(result, Err(super::ConfigError::IncompatibleScanRate));
    }
}
