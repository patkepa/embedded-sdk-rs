# ADR 0008: Use a constrained rustls TLS 1.2 adapter experimentally

- Status: Accepted for experimental implementation
- Date: 2026-09-04
- Scope: Azure IoT Hub TLS transport proof

## Context

The classic Azure IoT Hub device endpoint requires a production-capable TLS
1.2 client with SNI, DNS-name verification, certificate-chain and validity
checking, platform randomness, and cipher suites compatible with Azure's RSA
server certificates. The MQTT 3.1.1 backend consumes an
`embedded-io-async` byte stream, so TLS must expose the same traits without
depending on Embassy or a particular TCP socket.

The first plan assumed that the complete cloud path could avoid a global
allocator. The MQTT and Azure provider layers meet that constraint, but the
reviewed TLS 1.2 candidates do not. In particular:

- `embedded-tls` 0.19 is allocation-free but implements TLS 1.3, not the TLS
  1.2 baseline required by the classic endpoint;
- `mbedtls` can run without `std`, but requires allocation and does not expose
  an async embedded-I/O client without a Tokio/std adapter;
- other small TLS candidates reviewed during the proof either did not support
  the required protocol/verification behavior or used an incompatible
  license;
- `rustls` 0.23.37 provides a `no_std` unbuffered connection API and TLS 1.2,
  while `rustls-rustcrypto` 0.0.2-alpha provides pure-Rust, `no_std` TLS 1.2
  cipher suites that compile for `riscv32imac-unknown-none-elf`.

Both rustls and the RustCrypto provider require `alloc`. The current XIAO
firmware already configures a global allocator, but this changes the resource
and failure model and therefore cannot be described as allocation-free.
`rustls-rustcrypto` is also an alpha dependency and obtains randomness through
the process-wide `getrandom` integration rather than an injected SDK trait.

## Decision

Add `embedded-sdk-tls-rustls` as an experimental concrete backend, not as a
portable facade dependency. It:

- enables TLS 1.2 only for the classic Azure endpoint;
- restricts negotiation to
  `TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256` and
  `TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384`;
- constructs the connection with a DNS `ServerName`, preserving both SNI and
  certificate hostname verification, and rejects IP-address identities;
- requires a non-empty explicit root store and a trusted Unix-time snapshot;
- exposes `embedded-io-async` `Read` and `Write` over a generic byte stream;
- keeps incoming TLS records, outgoing TLS records, and decrypted spill bytes
  in caller-owned fixed buffers with explicit capacity errors;
- disables client authentication for the initial SAS-token slice;
- never installs a dummy random-number handler. The final firmware must bind
  `getrandom` to the ESP32-C6 hardware RNG and prove failure behavior;
- is not re-exported by the portable `embedded-sdk` facade.

The adapter's caller-owned record buffers bound those buffers, but they do not
bound all heap activity inside rustls, certificate parsing, or RustCrypto.
Connection construction and handshakes may allocate. Allocation failure,
peak heap, fragmentation, stack use, and repeated reconnect behavior are
release gates.

## Consequences

This decision provides a compilable path to the required TLS 1.2 protocol
without coupling MQTT or Azure logic to a platform socket. Certificate and
hostname failures remain distinguishable from transport and capacity errors,
and trust anchors can be replaced without pinning Azure leaf certificates.

It also means the earlier acceptance criterion “without a global allocator
requirement” is not currently achievable for the selected TLS path. The
criterion is replaced by an explicit, measured allocator budget for the
experimental XIAO firmware. Provider and MQTT crates remain allocation-free.

The backend must not be promoted beyond experimental until all of the
following pass:

1. **Completed:** host success and negative tests for trust, hostname,
   certificate time, cipher mismatch, truncation, and corrupted records;
2. continued regression coverage for the implemented generic MQTT 3.1.1
   CONNECT and QoS 1 PUBLISH/PUBACK exchange through this stream;
3. an ESP32-C6 hardware RNG binding with startup and continuous failure tests
   (the opt-in port adapter and `getrandom` bridge compile; final firmware
   registration and HIL entropy evidence remain);
4. successful target link, on-device handshake, reconnect, and cancellation;
5. measured peak heap, heap fragmentation, stack, flash, record buffers, and
   handshake latency under concurrent firmware workloads;
6. dependency/license audit and an explicit decision whether an alpha crypto
   provider is supportable;
7. a separate X.509 client-auth proof. The current configuration is
   server-authenticated SAS only and does not prove opaque-key support.

## Revisit conditions

Re-evaluate this backend if a maintained allocation-free TLS 1.2 implementation
meets the verification and async-I/O requirements, if Azure's TLS 1.3 device
endpoint becomes generally available for the intended product, or if the
RustCrypto provider cannot meet maintenance, RNG, memory, or X.509 gates.
