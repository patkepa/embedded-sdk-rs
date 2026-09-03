# ADR 0003: Portable MQTT Boundary

- Status: Accepted
- Date: 2026-09-03

## Context

The SDK has recoverable Wi-Fi, DHCPv4, DNS, and TCP, but no application
protocol above an ordered byte stream. MQTT must remain reusable across IP
stacks and hardware targets, and its delivery guarantees must not be confused
with device-side persistence. A pre-1.0 protocol implementation also should
not become part of the stable SDK facade.

## Decision

1. `embedded-sdk-mqtt` owns allocation-free broker, client, topic, QoS,
   lifecycle, normalized-error, and reconnect concepts. It has no executor,
   network-stack, MCU, or MQTT implementation dependency.
2. `embedded-sdk-mqtt-minimq` adapts `minimq` 0.13 to the portable model over
   any established `embedded-io-async` 0.7 `Read + Write` stream. `minimq`
   types are not exported by the `embedded-sdk` facade.
3. MQTT 5 is the initial wire protocol. QoS 0 and QoS 1 are exposed. QoS 2 is
   intentionally absent rather than silently downgraded.
4. Caller-owned RX and TX buffers live for the full client session and survive
   transport reconnects. Buffer exhaustion is an explicit capacity error.
5. Wi-Fi association, IP/DNS readiness, TCP, TLS, MQTT session, and
   application queue state are separate failure and recovery domains.
6. QoS 1 covers MQTT session acknowledgement and replay. It does not promise
   delivery across power loss; that requires a separately designed persistent
   outbox.
7. Protocol crates do not own production topics or payload schemas. The XIAO
   firmware uses fixture-only topics and a small versioned proof payload.
8. Credentials are borrowed at adapter composition time, redacted from debug
   output, and rejected for plaintext-fixture sessions. No credentials are
   stored in portable configuration or lifecycle snapshots.
9. Plain TCP is available only through an explicit fixture mode. Production
   support remains blocked until authenticated TLS, trust-anchor and clock
   policy, credential provisioning, certificate rotation, and negative tests
   are complete.

## Consequences

- Portable applications can validate configuration and observe MQTT health
  without importing Embassy, Espressif, or `minimq`.
- Firmware owns DNS, TCP/TLS construction, timeouts, tasks, queues, product
  topics, and secret provisioning.
- `minimq` updates are reviewed as potentially breaking and pinned in
  `Cargo.lock`.
- Session resumption retains in-flight MQTT state only while RAM survives.
- Secure MQTT is not listed as supported until the TLS and hardware gates pass.

## Alternatives considered

- Reimplementing MQTT was rejected because framing, acknowledgement, replay,
  and session rules already have a maintained allocation-free implementation.
- A general SDK socket or MQTT-client trait was rejected because one adapter
  does not yet demonstrate a stable abstraction need.
- Putting MQTT in the ESP32-C6 port was rejected because MQTT is neither
  chip-specific nor Wi-Fi-specific.
- Treating QoS 1 as a durable telemetry outbox was rejected because broker
  acknowledgement cannot preserve messages across device power loss.
