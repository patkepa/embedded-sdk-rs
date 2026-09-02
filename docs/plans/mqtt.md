# MQTT Integration Plan

## Status

- Status: Proposed
- Branch: `feat/mqtt`
- Target: Seeed Studio XIAO ESP32C6
- Protocol: MQTT 5.0
- Primary outcome: add a bounded, recoverable MQTT client path above the
  existing `embassy-net` DHCPv4, DNS, and TCP stack

## Recommendation

Introduce MQTT as a protocol layer, not as ESP32-C6 or product logic. Keep the
portable configuration and lifecycle model in `embedded-sdk-mqtt`, isolate the
selected MQTT implementation in `embedded-sdk-mqtt-minimq`, and let firmware
compose DNS, TCP, transport security, credentials, tasks, and product topics.

Use `minimq` 0.13 as the initial implementation candidate. It matches the
workspace particularly well because it is `no_std`, does not require an
allocator, uses caller-owned packet buffers, implements MQTT 5, and consumes
`embedded-io-async` 0.7 streams. `embassy-net::tcp::TcpSocket` implements that
same I/O contract. `minimq` also makes reconnect and broker-session behavior
explicit instead of hiding it in an internal runtime.

The dependency is still pre-1.0. Its types must therefore remain behind the
adapter boundary rather than spreading through the facade, firmware business
logic, or cloud APIs. Pin the selected release in `Cargo.lock` and review each
minor-version update as a potentially breaking change.

Raw MQTT on TCP port 1883 is acceptable only for a local, isolated test fixture
with no reusable credentials or sensitive payloads. It is not a supported
deployment mode. MQTT must not be marked supported in the compatibility matrix
until broker authentication, confidentiality, integrity, and credential
handling have passed the security gate below.

## Preconditions and explicit non-goals

The existing networking slice already provides the prerequisites that MQTT
should reuse:

- recoverable Wi-Fi association;
- DHCPv4 and DNS readiness;
- an `embassy-net` TCP socket;
- fixed socket and TCP-buffer budgets;
- independent Wi-Fi, network-runner, network-monitor, BLE, and heartbeat tasks.

This slice must not:

- put MQTT code in the ESP32-C6 port or board package;
- define another IP stack, DNS resolver, or universal SDK socket trait;
- embed broker credentials, private keys, or production endpoints in source;
- claim that MQTT QoS is durable across power loss before persistent outbound
  storage exists;
- make MQTT failure restart Wi-Fi, BLE, or the executor;
- define AWS IoT, Azure IoT, or another provider's topic layout in the protocol
  crate;
- add OTA, fleet provisioning, device shadows, or command authorization under
  the MQTT feature name.

## Architecture decisions to record first

Add an ADR before implementation. It should record these decisions:

1. MQTT is an application protocol over any ordered async byte stream. The
   portable API depends on protocol concepts, not `embassy-net`, Espressif, or
   a board.
2. The SDK wraps an established implementation instead of reimplementing MQTT
   framing and session rules.
3. MQTT 5 is the initial wire protocol. Supporting MQTT 3.1.1 later must be an
   additive capability with broker interoperability tests.
4. Connection recovery has separate domains: Wi-Fi association, IP/DNS,
   transport connection, TLS, MQTT session, and application publish state.
   Failure at one layer must not be reported as failure at another.
5. MQTT protocol QoS and device-side durable delivery are different promises.
   QoS 1 can cover a live or resumed MQTT session, but power-loss-safe delivery
   requires an SDK-owned persistent outbox.
6. Protocol crates do not own production topic layouts or payload schemas.
   Those belong to a provider adapter or firmware product.
7. Buffer sizes, maximum packet size, subscriptions, in-flight operations, and
   channel depth are compile-time resource contracts.
8. Plaintext transport remains test-only. The production support gate requires
   authenticated encryption and a documented trust/credential lifecycle.

## Proposed packages and APIs

