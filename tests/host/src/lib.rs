#![doc = "Host-side integration tests for portable SDK contracts."]

#[cfg(test)]
mod tests {
    use embedded_sdk::{
        config::SchemaVersion,
        core::Capabilities,
        wifi::{Authentication, Passphrase, Ssid, StationConfig},
    };
    use embedded_sdk_board_xiao_esp32c6::HARDWARE;

    #[test]
    fn xiao_esp32c6_descriptor_is_available_without_cross_compiling() {
        assert_eq!(HARDWARE.board, "xiao-esp32c6");
        assert_eq!(HARDWARE.chip, "esp32c6");
        assert!(HARDWARE.capabilities.contains(Capabilities::WIFI));
    }

    #[test]
    fn persistent_schema_policy_is_backwards_compatible_within_major_version() {
        let current = SchemaVersion::new(1, 2);

        assert!(current.is_compatible_with(SchemaVersion::new(1, 0)));
        assert!(!current.is_compatible_with(SchemaVersion::new(2, 0)));
    }

    #[test]
    fn facade_exposes_portable_wifi_configuration() {
        let station = StationConfig::personal(
            Ssid::try_from("test-network").unwrap(),
            Passphrase::new("test-password").unwrap(),
            Authentication::Wpa2Wpa3Personal,
        )
        .unwrap();

        assert_eq!(station.ssid().as_str(), Some("test-network"));
    }
}
