#![no_std]
#![forbid(unsafe_code)]
#![doc = "Bounded product configuration for the XIAO ESP32C6 reference firmware."]

use core::str;

use embedded_sdk_config::{SchemaVersion, Validate};
use embedded_sdk_mqtt::{
    BrokerHostname, BrokerPort, ClientId, Config as MqttConfig, MAX_CLIENT_ID_LEN, MAX_HOSTNAME_LEN,
};
use embedded_sdk_provisioning::{Authority, MAX_CANDIDATE_BYTES, ProvisioningCandidate};
use embedded_sdk_wifi::{
    Authentication, MAX_PASSPHRASE_LEN, MAX_SSID_LEN, Passphrase, Ssid, StationConfig,
};
use zeroize::Zeroize;

mod boot;
mod fixture;
mod fixture_serial;

pub use boot::{
    BootConfiguration, BootConfigurationError, BootOutcome, BootTransition,
    recover_boot_configuration,
};
pub use fixture::{FixtureOutcome, HilFixtureProvisioner};
pub use fixture_serial::{
    SERIAL_FRAME_HEADER_BYTES, SERIAL_FRAME_OVERHEAD_BYTES, SerialFrame, SerialFrameDecoder,
    SerialFrameError, SerialFrameKind, encode_serial_frame,
};

/// Product-configuration schema written by this firmware.
pub const CURRENT_SCHEMA: SchemaVersion = SchemaVersion::new(1, 0);
/// Maximum deterministic encoding produced by this schema.
pub const MAX_ENCODED_BYTES: usize = 683;

const _: () = assert!(MAX_ENCODED_BYTES <= MAX_CANDIDATE_BYTES);

const MAGIC: [u8; 4] = *b"XCF1";
const FLAG_NETWORK_PROBE: u8 = 1 << 0;
const FLAG_MQTT_FIXTURE: u8 = 1 << 1;
const KNOWN_FLAGS: u8 = FLAG_NETWORK_PROBE | FLAG_MQTT_FIXTURE;

/// Redacted product-configuration decoding failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The repository or request declared an unsupported product schema.
    UnsupportedSchema,
    /// The product record is truncated or structurally inconsistent.
    Malformed,
    /// A bounded field exceeds the product limit.
    CapacityExceeded,
    /// A textual field is not valid UTF-8.
    InvalidText,
    /// A security discriminant or opt-in marker is unknown.
    SecurityDowngrade,
    /// Bytes remain after the complete product record.
    TrailingData,
}

/// Redacted semantic validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ValidationError {
    /// The Wi-Fi identity, authentication, or credential combination is invalid.
    Wifi,
    /// The controlled network verification endpoint is invalid.
    NetworkProbe,
    /// Development MQTT fields or limits are invalid.
    MqttFixture,
    /// Plaintext fixture MQTT was not explicitly enabled in the record.
    PlaintextMqttNotAcknowledged,
    /// The authenticated authority cannot enable development fixture MQTT.
    Authority,
}

/// Product encoding failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EncodeError {
    /// Caller-owned output cannot hold the complete deterministic record.
    OutputTooSmall,
    /// The in-memory configuration uses a security mode unknown to this schema.
    UnsupportedSecurity,
}

/// Borrowed network endpoint used for boot-time connectivity verification.
///
/// This type deliberately implements neither `Debug` nor `Display` because
/// provisioning diagnostics treat configured hostnames as sensitive.
pub struct NetworkProbe<'a> {
    host: &'a str,
    port: u16,
}

impl<'a> NetworkProbe<'a> {
    /// Returns the validated DNS hostname.
    #[must_use]
    pub const fn host(&self) -> &'a str {
        self.host
    }

    /// Returns the nonzero TCP verification port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }
}

/// Borrowed development MQTT fixture settings.
///
/// This type deliberately implements neither `Debug` nor `Display` because
/// its broker and client identity are sensitive provisioning fields.
pub struct MqttFixture<'a> {
    host: &'a str,
    port: u16,
    client_id: &'a str,
}

