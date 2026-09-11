# ADR 0004: Portable Cloud Provider Boundary

- Status: Accepted
- Date: 2026-09-03

## Context

The SDK needs device-to-cloud telemetry, commands, remote state, provisioning,
and fleet operations across providers such as Azure IoT Hub and AWS IoT Core.
Those providers share transports and some lifecycle concerns, but their topic
layouts, authentication, state synchronization, command semantics, quotas, and
delivery behavior are not interchangeable.

Putting provider topics in firmware would duplicate protocol rules between
products. Putting them in the MQTT crate would make a general application
protocol depend on one cloud service. Conversely, defining a large universal
cloud client before two providers exist would freeze guessed similarities into
the public SDK.

## Decision

1. `embedded-sdk-cloud-core` owns only provider-independent lifecycle states,
   error categories, capability flags, and health counters demonstrated by
   concrete integrations.
2. Each provider has a separate portable crate. The initial
   `embedded-sdk-cloud-azure-iot` crate owns Azure identity validation,
   connection parameters, topics, response parsing, and provider state
   machines.
3. Provider crates may depend on portable protocol and security contracts but
   not on an executor, network stack, MCU, board, concrete MQTT backend, TLS
   implementation, or product payload schema.
4. Firmware composes DNS, TCP, TLS, credentials, storage, protocol adapters,
   bounded queues, task supervision, and product policy.
5. MQTT owns MQTT concepts only. Azure topics and identity rules must not enter
   `embedded-sdk-mqtt` or its backend adapters.
6. Provider operations remain explicit. Azure twins and direct methods are not
   renamed into generic shadows or commands until a second implementation
   proves a safe shared contract.
7. Provider facade exports are opt-in Cargo features. Enabling Azure support
   adds APIs and must not replace an MQTT backend or change another session's
   wire version.
8. Cloud capability claims are granular. Telemetry support does not imply
   cloud-to-device messages, twins, provisioning, production authentication,
   or durable delivery.

## Consequences

- Azure protocol logic can be host-tested without Embassy, Espressif, TLS, or
  live credentials.
- Future AWS support can reuse MQTT, TLS, security, storage, and generic cloud
  health concepts while keeping provider semantics honest.
- Firmware remains responsible for resource and recovery policy, so a cloud
  outage need not restart local connectivity or unrelated services.
- The facade gains a small always-available cloud core and feature-gated
  provider namespaces.
- Some apparently similar operations remain duplicated until evidence supports
  a common abstraction. This is intentional and cheaper than maintaining a
  misleading compatibility layer.

## Alternatives considered

- A single `CloudClient` trait was rejected because telemetry, twins/shadows,
  commands, jobs, and provisioning have materially different semantics across
  providers.
- Azure logic in the MQTT crate was rejected because MQTT is also used with
  ordinary brokers and other cloud services.
- Azure logic in the ESP32-C6 port was rejected because a cloud protocol is not
  a chip capability.
- Azure logic only in reference firmware was rejected because topic encoding,
  parsing, correlation, and service state are portable and independently
  testable.
- Depending directly on the deprecated Azure SDK for Embedded C was rejected
  because it would introduce an unmaintained FFI foundation and does not match
  the workspace's Rust protocol boundaries.
