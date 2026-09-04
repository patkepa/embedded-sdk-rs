# ADR 0006: Provisioning Wire Codec and Initial Limits

- Status: Accepted for the serial slice
- Date: 2026-09-04

## Context

Provisioning requests carry credentials and cross a framing boundary exposed
to malformed input. The format must work without allocation, preserve stable
numeric field identities, reject ambiguous representations before mutation,
and leave enough RAM for the ESP32-C6 radio stacks and executor tasks.

The largest initial product configuration is dominated by two bounded
hostnames (253 bytes each), a 64-byte MQTT client identifier, a 32-byte SSID,
and a 64-byte Wi-Fi credential. A 1 KiB candidate limit leaves more than 100
bytes for flags, ports, lengths, schema evolution, and product metadata.

## Decision

1. Version 1 uses `minicbor` 2.3 with default features disabled. It is a
   maintained `no_std` CBOR implementation and requires no allocator or Serde.
2. The SDK manually encodes and decodes the security-sensitive envelope rather
   than deriving it. The envelope is a definite CBOR map with numeric keys,
   an explicit magic, major and minor version, operation, request identifier,
   optional transaction identifier, repeated payload length, and byte-string
   payload.
3. Decoding rejects indefinite maps, duplicate or unknown fields, zero IDs,
   unsupported versions, inconsistent payload lengths, invalid operation
   payloads, oversized input, and trailing bytes before the service sees a
   request.
4. Wire and persistent formats remain independent. CBOR is not written
   directly into provisioning repository metadata.
5. Responses use the same envelope fields without a transaction identifier.
   Success kinds carry only bounded capabilities, redacted status, transition
   acknowledgement, or pending generation. Failure payloads contain one stable
   error discriminant and never formatted backend or candidate errors.
6. The fixed 36-byte status payload uses explicit device-state and
   transaction-state discriminants plus fixed-width IDs, generations, schema,
   attempt count, and bounded reason. Unused fields must be zero so alternate
   ambiguous representations are rejected.

Initial fixed budgets are:

| Item | Bytes |
| --- | ---: |
| Product candidate | 1,024 |
| Complete request envelope | 1,088 |
| Complete response envelope | 256 |
| Reassembled transport frame | 1,104 |

The 16-byte difference between request and transport limits is reserved for a
serial delimiter, explicit frame length, integrity value, and framing flags.
BLE chunk metadata is outside the reassembled service request.

## Consequences

- Candidate decoding has a hard linear-work and memory bound.
- Numeric map keys permit compatible optional fields in later minor versions,
  but version 1 rejects unknown keys until their semantics are explicitly
  accepted.
- The duplicated payload length catches framing or envelope disagreement even
  though CBOR byte strings already carry a length.
- Request and response encoding and decoding have checked-in golden,
  round-trip, and malformed-input tests. Transport framing remains subsequent
  work.
- Compile-size and stack deltas must be recorded when the codec is linked into
  the XIAO serial fixture build; a host-only dependency does not provide that
  evidence.

## Alternatives considered

- JSON was rejected because it adds textual ambiguity and does not naturally
  provide the required bounded, allocation-free device representation.
- Postcard was rejected for this envelope because its compact positional Serde
  representation does not expose permanent numeric field identifiers.
- Deriving the CBOR decoder was rejected because the service needs explicit
  duplicate-field, unknown-field, definite-container, and trailing-data policy
  at the trust boundary.
