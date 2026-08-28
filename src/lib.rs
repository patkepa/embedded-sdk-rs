#![no_std]

/// Returns the SDK's greeting.
pub const fn hello() -> &'static str {
    "Hello, world!"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_hello() {
        assert_eq!(hello(), "Hello, world!");
    }
}
