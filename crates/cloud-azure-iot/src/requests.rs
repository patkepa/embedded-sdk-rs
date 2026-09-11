use crate::encode::parse_decimal_u32;

/// Numeric request identifier generated for Azure request-response topics.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RequestId(u32);

impl RequestId {
    /// Creates a request identifier from its numeric representation.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Returns the numeric representation.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Returns whether a service response contains this request identifier.
    #[must_use]
    pub fn matches(self, encoded: &str) -> bool {
        parse_decimal_u32(encoded) == Some(self.0)
    }
}

/// Wrapping allocator for bounded request identifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestIdGenerator {
    next: u32,
}

impl RequestIdGenerator {
    /// Creates a generator whose first allocated identifier is `first`.
    ///
    /// Zero is replaced by one so the generated text is never confused with
    /// an uninitialized identifier in application state.
    #[must_use]
    pub const fn new(first: u32) -> Self {
        Self {
            next: if first == 0 { 1 } else { first },
        }
    }

    /// Allocates an identifier and advances with non-zero wrapping behavior.
    pub const fn allocate(&mut self) -> RequestId {
        let allocated = self.next;
        self.next = match self.next.checked_add(1) {
            Some(next) => next,
            None => 1,
        };
        RequestId::new(allocated)
    }
}

impl Default for RequestIdGenerator {
    fn default() -> Self {
        Self::new(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_wraps_without_allocating_zero() {
        let mut generator = RequestIdGenerator::new(u32::MAX);
        assert_eq!(generator.allocate(), RequestId::new(u32::MAX));
        assert_eq!(generator.allocate(), RequestId::new(1));
        assert!(RequestId::new(42).matches("42"));
        assert!(!RequestId::new(42).matches("invalid"));
    }
}
