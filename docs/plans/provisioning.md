# Transport-Neutral Device Provisioning Implementation Plan

## Status

- Status: Proposed
- Branch: `feat/provisioning`
- Initial target: Seeed Studio XIAO ESP32C6
- Primary outcome: replace build-time development credentials with one bounded,
  durable provisioning transaction that can be carried by fixture serial first
  and authenticated Bluetooth LE later

## Objective

Provisioning is the controlled transition from a device with no usable product
configuration, or an older confirmed configuration, to a new confirmed
configuration. It is not a transport feature. USB serial, Bluetooth LE, and a
factory fixture may authenticate callers differently and frame bytes
differently, but they must all invoke the same validation, persistence,
activation, rollback, and audit policy.

The first implementation should make the following invariant true:

> A reset, transport loss, malformed request, invalid credential, or power loss
> during provisioning leaves the device with either its previous complete
> confirmed configuration or a complete recoverable candidate. It must never
> activate a partially decoded record or silently discard the last working
> configuration.

The initial delivery path is fixture-owned USB serial because it permits the
transaction and recovery model to be proven without implying that the current
unauthenticated BLE service is safe for credentials. The portable service and
wire operations must not contain serial-specific assumptions, so the later BLE
adapter does not create a second provisioning implementation.

## Current baseline

The repository already provides several pieces of the required foundation:

- `embedded-sdk-config` defines schema compatibility and a portable
  `Validate` contract.
- `embedded-sdk-storage` defines an allocation-free asynchronous
  `KeyValueStore`. A completed `put` atomically replaces one logical value, and
  interrupted writes recover to either the complete old or complete new value.
- `embedded-sdk-wifi` provides bounded SSID and credential types.
- The XIAO firmware has a connectable BLE peripheral and independent Wi-Fi,
  networking, MQTT, BLE, and heartbeat tasks.
- Wi-Fi, network-probe, and development MQTT settings are currently selected
  with `option_env!`, which forces a rebuild for each configuration.

Important gaps must be treated as prerequisites rather than hidden inside a
transport adapter:

- The XIAO board has no committed persistent-storage partition and does not
  advertise the persistent-storage capability.
- No ESP32-C6 flash backend is connected to `embedded-sdk-storage`.
- The reference GATT service provides no pairing, link encryption, bonding,
  proof of possession, replay protection, or authorization.
- There is no physical-presence or factory-reset input policy.
- There is no stable provisioning wire format, transaction identifier, or
  redacted status model.
- CRC-protected storage detects accidental corruption but provides neither
  confidentiality nor authenticity for credentials.

## Scope

### In scope for the first complete slice

- A portable, `no_std`, allocation-free provisioning state model and service.
- One versioned product configuration containing:
  - Wi-Fi station SSID, authentication mode, and optional credential;
  - controlled network verification host and port when enabled;
  - development MQTT fixture host, port, and client identifier when enabled.
- A transport-neutral request/response protocol with bounded messages,
  transaction identifiers, request identifiers, explicit status, and
  idempotent retry behavior.
- Authority and physical-presence information supplied to the service by a
  transport/session adapter.
- Candidate validation before persistent state changes.
- Durable pending, confirmed, rejected, and rollback behavior.
- Reboot-to-apply semantics for the initial ESP32-C6 integration.
- A reviewed XIAO storage partition and an ESP32-C6 storage adapter.
- A fixture-only USB serial adapter suitable for automated HIL tests.
- Boot-time selection of the confirmed or pending runtime configuration.
- Host tests, storage interruption tests, cross-compilation, and HIL evidence.
- Secret-safe diagnostics that expose state, generation, and bounded error
  categories but never configuration values.

### Deferred until a security design is accepted

- Provisioning credentials over Bluetooth LE.
- Owner/mobile application flows and QR-code formats.
- Proof-of-possession enrollment and recovery-code policy.
- Production use of a cable without a trusted fixture or physical-presence
  gate.
- Confidential or authenticated credential storage without platform flash
  protection or an authenticated encryption design.

### Out of scope

- OTA, general remote device management, and arbitrary runtime configuration.
- Cloud account enrollment, tenant assignment, and production MQTT identity.
- Manufacturing private-key injection, certificate issuance, or secure-element
  support.
- Wi-Fi access-point/captive-portal provisioning.
- A mobile or desktop provisioning UI.
- A universal serialization framework for unrelated product state.
- Secure erase claims. Flash tombstones and whole-region clearing do not prove
  that previous credential bytes are irrecoverable.

