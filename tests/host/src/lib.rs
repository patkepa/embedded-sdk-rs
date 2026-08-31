#![doc = "Host-side integration tests for portable SDK contracts."]

#[cfg(test)]
mod tests {
    use embedded_sdk::{
        bluetooth::{AdvertisingInterval, BeaconUuid, DeviceName, IBeacon, StaticRandomAddress},
        config::SchemaVersion,
        core::Capabilities,
        networking::{DnsServers, Ipv4Configuration, LinkState, NetworkSnapshot},
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
    fn facade_exposes_portable_ibeacon_configuration() {
        let uuid = BeaconUuid::parse("7a1e1000-4c2a-4f66-a1d4-3f55b55a1000").unwrap();
        let beacon = IBeacon::new(uuid, 1, 2, -59);

        assert_eq!(AdvertisingInterval::default().as_millis(), 250);
        assert_eq!(beacon.uuid(), uuid);
        assert_eq!(&beacon.manufacturer_payload()[18..22], &[0, 1, 0, 2]);
    }

    #[test]
    fn facade_exposes_stable_namespaced_storage_keys() {
        let key = Key::new(0x0100, 0x0002);

        assert_eq!(key.to_raw(), 0x0100_0002);
        assert_eq!(Key::from_raw(key.to_raw()), key);
    }

    #[test]
    fn facade_exposes_portable_network_readiness() {
        let configuration = Ipv4Configuration::new(
            "192.0.2.10".parse().unwrap(),
            24,
            Some("192.0.2.1".parse().unwrap()),
            DnsServers::new(&["192.0.2.53".parse().unwrap()]).unwrap(),
        )
        .unwrap();
        let snapshot = NetworkSnapshot::new(LinkState::Up, Some(configuration));

        assert!(snapshot.is_ip_ready());
        assert!(snapshot.is_dns_ready());
    }
}
