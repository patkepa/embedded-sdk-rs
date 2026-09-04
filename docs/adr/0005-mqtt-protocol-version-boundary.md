# ADR 0005: MQTT Protocol Version Boundary

- Status: Accepted
- Date: 2026-09-03

## Context

The first MQTT implementation uses `minimq` 0.13 and MQTT 5. Azure IoT Hub and
Azure DPS device endpoints instead require MQTT 3.1.1. MQTT 5 session expiry
properties cannot be sent to an MQTT 3.1.1 endpoint, while MQTT 3.1.1 clean
session behavior does not fully describe an MQTT 5 session.

An implicit protocol version in portable configuration could create packets
that a configured provider cannot accept. Replacing the MQTT 5 backend would
also break the existing local-broker path and discard a useful implementation.

## Decision

1. `embedded-sdk-mqtt::Config` identifies its wire protocol explicitly.
2. MQTT 3.1.1 and MQTT 5 use separate session configuration values. Callers
   cannot attach MQTT-5-only session expiry to an MQTT 3.1.1 session.
3. Hostname, port, client ID, keepalive, QoS 0/1, topic, packet capacity,
   lifecycle, errors, and reconnect concepts remain common where their meaning
   is compatible.
4. The portable client-ID bound is 128 bytes to accommodate cloud device
   identities. A backend may advertise or enforce a smaller limit.
5. `embedded-sdk-mqtt-minimq` remains an MQTT 5 adapter and rejects MQTT 3.1.1
   configuration before allocating session state or writing a transport.
6. Azure support uses a separate MQTT 3.1.1 backend adapter selected through
   source review, local broker tests, live Azure tests, and hardware resource
   measurement.
7. A narrow asynchronous session trait is introduced only after the second
   backend proves the operations, lifetimes, acknowledgment behavior, and
   capability reporting required by consumers.
8. Supporting a protocol version is a wire-level capability. No adapter may
   silently upgrade, downgrade, or translate between MQTT 3.1.1 and MQTT 5.

## Consequences

- Existing MQTT 5 behavior and tests remain available.
- Azure configuration can express `CleanSession=false` without pretending it
  is an MQTT 5 session-expiry interval.
- Provider code can reject an incompatible backend during composition rather
  than after a remote connection fails mysteriously.
- Concrete backend limits remain explicit. In particular, the current
  `minimq` adapter can continue enforcing its smaller internal client-ID bound.
- The SDK temporarily has version-aware configuration without a common client
  trait. This avoids designing receive lifetimes and manual acknowledgment
  semantics from only one implementation.

## Alternatives considered

- Converting the workspace to MQTT 3.1.1 only was rejected because it would
  remove working MQTT 5 support and constrain future brokers and AWS IoT use.
- Sending MQTT 5 to IoT Hub was rejected because the service's device endpoint
  specifies MQTT 3.1.1.
- Treating session expiry and clean session as the same numeric option was
  rejected because their wire encodings and lifecycle semantics differ.
- Forking `minimq` immediately was rejected until maintained MQTT 3.1.1
  ecosystem implementations have been evaluated.
- Defining the common session trait before selecting a second backend was
  rejected because inbound borrowing, QoS acknowledgment, cancellation, and
  reconnect state are precisely the behavior that must be proven first.