## Required architecture decisions

Record an ADR before implementation that fixes the following boundaries:

1. Provisioning is one portable transaction service; transports do not own
   validation, persistence, activation, or rollback policy.
2. A transport authenticates a session and supplies an authority context. The
   portable service authorizes each operation from that context.
3. Product firmware owns the concrete configuration schema and the definition
   of successful activation. The SDK service owns transaction semantics.
4. `Commit` durably stages a candidate but does not claim the candidate is
   operational. Confirmation is a separate state transition after product
   verification succeeds.
5. The first ESP32-C6 implementation applies a candidate after reboot. Runtime
   reconfiguration may be added later without changing the transaction model.
6. The persistent format uses two value slots and one atomic state record so a
   previous confirmed generation remains addressable while a candidate is
   tested.
7. Persistent provisioning keys are permanent format identifiers and must not
   be renumbered after release.
8. The wire schema and persistent schema are independently versioned. A new
   wire operation must not implicitly rewrite stored records.
9. No custom cryptographic handshake is introduced. BLE provisioning remains
   disabled until a separate threat model selects reviewed authentication,
   encryption, replay, key-storage, and recovery mechanisms.

A separate board/partition decision should document the ESP32-C6 flash region,
alignment, ownership, bootloader and application boundaries, erase endurance,
and interaction with any future OTA layout.

## Responsibility boundaries

```text
serial framing ----\
                    +--> authenticated session --> provisioning service
BLE GATT/chunking --/                              |  authorize
                                                   |  decode/validate
factory fixture ---------------------------------> |  stage/commit
                                                   |  report state
                                                   v
                                            provisioning repository
                                              | state record
                                              | slot A
                                              | slot B
                                                   |
                                                   v
                                           product verifier/applicator
```

### Transport/session adapter

Each adapter owns only:

- byte framing, fragmentation, reassembly, and transport timeouts;
- transport-level authentication and session establishment;
- construction of an unforgeable in-process `SessionContext`;
- forwarding complete bounded requests to the provisioning task;
- mapping bounded responses to the transport;
- disconnect cleanup for uncommitted in-memory work.

An adapter must not parse Wi-Fi fields, call the configuration store directly,
or decide that a candidate is confirmed.

### Portable provisioning service

The service owns:

- request sequencing, transaction ownership, and idempotency;
- authority checks and physical-presence requirements;
- wire-version and operation validation;
- candidate decoding through the product schema contract;
- semantic validation before writes;
- state transitions and retry-safe responses;
- calls to the provisioning repository;
- redacted status and error categories.

The service must serialize mutation through one owner task. Multiple transports
may submit requests, but only one transaction can stage or commit at a time.
Read-only capability and status operations may remain available while another
session owns the transaction.

### Product configuration and verifier

Product firmware owns:

- the concrete bounded configuration fields and their encoding;
- cross-field validation beyond reusable SDK types;
- mapping confirmed fields into Wi-Fi, networking, and MQTT configuration;
- what constitutes successful verification;
- verification timeout and retry policy;
- whether a successful candidate can be applied live or requires reboot;
- which provisioning authorities may change product-specific fields.

The first XIAO policy should require Wi-Fi association and DHCPv4 readiness.
When a network verification endpoint is configured, its DNS and TCP probe must
also succeed. Development MQTT connectivity may be reported separately and
must not become a production-security claim.

### Provisioning repository

The repository owns the stable storage keys, record encoding, slot selection,
generation checks, recovery, and atomic metadata changes. It consumes a
`KeyValueStore` but does not depend on ESP32-C6 or a transport.

## Proposed package and API shape

Create `crates/provisioning` as `embedded-sdk-provisioning`. It remains
`no_std`, forbids unsafe code, and depends only on portable SDK crates and
reviewed allocation-free codec/support crates.

Suggested modules:

```text
crates/provisioning/src/
  lib.rs          public exports and limits
  authority.rs    session authority and operation policy
  protocol.rs     versioned request/response operations
  service.rs      transaction state machine and idempotency
  repository.rs   two-slot persistent state and recovery
  status.rs       redacted public status and error kinds
```

The exact Rust API should be proven with tests before stabilization, but the
boundary should resemble:

