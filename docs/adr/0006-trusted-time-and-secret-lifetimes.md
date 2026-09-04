# ADR 0006: Trusted Time and Secret Lifetimes

- Status: Accepted
- Date: 2026-09-04

## Context

Azure TLS certificate validation and SAS token issuance both depend on trusted
wall-clock time. A device that has only monotonic uptime after reset cannot
determine whether a certificate or SAS credential is currently valid. Treating
an arbitrary build timestamp, unauthenticated network response, or zero-valued
clock as trusted would turn connectivity recovery into an authentication
bypass.

Cloud credentials also cross storage, signing, TLS, and MQTT boundaries.
Ordinary owned byte arrays are easy to format, clone, retain past expiry, or
leave in memory. The provider crate should consume credentials without owning
their persistence or exposing them in provider configuration and health
snapshots.

## Decision

1. `embedded-sdk-security` is a portable `no_std` capability crate. It does not
   implement a TLS stack, certificate parser, random-number generator, secure
   element, or persistent credential store.
2. Security-sensitive wall-clock consumers accept `TrustedTime`. Its provider
   either returns an explicit trusted Unix timestamp or fails closed with a
   reason such as untrusted, unavailable, or invalid persisted lower bound.
3. A platform time policy may combine a protected last-known-good lower bound
   with a network refresh, but time must never move backwards silently. The
   portable crate provides an anchored clock that advances a caller-validated
   Unix-time snapshot using monotonic uptime and rejects backwards movement.
   Snapshot acquisition, the concrete policy, and storage integrity remain
   firmware/platform concerns.
4. Time-bounded credentials carry a non-secret `CredentialLease` with separate
   issuance, refresh, and hard-expiry instants. Refresh begins before expiry;
   an expired credential cannot start a new connection.
5. Owned secret material uses fixed-capacity `SecretBytes`. Construction is
   fallible, debug output is always redacted, ordinary slice/display traits are
   intentionally absent, and the entire backing storage is zeroized on drop.
6. Secret access occurs only through an explicit scoped operation. Provider
   configuration, lifecycle state, errors, and snapshots contain no secret
   material.
7. Cryptographically secure randomness is requested through `SecureRandom`.
   Concrete hardware RNG composition and health requirements remain owned by
   the platform/firmware.
8. SAS signing, X.509 identity, trust-anchor, and opaque private-key traits are
   added only with their first concrete consumer and backend proof. This avoids
   freezing a generic crypto API before the TLS and signer libraries are
   selected.

## Consequences

- TLS and SAS work has one fail-closed trusted-time contract instead of
  accepting raw integers with unknown provenance.
- Credential refresh can be supervised independently from expiry and MQTT
  reconnect backoff.
- Fixed-capacity secrets are allocation-free and reduce accidental disclosure,
  but zeroization alone does not make ordinary RAM protected against physical
  access, DMA, debugger access, or copies made by downstream libraries.
- Platform integrations must document how trust is established after cold boot
  and how the persisted lower bound is protected against rollback.
- An externally supplied development SAS token remains experimental until the
  same trusted-time and secret-lifetime rules are enforced end to end.

## Alternatives considered

- Accepting `u64` timestamps directly in cloud APIs was rejected because the
  type cannot distinguish trusted wall-clock time from uptime or an unchecked
  network value.
- Fetching time inside the Azure provider crate was rejected because network,
  storage, platform clocks, and trust policy belong to firmware composition.
- Storing SAS tokens or private keys in `HubConfig` was rejected because that
  configuration is non-secret, copyable, and intentionally diagnostic-safe.
- Relying on redacted formatting without zeroization was rejected because
  formatting safety does not constrain memory lifetime.
- Designing one generic cryptography provider up front was rejected because
  symmetric HMAC, TLS certificate identity, and opaque asymmetric signing have
  different ownership and asynchronous behavior.
