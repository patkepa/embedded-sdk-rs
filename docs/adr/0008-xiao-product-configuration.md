# ADR 0008: XIAO Reference Product Configuration

- Status: Accepted for compile validation
- Date: 2026-09-04
- Firmware integration: Complete
- HIL validation: Pending

## Context

The portable provisioning service deliberately does not know which fields make
up a product configuration. The XIAO reference firmware needs one bounded
schema that can replace its build-time Wi-Fi, controlled network-probe, and
development MQTT fixture settings without moving product policy into a
transport or the generic provisioning crate.

The product representation must preserve the distinction between an absent
credential and a present empty credential, reject partial optional sections,
require an explicit plaintext-MQTT acknowledgement, and avoid printable types
for configured identities.

## Decision

The portable `xiao-esp32c6-config` crate is owned by the XIAO reference
firmware. Its current product schema is `1.0` and implements both
`embedded_sdk_config::Validate` and the provisioning candidate contract.

The deterministic binary representation uses:

- magic `XCF1` and a bounded flags byte;
- an explicit Wi-Fi authentication discriminant;
- separate credential-present and credential-length fields;
- length-prefixed SSID and credential bytes;
- an optional all-or-none DNS hostname and nonzero TCP probe port;
- an optional all-or-none MQTT fixture hostname, port, and client identifier;
- a required explicit plaintext-fixture acknowledgement when MQTT is present.

All integers are big-endian. Unknown flags, authentication modes, and opt-in
values are rejected instead of normalized. The exact maximum representation is
683 bytes, below the provisioning candidate budget of 1,024 bytes.

Semantic validation reconstructs and uses the SDK Wi-Fi and MQTT value types.
Network probe hostnames follow the existing bounded DNS-hostname policy.
Development plaintext MQTT may be provisioned only by `HilFixture` or
`Factory` authority; `OwnerSetup` may configure Wi-Fi and the controlled probe
but not fixture MQTT.

The owned configuration and its borrowed endpoint views implement neither
`Debug` nor `Display`. Decode and validation errors contain only bounded enum
categories. All owned backing arrays and field metadata are zeroized both
explicitly and on drop.

## Consequences

- Product schema and repository format remain independently versioned.
- Boot code can validate stored configuration without inventing a transport
  authority, while provisioning performs an additional authority-policy check
  before staging.
- The schema currently writes only version 1.0. A later compatible reader may
  accept earlier 1.x records, but a newer minor or different major is rejected
  until explicitly implemented and tested.
- MQTT remains a development fixture feature and does not become a production
  identity or transport-security claim.
- Golden-vector, maximum-boundary, truncation, redaction, zeroization, and boot
  recovery tests run on the host. The reference firmware applies recovered
  configuration and cross-compiles with and without the development fallback.
  Hardware-in-the-loop and power-interruption evidence remain required.

## Alternatives considered

- Reusing the provisioning CBOR envelope as the stored product representation
  was rejected because wire and product schemas evolve independently.
- Storing SDK structs directly was rejected because Rust layout and enum
  discriminants are not persistent formats.
- Treating an empty credential as absent was rejected because it can turn an
  invalid secured-network configuration into an unintended open-network one.
- Allowing plaintext MQTT without a stored acknowledgement was rejected as an
  implicit security downgrade.