```rust,ignore
pub struct SessionContext {
    pub session_id: SessionId,
    pub authority: Authority,
    pub physical_presence: bool,
}

pub enum Authority {
    HilFixture,
    OwnerSetup,
    Factory,
}

pub enum Request<'a> {
    Capabilities,
    Status,
    Begin {
        request_id: RequestId,
        transaction_id: TransactionId,
        schema: SchemaVersion,
    },
    SubmitCandidate {
        request_id: RequestId,
        transaction_id: TransactionId,
        encoded: &'a [u8],
    },
    Validate {
        request_id: RequestId,
        transaction_id: TransactionId,
    },
    Commit {
        request_id: RequestId,
        transaction_id: TransactionId,
    },
    Abort {
        request_id: RequestId,
        transaction_id: TransactionId,
    },
    FactoryReset {
        request_id: RequestId,
    },
}

pub enum CommitDisposition {
    RebootRequired { pending_generation: u32 },
    ApplyScheduled { pending_generation: u32 },
}

pub trait ProvisioningCandidate: Sized {
    type DecodeError;
    type ValidationError;

    fn decode(version: SchemaVersion, bytes: &[u8])
        -> Result<Self, Self::DecodeError>;
    fn validate_for(
        &self,
        authority: Authority,
    ) -> Result<(), Self::ValidationError>;
}
```

IDs must be fixed-width nonzero wrappers. Status and response types must be
bounded and must not borrow or copy candidate bytes. Credential-bearing types
must not implement `Debug` or `Display`, and their RAM storage should be
zeroized when ownership ends where this can be done reliably.

The facade should export portable provisioning types as
`embedded_sdk::provisioning`. BLE, serial, Espressif, and board packages must
not become dependencies of the portable crate.

## Wire protocol

### Envelope

Every complete request and response should contain:

- protocol magic and wire schema version;
- message kind;
- request identifier;
- transaction identifier where applicable;
- payload length;
- bounded payload;
- integrity check appropriate to the transport/framing layer.

The codec must reject before mutation:

- unknown incompatible major versions;
- truncated or trailing data;
- payloads larger than the documented maximum;
- duplicate singular fields;
- invalid enum discriminants;
- zero or reserved identifiers;
- operations not valid in the current transaction state.

Use a reviewed `no_std` codec with explicit numeric field identifiers and
bounded decode. Do not use an unbounded `serde_json` representation on the
device, and do not make Rust struct layout the persistent or wire format. The
codec choice and maximum request/response sizes should be recorded in the ADR
after a small compile-size and malformed-input proof.

### Framing

Framing remains transport-specific:

- USB serial uses a resynchronizable frame delimiter plus length/integrity
  validation. Random boot logs and provisioning frames must be distinguishable.
- BLE uses GATT operations and explicit chunk offset/total-length metadata when
  a message exceeds the negotiated ATT payload. The adapter reassembles one
  bounded message before calling the service.
- A factory fixture may reuse serial framing but authenticates with a different
  authority policy.

Chunking is not part of the service state machine. Incomplete transport frames
expire without creating or changing a durable candidate.

### Idempotency

The service retains the most recent completed request identifier and redacted
response for the active transaction. Repeating the same request produces the
same logical result. Reusing an identifier with different bytes is rejected.

In particular:

- duplicate `Begin` does not create a second transaction;
- duplicate `SubmitCandidate` does not append another persistent value;
- duplicate `Commit` returns the existing pending generation;
- duplicate `Abort` succeeds without resurrecting state;
- repeated factory-reset requests converge on the unprovisioned state.

Only the minimum replay information required for crash recovery should be
persisted. The implementation must document which response retries survive a
device reboot.

## State model

Separate externally visible device state from the short-lived service
transaction state.

### Device state

```text
Unprovisioned
Provisioned { confirmed_generation }
PendingVerification { previous_generation?, pending_generation, attempts }
RollbackRequired { previous_generation?, rejected_generation, reason }
RecoveryRequired { reason }
ResetInProgress
```

### Transaction state

```text
Idle
Owned { session_id, transaction_id, schema }
CandidateReceived { ... }
CandidateValidated { ... }
CommitInProgress { ... }
Committed { pending_generation }
```

Rules:

1. Only the owning session may mutate or abort a transaction.
2. Disconnect before durable commit releases the transaction after a bounded
   timeout and zeroizes the in-memory candidate.
3. Disconnect after durable commit does not roll back the candidate. Boot-time
   verification owns the outcome.
4. A new candidate cannot overwrite the slot containing the confirmed
   generation.