### `embedded-sdk-mqtt`

Create `crates/mqtt`. Keep it `no_std`, allocation-free, and independent of an
MCU, IP stack, executor, and concrete MQTT client.

It should own only stable SDK concepts:

- bounded broker hostname, client identifier, topic name, and topic filter;
- validated port, keepalive, session-expiry, and maximum-packet settings;
- `QoS`, `ConnectionState`, and backend-independent `ErrorKind` values;
- an MQTT-specific bounded reconnect policy with deterministic jitter input;
- lifecycle snapshots and counters suitable for health and telemetry;
- validation that rejects empty identifiers, invalid topic/filter syntax,
  wildcards in topic names, zero capacities, and values above documented
  limits.

Credentials must not be a public string field on the portable configuration.
The initial adapter should accept credentials at composition time through a
borrowed, short-lived value. Secret-owning types must zeroize their storage and
must never implement `Display` or expose it through diagnostic snapshots.

Do not define a general `MqttClient` trait merely to hide one dependency. Add a
trait only when a second implementation or a host fake demonstrates a stable
consumer need. The portable facade may export `embedded_sdk::mqtt`; it must not
export `minimq` types.

### `embedded-sdk-mqtt-minimq`

Create `crates/mqtt-minimq`. It depends on `embedded-sdk-mqtt`, `minimq`, and
the standard async I/O traits, but not on `embassy-net`, an MCU HAL, a board, or
product topics.

The adapter should:

- translate validated SDK configuration into `minimq` session configuration;
- accept an already-connected `embedded_io_async::Read + Write` transport;
- retain caller-owned RX/TX packet buffers across reconnects;
- expose SDK connection events and normalized errors while preserving enough
  backend detail for diagnostics;
- support QoS 0 and QoS 1 in the first vertical slice;
- support subscribe, unsubscribe, receive, keepalive polling, graceful
  disconnect, and MQTT session resumption;
- keep all waits cancellable or externally timeout-bounded;
- return explicit capacity errors instead of truncating topics or payloads.

QoS 2, topic aliases, request/reply, and multiple in-flight publishes remain
out of the first vertical slice unless the product use case requires them.
Their absence must be represented as a documented capability boundary, not as
a silent downgrade.

### Transport security boundary

MQTT must accept a generic established byte stream so plain TCP and TLS use the
same protocol path. A likely TLS candidate is `embedded-tls` 0.19 because it is
TLS 1.3, `no_std`, no-allocation, async, and also uses `embedded-io-async` 0.7.
Treat that as a candidate requiring a focused proof, not a decision made only
from API documentation.

The TLS proof must demonstrate on ESP32-C6:

- cryptographically strong randomness from the hardware RNG;
- SNI and broker-hostname verification;
- verification against an explicitly provisioned trust anchor;
- a documented clock policy for certificate validity checks;
- bounded TLS record buffers and handshake timeout;
- clean composition of the resulting TLS stream with the MQTT adapter;
- broker certificate rotation without a firmware recovery dead end;
- rejection of an unknown CA, wrong hostname, expired certificate, and
  tampered handshake.

`NoVerify`, an all-trusting certificate verifier, or logging secrets is never
allowed in a production feature. If reliable certificate-time validation
cannot yet be provided, use a private fixture to validate the MQTT protocol and
leave production MQTT support blocked. Do not silently replace authentication
with encryption-only transport.

The first authenticated fixture may use username/token authentication inside
verified TLS. Mutual TLS and secure-element-backed private keys are valuable
follow-ups, but should not be claimed until identity provisioning and key
storage exist.

## Reference firmware composition

Add a dedicated MQTT service task to
`firmware/seeed/xiao-esp32c6/src/main.rs`. The task should own the MQTT session,
packet buffers, TCP/TLS transport, broker reconnect policy, and broker-facing
diagnostics.

Its state machine should be explicit:

```text
disabled
  -> wait for IP and DNS
  -> resolve broker
  -> connect TCP
  -> authenticate TLS
  -> connect MQTT / resume session
  -> subscribe and run
  -> back off after transport, TLS, or MQTT failure
  -> wait for IP again after network loss
```

The service must wait on the existing network readiness model. DNS or broker
failure must not force Wi-Fi reassociation. Link or lease loss should cancel
the active transport promptly, preserve only the session state the adapter can
honestly replay, and return to network readiness waiting.

Use a bounded `embassy-sync` channel between producers and the MQTT task.
Producers enqueue application messages or structured telemetry without owning
the MQTT connection. Define the full-queue policy per message class:

- replace or drop old best-effort telemetry and increment a drop counter;
- reject commands/replies that cannot be queued rather than pretending they
  were accepted;
- never block heartbeat, BLE, or safety-critical work indefinitely;
- add persistent storage only in a separate durable-outbox phase.

Start with one controlled telemetry publish and one command subscription. Use
fixture-only topics such as `embedded-sdk/test/{client-id}/telemetry` and
`embedded-sdk/test/{client-id}/commands`. This proves both directions without
turning the protocol crate into a cloud-provider API. Payload bytes should be
versioned and covered by golden test vectors; selecting JSON, CBOR, or another
encoding is a separate schema decision.

Configure the reference firmware through explicit development inputs:

- `MQTT_HOST` and `MQTT_PORT`;
- a non-secret `MQTT_CLIENT_ID` or a deterministic identifier derived through
  a documented privacy-preserving scheme;
- optional credential inputs that are never emitted by `xtask` or logs;
- a separately named, explicit fixture-only switch for plaintext MQTT.

Partial configuration, empty values, port zero, an invalid topic, or a request
for credentials over plaintext must fail closed while leaving Wi-Fi, BLE, and
heartbeat operational.

## Resource budget

Do not guess buffer sizes. Derive them from the largest accepted MQTT packet
and TLS record, then enforce that maximum in configuration and broker tests.
Record at least:

- MQTT RX packet buffer;
- MQTT TX/replay buffer;
- TCP RX and TX buffers;
- TLS read and write record buffers;
- outbound channel depth and per-entry payload capacity;
- maximum topic, client ID, hostname, and credential lengths;
- maximum subscriptions and in-flight QoS 1 publishes;
- firmware flash delta, static RAM delta, peak heap, and task allocation cost.

The current network stack reserves three sockets for DHCP, DNS, and the
controlled TCP probe. MQTT may reuse the probe's TCP slot only when the probe
has completed and cannot overlap it. Otherwise increase the stack socket count
to four and account for the memory delta. DNS resolution and the long-lived
MQTT TCP connection also need to coexist.

TLS record buffers may dominate RAM. Measure realistic broker handshakes before
selecting constants or enlarging the existing 96 KiB heap. Prefer static,
caller-owned resources and document any unavoidable heap allocation.

## File-level change map

| File or directory | Planned change |
| --- | --- |
| `Cargo.toml` | Add the two SDK packages, `embedded-io-async` 0.7, and a reviewed `minimq` 0.13 dependency with defaults disabled where applicable. Add TLS dependencies only after the proof. |
| `Cargo.lock` | Pin and review MQTT/TLS dependencies, licenses, advisories, and duplicate Embassy/heapless versions. |
| `crates/mqtt/` | Portable bounded configuration, lifecycle, errors, retry policy, documentation, and unit tests. |
| `crates/mqtt-minimq/` | Concrete MQTT 5 adapter over generic async byte streams and caller-owned buffers. |
| `crates/embedded-sdk/` | Export only the portable MQTT package through the facade. |
| `crates/telemetry/` | Add only the envelope or queue-facing concepts proven necessary; do not make MQTT the telemetry abstraction. |
| `firmware/seeed/xiao-esp32c6/` | Compose DNS, TCP, optional verified TLS, MQTT task, bounded channels, fixture topics, and reconnect policy. |
| `tests/host/` | Cover facade access, validation, state transitions, error normalization, and retry behavior. |
| `tests/integration/` | Add broker interoperability tests with an ephemeral local MQTT 5 broker. |
| `tests/hil/` | Add ESP32-C6 broker, TLS, recovery, coexistence, and resource scenarios. |
| `tools/xtask/` | Add deterministic broker-test commands only when credentials can be redacted and a local fixture can be controlled. |
| `docs/adr/0003-portable-mqtt.md` | Record protocol, dependency, ownership, QoS/durability, and security decisions. |
| `docs/connectivity/mqtt.md` | Document configuration, states, topic/payload boundary, limits, recovery, and security status. |
| README, porting guide, and compatibility matrix | Advertise only the validation level actually achieved. |

