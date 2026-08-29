#![doc = "Host-side integration tests for portable SDK contracts."]

#[cfg(test)]
mod tests {
    use embedded_sdk::{
        bluetooth::{DeviceName, StaticRandomAddress},
        config::SchemaVersion,
        core::Capabilities,
        storage::Key,
        wifi::{Authentication, Passphrase, Ssid, StationConfig},
    };
    use embedded_sdk_board_xiao_esp32c6::HARDWARE;

    #[test]
    fn xiao_esp32c6_descriptor_is_available_without_cross_compiling() {
        assert_eq!(HARDWARE.board, "xiao-esp32c6");
        assert_eq!(HARDWARE.chip, "esp32c6");
        assert!(HARDWARE.capabilities.contains(Capabilities::WIFI));
        assert!(HARDWARE.capabilities.contains(Capabilities::BLE));
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

    #[test]
    fn facade_exposes_portable_bluetooth_configuration() {
        let name = DeviceName::new("test-device").unwrap();
        let address = StaticRandomAddress::from_seed([1, 2, 3, 4, 5, 6]);

        assert_eq!(name.as_str(), "test-device");
        assert_eq!(address.as_bytes()[0] & 0xc0, 0xc0);
    }

    #[test]
    fn facade_exposes_stable_namespaced_storage_keys() {
        let key = Key::new(0x0100, 0x0002);

        assert_eq!(key.to_raw(), 0x0100_0002);
        assert_eq!(Key::from_raw(key.to_raw()), key);
    }
}
