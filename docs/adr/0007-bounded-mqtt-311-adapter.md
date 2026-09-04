# ADR 0007: Bounded In-Tree MQTT 3.1.1 Adapter

- Status: Accepted
- Date: 2026-09-04
- Supersedes: the MQTT reimplementation constraint in ADR 0003 for the
  explicitly bounded MQTT 3.1.1 subset below

## Context

Azure IoT Hub requires MQTT 3.1.1, while the existing `minimq` adapter emits
MQTT 5. A backend proof evaluated maintained embedded alternatives before
introducing another protocol implementation:

- `mqtt-async-embedded` 1.0.0 emits MQTT 3.1 (`MQIsdp`, protocol level 3) on
  its `V3` path and leaves required publish, subscribe, receive, and
  acknowledgment behavior incomplete;
- `mqtt-protocol-core` 0.7.9 requires `alloc`;
- `shiguredo_mqtt` requires a newer Rust toolchain than this workspace;
- `embassy-mqtt-lite` is QoS-0-only and its license does not fit this
  dual-licensed SDK;
- `rust-mqtt` 0.5 implements MQTT 5 rather than MQTT 3.1.1; and
- `mqttrust` 0.6 builds without `std`, but its encoder drops caller-supplied
  QoS 1 packet identifiers, its decoder can panic on a short CONNACK, and its
  SUBACK representation does not retain the broker's return code.

Adapting any of these issues locally would create a larger, less visible fork
while still requiring a new asynchronous stream adapter and manual inbound
acknowledgment policy. Azure work cannot proceed safely on a backend that
mis-encodes or loses required protocol state.

## Decision

1. Add `embedded-sdk-mqtt-v311`, an experimental, allocation-free adapter over
   `embedded-io-async` byte streams.
2. Limit its wire surface to the packets required by the Azure device path:
   CONNECT/CONNACK, QoS 0/1 PUBLISH, PUBACK, one-filter
   SUBSCRIBE/SUBACK, PINGREQ/PINGRESP, and client DISCONNECT.
3. Reject QoS 2, malformed or non-minimal frames, unexpected packets, zero
   packet identifiers, oversized packets, and inconsistent session state.
4. Keep RX, TX, and one in-flight replay packet in caller-owned buffers. No
   global allocator or hidden queue is introduced.
5. Retain an outbound QoS 1 publish or subscription until its matching broker
   acknowledgment. Replay it after reconnect; set DUP on a replayed publish.
6. Delay inbound QoS 1 PUBACK until the application explicitly accepts the
   borrowed message and calls `acknowledge_received`.
7. Permit credentials only when the caller marks the supplied stream as an
   authenticated encrypted transport. Plaintext remains available solely for
   isolated broker fixtures without credentials.
8. Preserve the separate MQTT 5 `minimq` adapter. Neither adapter translates
   between MQTT versions.
9. Treat the adapter as experimental until strict-broker, verified-TLS, live
   IoT Hub, fuzz, and ESP32-C6 resource gates pass.

## Consequences

- The Azure path has an auditable MQTT 3.1.1 implementation whose memory and
  acknowledgment ownership match the SDK's requirements.
- The implementation is small enough to review, but the repository now owns
  protocol parsing, interoperability, fuzzing, and maintenance for this
  subset.
- Only one acknowledgment-bearing outbound operation may be in flight. This
  is an intentional first-slice memory bound, not a broker limitation.
- A generic common session trait remains deferred until the Azure provider
  state machine proves the exact cross-backend API. The concrete adapter is a
  firmware dependency and is not exported by the portable facade.
- A future maintained backend may replace this adapter after passing the same
  tests without changing Azure topic or identity code.

## Alternatives considered

- Using an incomplete or incorrect ecosystem backend was rejected because
  successful compilation would hide wire-level and acknowledgment failures.
- Forking a candidate was rejected because it would retain unnecessary APIs
  and dependencies while creating equivalent maintenance responsibility.
- Extending `minimq` was rejected for this slice because its state machine and
  encoding are designed around MQTT 5; changing them risks the working MQTT 5
  path.
- Blocking all Azure work on a future dependency release was rejected because
  the bounded subset can be implemented and verified independently.