impl<'a> MqttFixture<'a> {
    /// Returns the validated fixture broker hostname.
    #[must_use]
    pub const fn host(&self) -> &'a str {
        self.host
    }

    /// Returns the nonzero fixture broker port.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Returns the validated fixture MQTT client identifier.
    #[must_use]
    pub const fn client_id(&self) -> &'a str {
        self.client_id
    }

    /// Builds the reusable SDK MQTT session configuration.
    pub fn session_config(
        &self,
        keep_alive_seconds: u16,
        session_expiry_seconds: u32,
        maximum_packet_size: u32,
    ) -> Result<MqttConfig, ValidationError> {
        MqttConfig::new(
            BrokerHostname::new(self.host).map_err(|_| ValidationError::MqttFixture)?,
            BrokerPort::new(self.port).map_err(|_| ValidationError::MqttFixture)?,
            ClientId::new(self.client_id).map_err(|_| ValidationError::MqttFixture)?,
            keep_alive_seconds,
            session_expiry_seconds,
            maximum_packet_size,
        )
        .map_err(|_| ValidationError::MqttFixture)
    }
}

/// Complete bounded configuration owned by the reference product firmware.
///
/// All backing arrays are cleared on explicit zeroization and on drop. The
/// type deliberately implements neither `Debug` nor `Display`.
pub struct XiaoConfiguration {
    schema: SchemaVersion,
    authentication: Authentication,
    ssid: [u8; MAX_SSID_LEN],
    ssid_len: u8,
    credential: [u8; MAX_PASSPHRASE_LEN],
    credential_len: u8,
    credential_present: bool,
    probe_host: [u8; MAX_HOSTNAME_LEN],
    probe_host_len: u8,
    probe_port: u16,
    probe_present: bool,
    mqtt_host: [u8; MAX_HOSTNAME_LEN],
    mqtt_host_len: u8,
    mqtt_port: u16,
    mqtt_client_id: [u8; MAX_CLIENT_ID_LEN],
    mqtt_client_id_len: u8,
    mqtt_plaintext_acknowledged: bool,
    mqtt_present: bool,
}

impl XiaoConfiguration {
    /// Decodes one complete deterministic product record without allocation.
    pub fn decode(schema: SchemaVersion, bytes: &[u8]) -> Result<Self, DecodeError> {
        if !CURRENT_SCHEMA.is_compatible_with(schema) {
            return Err(DecodeError::UnsupportedSchema);
        }
        if bytes.len() > MAX_CANDIDATE_BYTES {
            return Err(DecodeError::CapacityExceeded);
        }

        let mut reader = Reader::new(bytes);
        if reader.take(4)? != MAGIC {
            return Err(DecodeError::Malformed);
        }
        let flags = reader.u8()?;
        if flags & !KNOWN_FLAGS != 0 {
            return Err(DecodeError::Malformed);
        }
        let authentication = decode_authentication(reader.u8()?)?;
        let ssid_len = usize::from(reader.u8()?);
        if ssid_len > MAX_SSID_LEN {
            return Err(DecodeError::CapacityExceeded);
        }
        let credential_present = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err(DecodeError::Malformed),
        };
        let credential_len = usize::from(reader.u8()?);
        if credential_len > MAX_PASSPHRASE_LEN {
            return Err(DecodeError::CapacityExceeded);
        }
        if !credential_present && credential_len != 0 {
            return Err(DecodeError::Malformed);
        }
        let ssid_value = reader.take(ssid_len)?;
        let credential_value = reader.take(credential_len)?;
        str::from_utf8(credential_value).map_err(|_| DecodeError::InvalidText)?;

        let mut configuration = Self {
            schema,
            authentication,
            ssid: [0; MAX_SSID_LEN],
            ssid_len: ssid_len as u8,
            credential: [0; MAX_PASSPHRASE_LEN],
            credential_len: credential_len as u8,
            credential_present,
            probe_host: [0; MAX_HOSTNAME_LEN],
            probe_host_len: 0,
            probe_port: 0,
            probe_present: flags & FLAG_NETWORK_PROBE != 0,
            mqtt_host: [0; MAX_HOSTNAME_LEN],
            mqtt_host_len: 0,
            mqtt_port: 0,
            mqtt_client_id: [0; MAX_CLIENT_ID_LEN],
            mqtt_client_id_len: 0,
            mqtt_plaintext_acknowledged: false,
            mqtt_present: flags & FLAG_MQTT_FIXTURE != 0,
        };
        configuration.ssid[..ssid_len].copy_from_slice(ssid_value);
        configuration.credential[..credential_len].copy_from_slice(credential_value);

        if configuration.probe_present {
            let host_len = usize::from(reader.u8()?);
            if host_len > MAX_HOSTNAME_LEN {
                return Err(DecodeError::CapacityExceeded);
            }
            let host = reader.take(host_len)?;
            str::from_utf8(host).map_err(|_| DecodeError::InvalidText)?;
            configuration.probe_host[..host_len].copy_from_slice(host);
            configuration.probe_host_len = host_len as u8;
            configuration.probe_port = reader.u16()?;
        }