5. Generation comparison must handle `u32` wrap explicitly or reserve wrap as
   a recovery condition; ordinary integer ordering is insufficient at wrap.
6. Corruption, unsupported persistent schema, and missing referenced slots
   enter `RecoveryRequired`; they must not trigger a silent factory reset.
7. Provisioning status never returns SSID, hostnames designated sensitive,
   client identifiers, credential lengths, hashes of low-entropy secrets, or
   raw decode failures containing input bytes.

## Persistent format and power-loss behavior

Reserve one permanent provisioning namespace with at least these records:

```text
STATE  = Key(namespace, 1)
SLOT_A = Key(namespace, 2)
SLOT_B = Key(namespace, 3)
```

Published numbers must be added to a repository key registry and never reused.

### Slot record

A slot contains:

- persistent-format magic and version;
- product configuration schema version;
- generation;
- encoded payload length and bounded payload;
- integrity metadata needed above the storage engine;
- no externally printable secret-derived identifier.

### State record

The atomic state record contains no credentials. It identifies:

- confirmed slot and generation, if any;
- pending slot and generation, if any;
- pending verification-attempt count;
- last bounded outcome/reason;
- state-record format version.

### Commit sequence

1. Decode and validate the complete candidate in RAM.
2. Select the slot that does not contain the confirmed generation.
3. Write and read-verify the complete candidate slot.
4. Atomically replace `STATE` to reference that slot as pending while retaining
   the previous confirmed slot.
5. Return `RebootRequired` with the pending generation.
6. Flush the response if possible, then reboot after a bounded grace period.

Power loss during step 3 leaves state pointing only to the old confirmed slot.
Power loss during step 4 exposes either the complete old state or complete new
pending state because it is one `KeyValueStore::put`. An orphan slot is ignored
until safely overwritten.

### Verification and confirmation

On boot with a pending generation:

1. Validate both state references and decode the pending slot.
2. Atomically increment the attempt count before starting an attempt.
3. Apply the candidate and run the product verifier with bounded deadlines.
4. On success, atomically replace `STATE` so pending becomes confirmed.
5. On failure, record a bounded reason and retry only within policy.
6. After the maximum attempts, atomically restore the previous confirmed slot.
7. If no previous generation exists, return to unprovisioned setup state while
   retaining a redacted failure reason.

The first implementation should prefer one attempt per boot and a small fixed
maximum so repeated brownouts cannot create an infinite pending boot loop.

Confirmation means that the configured connectivity checks passed at least
once. It does not guarantee future network availability or cloud delivery.

### Factory reset

Factory reset is a state-machine operation, not a raw call to `clear()`:

1. Authorize the operation using product policy and physical presence.
2. Atomically mark reset in progress.
3. Delete state and both slot records in a restartable order.
4. On an interrupted reset, resume deletion before accepting configuration.
5. Return to `Unprovisioned` and restart setup advertising/serial acceptance.

Documentation must state that logical deletion is not verified secure erase.

## Configuration schema

Define the initial XIAO product configuration in firmware or a focused product
module, not in the generic provisioning state machine. It should use existing
bounded SDK value types wherever possible.

Conceptually:

```rust,ignore
pub struct XiaoConfiguration {
    pub schema: SchemaVersion,
    pub wifi: WifiStationConfiguration,
    pub network_probe: Option<NetworkProbeConfiguration>,
    pub mqtt_fixture: Option<MqttFixtureConfiguration>,
}
```

Validation includes:

- supported schema major/minor;
- SSID and passphrase limits and valid authentication combinations;
- endpoint hostname and port limits;
- all-or-none MQTT fixture fields;
- an explicit opt-in marker for plaintext fixture MQTT;
- authority restrictions for each field group;
- total encoded size within the configured slot and transport limits;
- rejection of unknown security downgrades.

The persistent representation must distinguish an absent optional credential
from an empty credential and must never normalize invalid input into a valid
but unintended configuration.

Schema migration policy for the first release:

- read the current major version and compatible earlier minor versions;
- write only the current version;
- reject a newer minor or different major into `RecoveryRequired` unless an
  explicit tested migration exists;
- preserve the raw incompatible record for recovery rather than erasing it.

## Boot and firmware integration

The provisioning repository must be opened before Wi-Fi configuration is
selected.

Precedence during migration should be explicit:

1. valid pending configuration under verification;
2. valid confirmed persistent configuration;
3. development `option_env!` configuration only in an explicitly documented
   development/HIL compatibility mode;
