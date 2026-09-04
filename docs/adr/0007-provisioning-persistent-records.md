# ADR 0007: Provisioning Persistent Records

- Status: Accepted
- Date: 2026-09-04

## Context

Provisioning must retain the last confirmed product configuration while a new
candidate is verified. A single atomically replaced value cannot keep both
generations addressable, and raw Rust layouts are not stable persistent
formats. Recovery must also distinguish corruption and incompatible formats
from an intentionally blank device.

## Decision

Provisioning permanently owns storage namespace `0x0001` with these records:

| Record | Key | Contents |
| --- | --- | --- |
| State | `0x0001:0x0001` | credential-free slot references and transition state |
| Slot A | `0x0001:0x0002` | one complete versioned product candidate |
| Slot B | `0x0001:0x0003` | one complete versioned product candidate |

Record numbers are permanent and are registered in `docs/storage.md`.

The state format is a fixed 24-byte, big-endian record. It contains magic,
persistent-format version, state discriminant, optional confirmed and pending
slot/generation references, verification-attempt count, bounded rejection
reason, reserved bytes, and CRC-32. It contains no candidate fields.

A slot is a bounded, big-endian record containing magic, persistent-format
version, independently versioned product schema, nonzero generation, payload
length, reserved bytes, the complete opaque product payload, and CRC-32. The
maximum product payload is 1,024 bytes and the encoded slot overhead is 24
bytes. Reserved bytes must be zero on read.

The repository writes and read-verifies the non-confirmed slot before one
atomic state-record replacement makes it pending. Confirmation changes only
the state record. Generation allocation uses checked progression and treats
`u32::MAX` as recovery-required instead of wrapping.

Factory reset first writes a reset-in-progress state. It then idempotently
deletes slot A, slot B, and finally the state record. Reopening a repository
with the marker resumes deletion; an absent state record is unprovisioned and
ignores orphan slots.

CRC-32 supplements the storage engine's integrity checks for format-level
validation. It is not cryptographic authentication or encryption. Logical
deletion is not claimed to securely erase old flash bytes.

## Consequences

- A slot write interruption leaves the state pointing only to the previous
  confirmed generation; a completed state replacement exposes the complete
  pending generation.
- A candidate can be confirmed without rewriting credential bytes.
- Missing slots, generation mismatches, corruption, and incompatible
  persistent versions enter a redacted recovery-required state and preserve
  evidence.
- The maximum storage-engine value is 1,048 bytes, so an ESP32-C6
  `SequentialStore` scratch allocation must provide at least 1,053 bytes after
  its key and tag overhead.
- Record encodings need golden-vector and hardware interruption evidence before
  stabilization. Changes after release require explicit migration, not silent
  reinterpretation.

## Alternatives considered

- One active value was rejected because a candidate could overwrite the last
  known-working configuration before verification.
- Three independently updated metadata records were rejected because recovery
  could observe mixed references.
- Using only the storage engine CRC was rejected because the provisioning
  record still needs an independently validated format boundary.
- Clearing the whole partition for reset was rejected because interruption can
  leave a partially erased region without a durable restart marker.