        if configuration.mqtt_present {
            configuration.mqtt_plaintext_acknowledged = match reader.u8()? {
                0 => false,
                1 => true,
                _ => return Err(DecodeError::SecurityDowngrade),
            };
            let host_len = usize::from(reader.u8()?);
            if host_len > MAX_HOSTNAME_LEN {
                return Err(DecodeError::CapacityExceeded);
            }
            let host = reader.take(host_len)?;
            str::from_utf8(host).map_err(|_| DecodeError::InvalidText)?;
            configuration.mqtt_host[..host_len].copy_from_slice(host);
            configuration.mqtt_host_len = host_len as u8;
            configuration.mqtt_port = reader.u16()?;

            let client_id_len = usize::from(reader.u8()?);
            if client_id_len > MAX_CLIENT_ID_LEN {
                return Err(DecodeError::CapacityExceeded);
            }
            let client_id = reader.take(client_id_len)?;
            str::from_utf8(client_id).map_err(|_| DecodeError::InvalidText)?;
            configuration.mqtt_client_id[..client_id_len].copy_from_slice(client_id);
            configuration.mqtt_client_id_len = client_id_len as u8;
        }

        if !reader.is_empty() {
            return Err(DecodeError::TrailingData);
        }
        Ok(configuration)
    }

    /// Returns the independently versioned product schema.
    #[must_use]
    pub const fn schema(&self) -> SchemaVersion {
        self.schema
    }

    /// Builds the reusable SDK Wi-Fi station configuration.
    pub fn station_config(&self) -> Result<StationConfig, ValidationError> {
        let ssid = Ssid::new(&self.ssid[..usize::from(self.ssid_len)])
            .map_err(|_| ValidationError::Wifi)?;
        match (self.authentication, self.credential_present) {
            (Authentication::Open, false) => {
                StationConfig::open(ssid).map_err(|_| ValidationError::Wifi)
            }
            (Authentication::Open, true) | (_, false) => Err(ValidationError::Wifi),
            (authentication, true) => {
                let credential =
                    str::from_utf8(&self.credential[..usize::from(self.credential_len)])
                        .map_err(|_| ValidationError::Wifi)?;
                StationConfig::personal(
                    ssid,
                    Passphrase::new(credential).map_err(|_| ValidationError::Wifi)?,
                    authentication,
                )
                .map_err(|_| ValidationError::Wifi)
            }
        }
    }

    /// Returns the optional validated network verification endpoint.
    pub fn network_probe(&self) -> Result<Option<NetworkProbe<'_>>, ValidationError> {
        if !self.probe_present {
            return Ok(None);
        }
        let host = str::from_utf8(&self.probe_host[..usize::from(self.probe_host_len)])
            .map_err(|_| ValidationError::NetworkProbe)?;
        BrokerHostname::new(host).map_err(|_| ValidationError::NetworkProbe)?;
        if self.probe_port == 0 {
            return Err(ValidationError::NetworkProbe);
        }
        Ok(Some(NetworkProbe {
            host,
            port: self.probe_port,
        }))
    }

    /// Returns optional development-only plaintext MQTT fixture settings.
    pub fn mqtt_fixture(&self) -> Result<Option<MqttFixture<'_>>, ValidationError> {
        if !self.mqtt_present {
            return Ok(None);
        }
        if !self.mqtt_plaintext_acknowledged {
            return Err(ValidationError::PlaintextMqttNotAcknowledged);
        }
        let host = str::from_utf8(&self.mqtt_host[..usize::from(self.mqtt_host_len)])
            .map_err(|_| ValidationError::MqttFixture)?;
        let client_id =
            str::from_utf8(&self.mqtt_client_id[..usize::from(self.mqtt_client_id_len)])
                .map_err(|_| ValidationError::MqttFixture)?;
        BrokerHostname::new(host).map_err(|_| ValidationError::MqttFixture)?;
        BrokerPort::new(self.mqtt_port).map_err(|_| ValidationError::MqttFixture)?;
        ClientId::new(client_id).map_err(|_| ValidationError::MqttFixture)?;
        Ok(Some(MqttFixture {
            host,
            port: self.mqtt_port,
            client_id,
        }))
    }

    /// Validates reusable SDK types, cross-field rules, and authority policy.
    pub fn validate_for(&self, authority: Authority) -> Result<(), ValidationError> {
        self.validate()?;
        if self.mqtt_present && !matches!(authority, Authority::HilFixture | Authority::Factory) {
            return Err(ValidationError::Authority);
        }
        Ok(())
    }

    /// Validates all authority-neutral product and cross-field invariants.
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.station_config()?;
        self.network_probe()?;
        self.mqtt_fixture()?;
        Ok(())
    }

    /// Encodes the complete configuration deterministically into caller storage.
    pub fn encode(&self, output: &mut [u8]) -> Result<usize, EncodeError> {
        let mut writer = Writer::new(output);
        writer.bytes(&MAGIC)?;
        let flags = (u8::from(self.probe_present) * FLAG_NETWORK_PROBE)
            | (u8::from(self.mqtt_present) * FLAG_MQTT_FIXTURE);
        writer.u8(flags)?;
        writer.u8(encode_authentication(self.authentication)?)?;
        writer.u8(self.ssid_len)?;
        writer.u8(u8::from(self.credential_present))?;
        writer.u8(self.credential_len)?;
        writer.bytes(&self.ssid[..usize::from(self.ssid_len)])?;
        writer.bytes(&self.credential[..usize::from(self.credential_len)])?;
        if self.probe_present {
            writer.u8(self.probe_host_len)?;
            writer.bytes(&self.probe_host[..usize::from(self.probe_host_len)])?;
            writer.u16(self.probe_port)?;
        }
        if self.mqtt_present {
            writer.u8(u8::from(self.mqtt_plaintext_acknowledged))?;
            writer.u8(self.mqtt_host_len)?;
            writer.bytes(&self.mqtt_host[..usize::from(self.mqtt_host_len)])?;
            writer.u16(self.mqtt_port)?;
            writer.u8(self.mqtt_client_id_len)?;
            writer.bytes(&self.mqtt_client_id[..usize::from(self.mqtt_client_id_len)])?;
        }
        Ok(writer.position())
    }
}