4. otherwise unprovisioned mode.

Production builds must not silently fall back to compiled credentials. Once
the persistent path is proven, build-time Wi-Fi and endpoint secrets should be
removed from normal firmware configuration.

The initial reboot-to-apply flow avoids attempting to recover ownership of a
Wi-Fi controller and network runner after they have been split into long-lived
tasks. It also tests the real boot path. A later live-apply implementation may
coordinate task shutdown and restart, but it must preserve the same durable
pending/confirmed semantics.

Firmware task ownership should be:

- one storage/provisioning task owns the `KeyValueStore` and repository;
- transport tasks send bounded commands over an Embassy channel;
- the boot/apply coordinator reports verification success or failure to the
  provisioning task;
- no BLE, Wi-Fi, or MQTT task writes provisioning records directly.

## USB serial fixture adapter

The first adapter exists to exercise the production state machine in a
controlled lab. It must be visibly and mechanically restricted:

- compile only behind an additive `hil-provisioning` or similarly explicit
  feature;
- identify itself as `HilFixture` authority;
- accept commands only during an initial bounded window or while an explicit
  fixture/setup condition is asserted;
- use binary frames distinguishable from ordinary diagnostics;
- never echo candidate payloads or include them in errors;
- bound frame size, inter-byte timeout, transaction duration, and attempts;
- zeroize receive and candidate buffers after completion or abort.

The adapter is not evidence that arbitrary physical serial access is a safe
production provisioning mechanism. If production cable provisioning is later
required, it needs its own authenticated authority and physical-presence
policy while reusing the same service.

## Bluetooth LE adapter security gate

The existing bring-up GATT service must not be extended with credential-write
characteristics. Before implementing BLE provisioning, accept a threat model
and ADR covering:

- device authenticity and owner proof of possession;
- BLE pairing mode and link-encryption requirements;
- whether app-level authenticated key agreement is also required;
- replay protection and binding requests to a live session;
- storage and rotation of bonding/proof-of-possession material;
- setup-mode entry, timeout, retry throttling, lockout, and recovery;
- authorization after the device is already provisioned;
- factory reset and ownership transfer;
- privacy of advertisements and stable device identifiers;
- behavior when the phone disconnects during staging or after commit;
- security review and negative HIL cases.

Use reviewed protocol and cryptographic implementations. Do not invent a
custom key exchange or treat an encrypted BLE link without authenticated
ownership as sufficient authorization.

Once the gate is satisfied, the GATT adapter should expose capabilities,
request/chunk input, response/status, and notification characteristics. It
must reconstruct the same wire request accepted by serial and call the same
service API.

## Diagnostics

Define stable, versioned event classes such as:

```text
provisioning ready: state=unprovisioned
provisioning transaction started
provisioning candidate validated
provisioning candidate pending: generation=...
provisioning verification started: attempt=...
provisioning configuration confirmed: generation=...
provisioning configuration rejected: reason=...
provisioning rollback completed: generation=...
provisioning recovery required: reason=...
provisioning factory reset completed
```

Events may include protocol version, state, generation, attempt number,
authority class, and bounded error kind. They must not include candidate bytes,
SSID, passphrase, configured hostname, client identifier, BLE address, proof of
possession, secret length, or secret-derived digest.

## File-level change map

| File or directory | Planned change |
| --- | --- |
| `docs/adr/` | Add the transport-neutral provisioning and XIAO partition decisions. |
| `Cargo.toml` / `Cargo.lock` | Add the provisioning crate and reviewed bounded codec dependencies. |
| `crates/provisioning/` | Add protocol, authority, service, repository, status, and unit tests. |
| `crates/config/` | Reuse schema and validation contracts; add only generally reusable helpers justified by the product schema. |
| `crates/storage/` | Reuse atomic value operations; do not add provisioning-specific policy. |
| `crates/embedded-sdk/` | Export the portable provisioning package. |
| `boards/seeed/xiao-esp32c6/` | Own reviewed storage-region metadata and advertise persistence only after validation. |
| `ports/espressif/esp32c6/` | Implement or adapt the chip flash backend to ecosystem NOR traits without product policy. |
| `firmware/seeed/xiao-esp32c6/` | Add product schema, boot selection, verification coordination, serial adapter, and redacted events. |
| `tests/host/` | Cover facade availability and cross-crate policy where unit tests are insufficient. |
| `tests/hil/` | Add serial provisioning, reset, rollback, and interruption scenarios. |
| `tools/xtask/` | Add deterministic fixture orchestration only after the serial protocol is stable. |
| `docs/connectivity/` | Document setup flow, security boundary, recovery, and removal of build-time credentials. |
| `docs/compatibility/platforms.md` | Claim persistent/runtime provisioning only after HIL and security gates pass. |

