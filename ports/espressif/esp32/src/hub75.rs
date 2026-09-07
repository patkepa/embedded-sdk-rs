//! Original ESP32 HUB75 adapter.
//!
//! The portable [`embedded_sdk_display_hub75`] crate owns panel and topology
//! configuration. This module owns the original ESP32-specific I2S parallel,
//! DMA, pin, and framebuffer implementation supplied by `esp-hub75`.

/// Portable panel, scan, wiring, and topology configuration.
pub use embedded_sdk_display_hub75 as config;
/// DMA-friendly framebuffers and tiling adapters.
pub use esp_hub75::framebuffer;
pub use esp_hub75::{
    Color, Hub75, Hub75Error, Hub75Pins8, Hub75Pins16, Hub75Swap, dma_descriptor_count,
};

/// Allocates the static DMA descriptor table required by a framebuffer type.
///
/// This macro is re-exported so firmware does not need a direct dependency on
/// the selected ESP32 HUB75 backend.
pub use esp_hub75::hub75_dma_descriptors;