impl ProvisioningCandidate for XiaoConfiguration {
    type DecodeError = DecodeError;
    type ValidationError = ValidationError;

    fn decode(version: SchemaVersion, bytes: &[u8]) -> Result<Self, Self::DecodeError> {
        Self::decode(version, bytes)
    }

    fn validate_for(&self, authority: Authority) -> Result<(), Self::ValidationError> {
        self.validate_for(authority)
    }
}

impl Validate for XiaoConfiguration {
    type Error = ValidationError;

    fn validate(&self) -> Result<(), Self::Error> {
        XiaoConfiguration::validate(self)
    }
}

impl Zeroize for XiaoConfiguration {
    fn zeroize(&mut self) {
        self.schema = SchemaVersion::new(0, 0);
        self.authentication = Authentication::Open;
        self.ssid.zeroize();
        self.ssid_len.zeroize();
        self.credential.zeroize();
        self.credential_len.zeroize();
        self.credential_present.zeroize();
        self.probe_host.zeroize();
        self.probe_host_len.zeroize();
        self.probe_port.zeroize();
        self.probe_present.zeroize();
        self.mqtt_host.zeroize();
        self.mqtt_host_len.zeroize();
        self.mqtt_port.zeroize();
        self.mqtt_client_id.zeroize();
        self.mqtt_client_id_len.zeroize();
        self.mqtt_plaintext_acknowledged.zeroize();
        self.mqtt_present.zeroize();
    }
}

impl Drop for XiaoConfiguration {
    fn drop(&mut self) {
        self.zeroize();
    }
}

const fn encode_authentication(authentication: Authentication) -> Result<u8, EncodeError> {
    match authentication {
        Authentication::Open => Ok(0),
        Authentication::Wpa2Personal => Ok(1),
        Authentication::Wpa3Personal => Ok(2),
        Authentication::Wpa2Wpa3Personal => Ok(3),
        _ => Err(EncodeError::UnsupportedSecurity),
    }
}

const fn decode_authentication(value: u8) -> Result<Authentication, DecodeError> {
    match value {
        0 => Ok(Authentication::Open),
        1 => Ok(Authentication::Wpa2Personal),
        2 => Ok(Authentication::Wpa3Personal),
        3 => Ok(Authentication::Wpa2Wpa3Personal),
        _ => Err(DecodeError::SecurityDowngrade),
    }
}

struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let (value, remaining) = self
            .remaining
            .split_at_checked(len)
            .ok_or(DecodeError::Malformed)?;
        self.remaining = remaining;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    const fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
}

