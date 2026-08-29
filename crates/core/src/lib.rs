#![no_std]
#![forbid(unsafe_code)]
#![doc = "Platform-neutral identities and capabilities used across the SDK."]

/// The processor architecture used by a device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Architecture {
    /// A 32-bit RISC-V processor.
    RiscV32,
    /// An Arm Cortex-M processor.
    CortexM,
    /// An Espressif Xtensa processor.
    Xtensa,
    /// A host implementation used for tests and simulation.
    Host,
    /// An architecture not yet represented by this SDK version.
    Other,
}

/// A compact, allocation-free set of hardware capabilities.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Capabilities(u64);

impl Capabilities {
    /// An empty capability set.
    pub const NONE: Self = Self(0);
    /// IEEE 802.11 Wi-Fi connectivity.
    pub const WIFI: Self = Self(1 << 0);
    /// Bluetooth Low Energy connectivity.
    pub const BLE: Self = Self(1 << 1);
    /// IEEE 802.15.4 radio support.
    pub const IEEE_802_15_4: Self = Self(1 << 2);
    /// Thread networking support.
    pub const THREAD: Self = Self(1 << 3);
    /// Hardware-backed random number generation.
    pub const HARDWARE_RNG: Self = Self(1 << 4);
    /// Hardware acceleration for cryptographic operations.
    pub const CRYPTO_ACCELERATION: Self = Self(1 << 5);
    /// USB connectivity.
    pub const USB: Self = Self(1 << 6);
    /// Over-the-air firmware update support.
    pub const OTA: Self = Self(1 << 7);
    /// Persistent storage available to application services.
    pub const PERSISTENT_STORAGE: Self = Self(1 << 8);

    /// Creates a capability set from its stable wire representation.
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Returns the stable wire representation of this capability set.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Returns a set containing all capabilities from `self` and `other`.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns whether every capability in `required` is present.
    #[must_use]
    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
}

/// Static information identifying a supported hardware target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HardwareDescriptor {
    /// Stable board identifier used in manifests and telemetry.
    pub board: &'static str,
    /// MCU or SoC model.
    pub chip: &'static str,
    /// Board manufacturer.
    pub manufacturer: &'static str,
    /// Processor architecture.
    pub architecture: Architecture,
    /// Hardware capabilities available on the target.
    pub capabilities: Capabilities,
}

/// Implemented by board support packages with stable hardware metadata.
pub trait BoardSupport {
    /// Static board descriptor.
    const HARDWARE: HardwareDescriptor;
}

#[cfg(test)]
mod tests {
    use super::Capabilities;

    #[test]
    fn capability_sets_are_additive() {
        let radio = Capabilities::WIFI.union(Capabilities::BLE);

        assert!(radio.contains(Capabilities::WIFI));
        assert!(radio.contains(Capabilities::BLE));
        assert!(!radio.contains(Capabilities::THREAD));
    }
}
