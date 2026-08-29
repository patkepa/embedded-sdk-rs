#![no_std]
#![forbid(unsafe_code)]
#![doc = "Allocation-free configuration versioning and validation primitives."]

/// Identifies the schema used to encode persistent configuration.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchemaVersion {
    /// Major version. A change may require an explicit migration.
    pub major: u16,
    /// Minor version. A change must remain backwards compatible.
    pub minor: u16,
}

impl SchemaVersion {
    /// Creates a schema version.
    #[must_use]
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// Returns whether this reader may consume data written with `encoded`.
    #[must_use]
    pub const fn is_compatible_with(self, encoded: Self) -> bool {
        self.major == encoded.major && self.minor >= encoded.minor
    }
}

/// Validation implemented by configuration values before they are activated.
pub trait Validate {
    /// Validation failure returned by the configuration value.
    type Error;

    /// Checks all invariants without changing device state.
    fn validate(&self) -> Result<(), Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::SchemaVersion;

    #[test]
    fn compatibility_requires_same_major_version() {
        let reader = SchemaVersion::new(2, 3);

        assert!(reader.is_compatible_with(SchemaVersion::new(2, 1)));
        assert!(!reader.is_compatible_with(SchemaVersion::new(1, 9)));
        assert!(!reader.is_compatible_with(SchemaVersion::new(2, 4)));
    }
}