struct Writer<'a> {
    output: &'a mut [u8],
    position: usize,
}

impl<'a> Writer<'a> {
    fn new(output: &'a mut [u8]) -> Self {
        Self {
            output,
            position: 0,
        }
    }

    fn bytes(&mut self, value: &[u8]) -> Result<(), EncodeError> {
        let end = self
            .position
            .checked_add(value.len())
            .ok_or(EncodeError::OutputTooSmall)?;
        let destination = self
            .output
            .get_mut(self.position..end)
            .ok_or(EncodeError::OutputTooSmall)?;
        destination.copy_from_slice(value);
        self.position = end;
        Ok(())
    }

    fn u8(&mut self, value: u8) -> Result<(), EncodeError> {
        self.bytes(&[value])
    }

    fn u16(&mut self, value: u16) -> Result<(), EncodeError> {
        self.bytes(&value.to_be_bytes())
    }

    const fn position(&self) -> usize {
        self.position
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use embedded_sdk_config::SchemaVersion;
    use embedded_sdk_provisioning::{Authority, ProvisioningCandidate};
    use std::{format, vec, vec::Vec};
    use zeroize::Zeroize;

    use super::{
        CURRENT_SCHEMA, DecodeError, MAX_ENCODED_BYTES, ValidationError, XiaoConfiguration,
    };

    fn complete_vector() -> Vec<u8> {
        let mut bytes = vec![b'X', b'C', b'F', b'1', 0x03, 0x01, 0x04, 0x01, 0x08];
        bytes.extend_from_slice(b"wifi");
        bytes.extend_from_slice(b"password");
        bytes.push(13);
        bytes.extend_from_slice(b"probe.example");
        bytes.extend_from_slice(&443_u16.to_be_bytes());
        bytes.push(1);
        bytes.push(12);
        bytes.extend_from_slice(b"mqtt.example");
        bytes.extend_from_slice(&1883_u16.to_be_bytes());
        bytes.push(6);
        bytes.extend_from_slice(b"client");
        bytes
    }

    fn decode_error(bytes: &[u8]) -> DecodeError {
        match XiaoConfiguration::decode(CURRENT_SCHEMA, bytes) {
            Err(error) => error,
            Ok(_) => panic!("configuration unexpectedly decoded"),
        }
    }

    fn maximum_hostname(label: u8) -> Vec<u8> {
        let mut hostname = Vec::new();
        for length in [63, 63, 63, 61] {
            if !hostname.is_empty() {
                hostname.push(b'.');
            }
            hostname.extend(core::iter::repeat_n(label, length));
        }
        hostname
    }

    #[test]
    fn deterministic_encoding_has_a_golden_vector() {
        let expected = complete_vector();
        let configuration = XiaoConfiguration::decode(CURRENT_SCHEMA, &expected).unwrap();
        configuration.validate_for(Authority::HilFixture).unwrap();
        let mut output = [0; MAX_ENCODED_BYTES];
        let len = configuration.encode(&mut output).unwrap();

        assert_eq!(&output[..len], expected);
        assert_eq!(configuration.schema(), CURRENT_SCHEMA);
        let station = configuration.station_config().unwrap();
        assert_eq!(station.ssid().as_bytes(), b"wifi");
        assert_eq!(station.passphrase().unwrap().as_str(), "password");
        let probe = configuration.network_probe().unwrap().unwrap();
        assert_eq!((probe.host(), probe.port()), ("probe.example", 443));
        let mqtt = configuration.mqtt_fixture().unwrap().unwrap();
        assert_eq!(
            (mqtt.host(), mqtt.port(), mqtt.client_id()),
            ("mqtt.example", 1883, "client")
        );
        mqtt.session_config(30, 300, 512).unwrap();
    }

    #[test]
    fn fixture_mqtt_requires_authority_and_explicit_plaintext_acknowledgement() {
        let bytes = complete_vector();
        let configuration = XiaoConfiguration::decode(CURRENT_SCHEMA, &bytes).unwrap();
        assert_eq!(
            configuration.validate_for(Authority::OwnerSetup),
            Err(ValidationError::Authority)
        );

        let mut without_acknowledgement = bytes;
        let marker = 9 + 4 + 8 + 1 + 13 + 2;
        without_acknowledgement[marker] = 0;
        let configuration =
            XiaoConfiguration::decode(CURRENT_SCHEMA, &without_acknowledgement).unwrap();
        assert_eq!(
            configuration.validate_for(Authority::HilFixture),
            Err(ValidationError::PlaintextMqttNotAcknowledged)
        );
    }

    #[test]
    fn absent_credential_is_distinct_from_an_empty_credential() {
        let open = [
            b'X', b'C', b'F', b'1', 0, 0, 4, 0, 0, b'w', b'i', b'f', b'i',
        ];
        XiaoConfiguration::decode(CURRENT_SCHEMA, &open)
            .unwrap()
            .validate_for(Authority::OwnerSetup)
            .unwrap();

        let present_empty = [
            b'X', b'C', b'F', b'1', 0, 0, 4, 1, 0, b'w', b'i', b'f', b'i',
        ];
        assert_eq!(
            XiaoConfiguration::decode(CURRENT_SCHEMA, &present_empty)
                .unwrap()
                .validate_for(Authority::OwnerSetup),
            Err(ValidationError::Wifi)
        );
    }

    #[test]
    fn malformed_and_security_downgrade_inputs_are_redacted() {
        let truncated = decode_error(b"XCF1");
        let unknown_authentication = decode_error(b"XCF1\0\xff\0\0\0");

        assert_eq!(truncated, DecodeError::Malformed);
        assert_eq!(unknown_authentication, DecodeError::SecurityDowngrade);
        assert_eq!(format!("{truncated:?}"), "Malformed");
    }

    #[test]
    fn every_truncated_prefix_is_rejected_without_panicking() {
        let bytes = complete_vector();
        for len in 0..bytes.len() {
            assert!(XiaoConfiguration::decode(CURRENT_SCHEMA, &bytes[..len]).is_err());
        }
        assert_eq!(
            decode_error(&[&bytes[..], &[0]].concat()),
            DecodeError::TrailingData
        );
        assert_eq!(
            XiaoConfiguration::decode(SchemaVersion::new(1, 1), &bytes)
                .err()
                .unwrap(),
            DecodeError::UnsupportedSchema
        );
    }

    #[test]
    fn maximum_fields_fit_the_documented_product_bound() {
        let mut bytes = vec![b'X', b'C', b'F', b'1', 3, 3, 32, 1, 64];
        bytes.extend_from_slice(&[b's'; 32]);
        bytes.extend_from_slice(&[b'a'; 64]);
        bytes.push(253);
        bytes.extend_from_slice(&maximum_hostname(b'p'));
        bytes.extend_from_slice(&443_u16.to_be_bytes());
        bytes.push(1);
        bytes.push(253);
        bytes.extend_from_slice(&maximum_hostname(b'm'));
        bytes.extend_from_slice(&1883_u16.to_be_bytes());
        bytes.push(64);
        bytes.extend_from_slice(&[b'c'; 64]);

        assert_eq!(bytes.len(), MAX_ENCODED_BYTES);
        let configuration = XiaoConfiguration::decode(CURRENT_SCHEMA, &bytes).unwrap();
        configuration.validate_for(Authority::HilFixture).unwrap();
        let mut output = [0; MAX_ENCODED_BYTES];
        assert_eq!(
            configuration.encode(&mut output).unwrap(),
            MAX_ENCODED_BYTES
        );
    }

    #[test]
    fn explicit_zeroize_clears_every_backing_field() {
        let mut configuration =
            XiaoConfiguration::decode(CURRENT_SCHEMA, &complete_vector()).unwrap();
        configuration.zeroize();

        assert!(configuration.ssid.iter().all(|byte| *byte == 0));
        assert!(configuration.credential.iter().all(|byte| *byte == 0));
        assert!(configuration.probe_host.iter().all(|byte| *byte == 0));
        assert!(configuration.mqtt_host.iter().all(|byte| *byte == 0));
        assert!(configuration.mqtt_client_id.iter().all(|byte| *byte == 0));
        assert_eq!(configuration.ssid_len, 0);
        assert_eq!(configuration.credential_len, 0);
        assert!(!configuration.credential_present);
        assert!(!configuration.probe_present);
        assert!(!configuration.mqtt_present);
    }

    #[test]
    fn trait_uses_the_same_schema_and_validation_policy() {
        let bytes = complete_vector();
        let configuration =
            <XiaoConfiguration as ProvisioningCandidate>::decode(CURRENT_SCHEMA, &bytes).unwrap();
        <XiaoConfiguration as ProvisioningCandidate>::validate_for(
            &configuration,
            Authority::Factory,
        )
        .unwrap();
    }
}