## Implementation sequence

### 0. Decisions and feasibility

1. Accept the provisioning-boundary ADR.
2. Inspect ESP32-C6 flash APIs and prove a minimal ecosystem NOR adapter.
3. Define and review the XIAO partition map with room for future OTA decisions.
4. Select the bounded wire codec after size, `no_std`, malformed-input, and
   maintenance review.
5. Fix maximum candidate, request, response, and transport-frame sizes.

Exit criteria: storage ownership and protocol dependencies are explicit; no
code assumes that unused flash is safe to claim.

### 1. Portable model and pure state machine

1. Create `embedded-sdk-provisioning` with authority, IDs, states, operations,
   statuses, and error kinds.
2. Implement transition rules without hardware or wire decoding.
3. Add table-driven tests for every state/operation/authority combination.
4. Prove transaction exclusivity, timeout cleanup, and request idempotency.
5. Export the crate from the facade.

Exit criteria: all state transitions and redaction rules run on the host with
no allocator, transport, storage backend, or MCU dependency.

### 2. Persistent repository

1. Define permanent keys and versioned slot/state encodings.
2. Implement open/recover, stage, mark-pending, confirm, reject, rollback, and
   restartable reset over `KeyValueStore`.
3. Add a fault-injecting in-memory store that can interrupt every get/put/delete
   boundary.
4. Exhaustively test the documented old/new state outcomes.
5. Test corruption, stale/orphan slots, generation mismatch, incompatible
   versions, attempt exhaustion, and interrupted reset.

Exit criteria: no injected interruption loses a valid confirmed generation or
causes partial candidate activation.

### 3. Product configuration

1. Define the bounded XIAO configuration and independent persistent schema.
2. Reuse SDK Wi-Fi and MQTT validation rather than duplicating limits.
3. Implement deterministic encode/decode and golden vectors.
4. Ensure credential-bearing fields do not format and are zeroized in RAM.
5. Add malformed, boundary, compatibility, and security-downgrade tests.

Exit criteria: arbitrary byte input cannot panic, allocate without bound, leak
secrets through errors, or produce an unvalidated configuration.

### 4. XIAO persistent storage

1. Add the reviewed partition metadata to the board layer.
2. Implement the ESP32-C6 raw flash adapter behind ecosystem traits.
3. Connect `SequentialStore` with a measured scratch and page budget.
4. Cross-compile and measure firmware/RAM changes.
5. Run repeated write, compaction, reboot, and cut-power validation on hardware.

Exit criteria: the board can advertise persistent storage with checked-in
partition and interruption evidence.

### 5. Serial provisioning and boot application

1. Implement the fixture-only framed serial adapter and bounded command queue.
2. Open provisioning storage before selecting Wi-Fi configuration.
3. Persist a candidate, respond with pending generation, and reboot to apply.
4. Confirm after bounded Wi-Fi/IP/probe verification.
5. Roll back after bounded failure attempts and expose a redacted result.
6. Implement authorized factory reset and recovery-mode behavior.
7. Keep current build-time configuration only behind an explicit compatibility
   path during migration.

Exit criteria: one generic firmware image can be provisioned, rebooted,
confirmed, updated, rejected, rolled back, and reset without rebuilding it.

### 6. HIL automation

Automate at least:

1. blank device reports unprovisioned and accepts an authorized transaction;
2. valid configuration becomes pending, survives reboot, and confirms;
3. invalid and incomplete candidates cause no persistent mutation;
4. duplicate and reordered requests follow the idempotency contract;
5. transport disconnect before commit abandons only transient state;
6. transport disconnect after commit does not lose pending state;
7. wrong Wi-Fi credentials do not replace a previous working generation;
8. power loss during slot write, pending-state write, attempt update,
   confirmation, rollback, and factory reset recovers as specified;
9. schema incompatibility enters recovery without erasing evidence;
10. captured commands, serial logs, and CI artifacts contain no secrets;
11. repeated update/rollback cycles do not show unbounded heap or flash growth;
12. heartbeat and BLE remain responsive during flash operations and network
    verification, within documented latency bounds.