## Implementation sequence

### 1. ADR and dependency proof

- Record the decisions above.
- Compile a minimal `minimq` session over an `embassy-net::TcpSocket` using the
  pinned workspace toolchain.
- Verify API cancellation behavior, partial read/write behavior, MSRV,
  licenses, `no_std`, and duplicate dependencies.
- Run a host connection against a local MQTT 5 broker before designing the
  adapter API.

This checkpoint may replace `minimq` if the proof exposes an unacceptable
constraint. Compare at least `rust-mqtt` on protocol coverage, cancellation,
buffer ownership, maintenance, and adapter complexity before changing the
selection.

### 2. Portable model

- Implement `embedded-sdk-mqtt` configuration and lifecycle types.
- Add bounded topic/client/host validation and reconnect policy tests.
- Export the package from the facade.
- Keep secrets and backend types out of all snapshots and display output.

### 3. MQTT adapter and host interoperability

- Implement `embedded-sdk-mqtt-minimq` over generic async I/O.
- Test fragmented reads/writes, disconnects during every handshake phase,
  malformed broker packets, oversized packets, broker rejection, keepalive,
  QoS 0/1, subscription restoration, and session resumption.
- Run integration tests against an ephemeral local broker; do not depend on a
  public internet broker in CI.

### 4. ESP32-C6 plaintext fixture proof

- Add the MQTT task, buffers, channel, DNS/TCP setup, and explicit reconnect
  state machine.
- Enable raw TCP only behind the fixture-only configuration gate.
- Publish a non-sensitive versioned test payload and receive a controlled
  command.
- Verify AP loss, broker loss, DNS failure, and recovery without affecting BLE
  or heartbeat.

This checkpoint proves protocol integration but does not satisfy the production
security acceptance criteria.

### 5. TLS and credential gate

- Complete the TLS feasibility proof and ADR decisions for trust anchors,
  hostname checking, certificate time, credential provisioning, and rotation.
- Replace the fixture TCP stream with the verified TLS stream without changing
  the MQTT protocol adapter.
- Prove negative certificate and authentication cases on host and hardware.
- Remove any route that could send credentials over plaintext.

### 6. Structured telemetry and durability boundary

- Define a versioned, bounded telemetry envelope with golden vectors.
- Connect `embedded-sdk-telemetry::Event` producers through a bounded queue.
- Document drop/coalescing metrics and payload schema evolution.
- If delivery across reboot is required, add a separately reviewed persistent
  outbox using `KeyValueStore`; do not imply it from MQTT QoS alone.

### 7. Hardware evidence and documentation

- Run the complete HIL matrix below.
- Record image size, static memory, peak heap, reconnect latency, buffer
  high-water marks, queue drops, and 20-cycle recovery evidence.
- Update the compatibility matrix only after the secure path passes.

## Verification matrix

### Host and compile checks

