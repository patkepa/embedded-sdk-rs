# Azure IoT X.509 Hardware Vertical Slice

## Objective

Turn the XIAO ESP32-C6 Azure target from a DNS-only preflight into an opt-in,
end-to-end Azure IoT Hub client matching the existing product firmware's cloud
connection shape:

```text
Wi-Fi -> DNS -> TCP:8883 -> mutual TLS -> MQTT 3.1.1
       -> Azure subscriptions -> full twin sync -> online processing
```

The safe default must remain disconnected. Development credentials and a
trusted-time snapshot are accepted only when the build explicitly enables the
development X.509 path. They must never be logged.

## Work items

- [x] Support MQTT 3.1.1 CONNECT with a username and no password for X.509
      authenticated Azure devices.
- [x] Support a caller-supplied client certificate chain and private key in the
      TLS 1.2 adapter.
- [x] Verify mutual TLS and rejection of an untrusted client certificate in
      host tests.
- [x] Add explicit, all-or-nothing X.509 development configuration validation
      to the XIAO firmware.
- [x] Compose DNS, TCP, mutual TLS, MQTT, Azure subscriptions, full twin sync,
      reported properties, telemetry, and direct-method responses.
- [x] Cancel the cloud session on IP loss and apply bounded reconnect backoff.
- [x] Compile without Azure configuration as a safe disconnected image.
- [x] Run focused unit/integration tests, strict linting, and the bare-metal
      release build.

## Acceptance boundary

This slice implements the code needed to attempt the **experimental hardware**
gate; that support label remains unmet until a real XIAO connects to a test hub
and survives the required scenarios. It does not make the implementation a
production candidate. Production still requires a protected runtime identity
store or opaque signer, authenticated/persisted trusted time, certificate
rotation, live IoT Hub evidence, fault injection, and ESP32-C6 resource/HIL
measurements.
