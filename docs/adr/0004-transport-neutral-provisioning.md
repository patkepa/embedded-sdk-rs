# ADR 0004: Transport-Neutral Provisioning Boundary

- Status: Accepted
- Date: 2026-09-04

## Context

Device provisioning crosses transport authentication, product configuration,
persistent storage, boot-time activation, and connectivity verification. If a
serial or Bluetooth adapter owns those policies, each transport can acquire a
different validation or recovery path and a disconnect can leave credentials
partially applied.

The storage layer already provides atomic replacement of one complete value,
but one value is insufficient to preserve a known-working configuration while
a candidate is tested. The current XIAO firmware also selects development
credentials at build time and has no reviewed persistent-storage partition.

## Decision

1. `embedded-sdk-provisioning` owns one portable, allocation-free transaction
   model. Transports own framing and authentication, then pass a bounded
   request and trusted in-process authority context to the service.
2. The provisioning service owns request sequencing, authorization,
   idempotency, candidate validation, durable transitions, and redacted
   status. Transport adapters do not parse product configuration or write its
   records.
3. Product firmware owns its concrete configuration schema, field-level
   authority policy, application of a candidate, and the definition and
   deadline of successful verification.
4. Commit stages a complete candidate as pending. It does not confirm that the
   configuration works. Confirmation occurs only after product verification.
5. The initial ESP32-C6 integration reboots to apply a pending candidate.
   Live reconfiguration may be added without changing the durable transaction
   states.
6. The repository uses two candidate slots and one atomically replaced state
   record. It never overwrites the slot holding the confirmed generation while
   a replacement is under test.
7. Provisioning storage keys are permanent format identifiers. Provisioning
   reserves one namespace whose state, slot A, and slot B record numbers are
   registered before the persistent repository is implemented.
8. Wire and persistent schemas evolve independently. Accepting a new wire
   operation does not imply rewriting stored product configuration.
9. No custom cryptographic handshake is introduced. Credential-bearing BLE
   operations remain disabled until a separate threat model selects reviewed
   authentication, encryption, replay, key-storage, and recovery mechanisms.

The portable API treats a `SessionContext` as a capability created only after
transport authentication. Its fields are private and wire decoding never
constructs it. Rust crate visibility is not an authentication boundary, so the
firmware composition root remains responsible for restricting context
construction to trusted adapters.

The wire codec, maximum frame sizes, permanent numeric key values, and XIAO
flash partition are intentionally not selected by this ADR. Each requires the
size, malformed-input, ownership, and hardware evidence listed in the
provisioning implementation plan.

## Consequences

- Serial, fixture, and future BLE adapters share validation, storage,
  activation, rollback, and status behavior.
- A transport disconnect before durable commit can discard only transient
  state; a disconnect after commit cannot silently undo the pending candidate.
- Status and errors expose bounded categories, generations, and attempts but
  never candidate fields, secret lengths, or secret-derived identifiers.
- The initial portable crate can be host-tested without a codec, allocator,
  executor, storage backend, transport, or MCU dependency.
- Board persistent-storage support cannot be claimed until a separate
  partition decision and hardware interruption evidence are complete.

## Alternatives considered

- Transport-owned provisioning was rejected because it duplicates security,
  validation, and recovery policy and makes behavior depend on the carrier.
- Updating one active configuration value in place was rejected because a bad
  candidate or interrupted activation could destroy the last working
  generation.
- Confirming during commit was rejected because durable storage says nothing
  about whether Wi-Fi, DHCP, DNS, or a configured verification probe works.
- Extending the current unauthenticated GATT service was rejected because link
  availability is not authenticated ownership and does not provide replay or
  recovery policy.
