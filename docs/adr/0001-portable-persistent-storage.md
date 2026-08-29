# ADR 0001: Portable Persistent Storage Boundary

- Status: Accepted
- Date: 2026-08-29

## Context

The SDK needs persistent configuration, provisioning state, crash records,
offline telemetry, and future OTA metadata. Those uses sit at different levels:
raw flash access is hardware-specific, key/value recovery is storage-engine
behavior, and value schemas belong to application components. Combining them
would tie portable code to one MCU, partition format, or serializer.

NOR flash also has failure modes that a simple `read`/`write` map hides: erase
granularity, one-way bit transitions, limited endurance, interrupted writes,
corrupt records, and compaction. A new in-tree flash log would require a much
larger fuzzing and hardware validation effort than this repository currently
has.

## Decision

1. Raw flash uses the standard `embedded-storage` and
   `embedded-storage-async` NOR traits. Platform ports implement those traits;
   the SDK does not create competing raw-device interfaces.
2. The portable application boundary is an asynchronous, allocation-free
   `KeyValueStore` over complete byte values. It has `get`, `put`, idempotent
   `delete`, and destructive `clear` operations.
3. Keys are stable 32-bit identifiers split into a 16-bit component namespace
   and 16-bit record number. This bounds key memory and makes ownership
   explicit. Products maintain the registry.
4. `SequentialStore` delegates log structure, CRCs, repair, compaction, and
   wear leveling to `sequential-storage` 8.x. It uses a compile-time scratch
   buffer and its uncached index by default.
5. SDK records contain a one-byte tag after the four-byte key: `0` is a
   tombstone and `1` precedes a live opaque value. Tombstones make deletion
   available on ordinary `NorFlash`, without requiring multi-write behavior.
6. `put` and `delete` completion is atomic at the logical-record level. After
   an interruption, recovery may select the complete old or complete new
   value; the outcome of the interrupted operation is intentionally not
   promised. Whole-region `clear` is not atomic.
7. Board packages own partition selection. A platform must not advertise
   `PERSISTENT_STORAGE` merely because the chip contains flash.
8. Serialization, schema migration, encryption, authentication, secure erase,
   filesystems, queues, and cross-task scheduling remain separate concerns.

## Consequences

- Portable services can be tested against any conforming NOR flash and moved
  between vendors without changing their persistence contract.
- The store has no allocator requirement and its principal working-memory cost
  is visible in the type.
- Reads use less RAM but may scan multiple pages. Caching can be added later as
  an opt-in implementation without changing the `KeyValueStore` contract.
- Deleted secrets can remain physically recoverable until an erase. Products
  requiring confidentiality must add a protected backend and cryptographic
  format rather than infer protection from this API.
- A `sequential-storage` major upgrade is an on-flash compatibility event even
  if the SDK Rust API remains source-compatible.
- Each concrete board still needs a reviewed partition layout, compile checks,
  and hardware fault-injection evidence before claiming storage support.

## Alternatives considered

- A bespoke SDK log was rejected because power-failure correctness, repair,
  wear leveling, and fuzzing would duplicate an established implementation.
- String keys were rejected because they add variable key storage, backend
  length differences, and runtime collision policy. Numeric keys make the
  persistent registry explicit.
- Typed serialization in the storage crate was rejected because value schema
  evolution belongs to the owning component and no single codec fits secrets,
  telemetry, configuration, and update metadata.
- A filesystem was rejected as the universal boundary. Filesystems remain
  useful for large, named, or streaming blobs, but are unnecessary overhead for
  small persistent state and do not replace transactional record semantics.