- `cargo xtask check`
- `cargo xtask build-xiao-esp32c6`
- `cargo tree -d`
- portable packages compile with default features and remain `no_std`;
- public APIs are documented and clean under `-D warnings`;
- broker integration tests cover MQTT 5 CONNACK rejection, QoS 0/1 publish,
  subscribe/receive, keepalive, graceful disconnect, abrupt disconnect,
  session present/absent, broker restart, and packet-size limits;
- TLS tests reject an unknown CA, wrong hostname, expired certificate, and bad
  credentials without exposing their values.

### Hardware-in-the-loop checks

Use a controlled access point, DNS record, MQTT 5 broker, TLS identity, and BLE
central. Capture no secrets in commands, logs, or artifacts.

1. Boot without MQTT configuration: existing networking, heartbeat, and BLE
   behavior is unchanged.
2. Invalid or partial MQTT configuration: MQTT remains disabled with a bounded
   configuration error and no task panic.
3. Valid secure configuration: DNS, TCP, TLS, MQTT CONNECT, subscription, and
   one publish all succeed in order.
4. Broker identity failures: wrong hostname, unknown CA, expired certificate,
   and invalid credential are rejected and retried with bounded backoff.
5. QoS 0 and QoS 1: broker-observed delivery matches the documented live-session
   guarantee; no power-loss durability claim is made.
6. Incoming command: a maximum-size allowed packet is accepted and an oversized
   or malformed packet is rejected without memory corruption or task death.
7. AP loss and restoration: MQTT disconnects, IP readiness clears, and the full
   stack recovers without reboot.
8. Broker restart and DNS/TCP refusal: MQTT reconnects without forcing Wi-Fi
   reassociation.
9. Twenty repeated recovery cycles: no allocator growth, socket leak, stalled
   keepalive, task allocation failure, or reconnect storm.
10. BLE coexistence: GATT reads and notifications remain responsive during TLS
    handshakes, MQTT traffic, and reconnection.
11. Backpressure: a full outbound queue follows the documented drop/reject
    policy and reports counters without blocking unrelated tasks.
12. Log audit: no SSID, password, token, private key, client certificate,
    hostname configured as sensitive, device address, or payload content leaks.

## Acceptance criteria

The MQTT slice is complete only when all of the following are true:

- portable code can configure and observe MQTT lifecycle without importing
  Embassy, Espressif, or `minimq`;
- the concrete adapter exchanges MQTT 5 messages over any compatible async byte
  stream with fixed caller-owned buffers;
- the XIAO ESP32C6 securely connects to a controlled broker, publishes QoS 0/1,
  receives a subscription, services keepalive, and reconnects after network or
  broker loss;
- broker identity and credentials are verified without an all-trusting TLS
  mode or plaintext credential path;
- every buffer, packet, subscription, queue, and in-flight limit is documented
  and tested at its boundary;
- MQTT degradation does not stop Wi-Fi recovery, BLE, heartbeat, or the
  executor;
- host, integration, cross-compile, and HIL checks pass;
- resource deltas and security limitations are recorded;
- documentation distinguishes protocol QoS, broker session recovery, queue
  backpressure, and true power-loss-safe delivery.

## Follow-up order

After the first secure MQTT slice:

1. persistent credential provisioning and rotation;
2. power-loss-safe telemetry outbox with wear and capacity policy;
3. provider-neutral command/reported-state services;
4. AWS IoT, Azure IoT, or custom-cloud topic and payload adapters;
5. secure-element-backed identity and mutual TLS where required;
6. MQTT-driven OTA notification integrated with an independently secured,
   signed update pipeline.

## Primary references

- OASIS MQTT Version 5.0 standard, especially session state, QoS, keepalive,
  Last Will, packet-size negotiation, and security guidance.
- `minimq` 0.13 documentation and source, especially its `Connection`, buffer,
  reconnect, cancellation, and `embedded-io-async` contracts.
- `embassy-net` TCP socket documentation for its async embedded-I/O contract.
- `embedded-tls` 0.19 documentation and source for TLS 1.3, verifier, clock,
  record-buffer, and feature limitations.