Exit criteria: results include firmware hash, board identity, partition
version, raw redacted event log, per-case timing, and pass/fail output.

### 7. BLE provisioning

Begin only after the BLE security gate is accepted:

1. implement authenticated session establishment and authorization context;
2. implement bounded GATT chunking/reassembly around the existing wire format;
3. forward requests to the same service task used by serial;
4. test disconnect/reconnect at every transaction stage;
5. run positive and negative pairing, proof-of-possession, replay, rate-limit,
   ownership-transfer, and reset cases;
6. compare serial and BLE provisioning outputs byte-for-byte at the stored
   configuration boundary.

Exit criteria: BLE introduces no alternate validation/storage path and passes
the accepted security and interoperability matrix.

## Host verification matrix

- Every operation in every service state.
- Every authority class with and without physical presence.
- Request-ID reuse with identical and different payloads.
- Transaction-ID collision and stale-session requests.
- Maximum and over-maximum request, response, and candidate sizes.
- Empty, truncated, extended, duplicate-field, and unknown-version messages.
- Credential and endpoint boundary values.
- Golden wire and persistent encoding vectors.
- Old/current/new schema compatibility.
- Repository recovery after interruption at every mutation boundary.
- Slot/state generation mismatch and orphan selection.
- Attempt-counter exhaustion and generation wrap policy.
- Interrupted and repeated factory reset.
- Assertions that debug/status/error output contains no candidate material.
- `no_std`, default-feature, facade, docs, formatting, lint, and target builds.

Property tests or fuzz targets should cover wire decoding and persistent record
decoding before either accepts field-originated input in a production build.

## Security acceptance criteria

The provisioning core is complete when:

- unauthenticated code cannot manufacture an elevated `SessionContext` through
  the public transport-neutral API;
- every mutating operation performs authorization before decoding or writing
  sensitive state where practicable;
- credential values cannot be formatted and are absent from normal diagnostics;
- request and candidate buffers are bounded and cleaned up after use;
- malformed input cannot panic, cause unbounded work, or mutate durable state;
- a failed candidate cannot destroy the previous confirmed generation;
- rollback and reset policies require explicit authorization;
- stored-data confidentiality and authenticity claims match the concrete board
  protection actually enabled;
- BLE remains disabled for credentials until its separate security gate passes;
- fixture-only serial support cannot be accidentally presented as an enabled
  production provisioning surface.

## Resource budgets to record

Before merging the hardware slice, record:

- maximum wire request, response, and reassembled candidate sizes;
- service channel depth and behavior when full;
- candidate, codec, storage scratch, and task stack/static RAM;
- flash partition size, erase size, usable record size, and expected endurance;
- firmware image-size delta;
- worst-case read, write, compaction, reset, and boot-recovery time;
- provisioning and verification timeouts;
- maximum pending boot attempts;
- flash-write impact on heartbeat, BLE, and network scheduling.

## Documentation and compatibility policy

Documentation must clearly distinguish:

- accepting a candidate from confirming that it works;
- transport encryption from authenticated provisioning authority;
- CRC corruption detection from confidentiality and authenticity;
- logical reset from secure erase;
- HIL fixture serial from a supported production cable interface;
- development plaintext MQTT configuration from production cloud identity;
- runtime provisioning availability from persistent-storage capability.

Do not update the XIAO compatibility row to claim provisioning until the
runtime configuration, persistent storage, reset, rollback, and HIL gates pass.
Do not claim secure BLE provisioning until its threat model and negative
hardware tests also pass.

## Definition of done for the initial serial slice

- One generic XIAO firmware image starts unprovisioned on blank supported flash.
- The fixture submits one bounded versioned configuration without rebuilding.
- The device validates it, stores it as pending, acknowledges only redacted
  metadata, and reboots.
- The device verifies connectivity and atomically confirms the generation.
- A later bad candidate automatically returns to the previous working
  generation after bounded attempts.
- Power interruption at every persistent transition produces a documented
  recoverable state.
- Authorized factory reset reliably returns the device to unprovisioned mode.
- Host, cross-compile, and automated HIL suites pass.
- No credential appears in logs, command displays, test reports, or uploaded
  artifacts.
- The storage partition, resource costs, limitations, and security boundaries
  are documented.

At that point the provisioning logic is ready for another authenticated
transport. It is not yet evidence that Bluetooth LE provisioning itself is
secure or production-supported.
