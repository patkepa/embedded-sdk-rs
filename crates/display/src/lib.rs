#![no_std]
#![forbid(unsafe_code)]
#![doc = "Platform-independent display geometry and presentation types."]

use core::fmt;

/// Error returned while constructing portable display configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// A display surface must have non-zero width and height.
    ZeroDimension,
    /// A percentage must be in the inclusive range 0 through 100.
    PercentageOutOfRange,
    /// A composed display surface exceeds the SDK's 16-bit dimensions.
    DimensionOverflow,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDimension => formatter.write_str("display dimensions must be non-zero"),
            Self::PercentageOutOfRange => {
                formatter.write_str("display percentage must be between 0 and 100")
            }
            Self::DimensionOverflow => {
                formatter.write_str("composed display dimensions exceed 65535 pixels")
            }
        }
    }
}

impl core::error::Error for ConfigError {}

/// Width and height of a logical display surface in pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplaySize {
    width: u16,
    height: u16,
}

impl DisplaySize {
    /// Creates a non-empty display size.
    pub const fn new(width: u16, height: u16) -> Result<Self, ConfigError> {
        if width == 0 || height == 0 {
            return Err(ConfigError::ZeroDimension);
        }

        Ok(Self { width, height })
    }

    /// Returns the display width in pixels.
    #[must_use]
    pub const fn width(self) -> u16 {
        self.width
    }

    /// Returns the display height in pixels.
    #[must_use]
    pub const fn height(self) -> u16 {
        self.height
    }

    /// Returns the number of pixels in the surface.
    #[must_use]
    pub const fn pixel_count(self) -> u32 {
        self.width as u32 * self.height as u32
    }

    /// Multiplies the surface dimensions, rejecting 16-bit overflow.
    pub const fn checked_scale(self, horizontal: u8, vertical: u8) -> Result<Self, ConfigError> {
        if horizontal == 0 || vertical == 0 {
            return Err(ConfigError::ZeroDimension);
        }

        let width = self.width as u32 * horizontal as u32;
        let height = self.height as u32 * vertical as u32;
        if width > u16::MAX as u32 || height > u16::MAX as u32 {
            return Err(ConfigError::DimensionOverflow);
        }

        Ok(Self {
            width: width as u16,
            height: height as u16,
        })
    }
}

/// Normalized display brightness stored without floating-point arithmetic.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Brightness(u8);

impl Brightness {
    /// Display output disabled.
    pub const OFF: Self = Self(0);
    /// Maximum output brightness.
    pub const MAX: Self = Self(u8::MAX);

    /// Creates brightness from its stable 0-through-255 representation.
    #[must_use]
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Creates brightness from an integer percentage.
    pub const fn from_percent(percent: u8) -> Result<Self, ConfigError> {
        if percent > 100 {
            return Err(ConfigError::PercentageOutOfRange);
        }

        Ok(Self((((percent as u16 * u8::MAX as u16) + 50) / 100) as u8))
    }

    /// Returns the stable 0-through-255 representation.
    #[must_use]
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// Logical orientation applied above a physical display driver.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum Orientation {
    /// No rotation.
    #[default]
    Degrees0,
    /// Clockwise quarter turn.
    Degrees90,
    /// Half turn.
    Degrees180,
    /// Clockwise three-quarter turn.
    Degrees270,
}

/// How application frames become visible on a display.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PresentationMode {
    /// Drawing updates the visible buffer directly and can tear.
    Immediate,
    /// A completed back buffer is swapped on a frame boundary.
    DoubleBuffered,
}

#[cfg(test)]
mod tests {
    use super::{Brightness, ConfigError, DisplaySize};

    #[test]
    fn display_size_rejects_empty_surfaces() {
        assert_eq!(DisplaySize::new(0, 32), Err(ConfigError::ZeroDimension));
    }

    #[test]
    fn display_size_scales_for_panel_grids() {
        let panel = DisplaySize::new(64, 32).unwrap();
        let canvas = panel.checked_scale(2, 2).unwrap();

        assert_eq!((canvas.width(), canvas.height()), (128, 64));
        assert_eq!(canvas.pixel_count(), 8_192);
    }

    #[test]
    fn brightness_percentage_uses_full_range() {
        assert_eq!(Brightness::from_percent(0).unwrap(), Brightness::OFF);
        assert_eq!(Brightness::from_percent(100).unwrap(), Brightness::MAX);
        assert_eq!(
            Brightness::from_percent(101),
            Err(ConfigError::PercentageOutOfRange)
        );
    }
}
