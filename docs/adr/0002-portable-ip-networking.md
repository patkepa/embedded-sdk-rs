# ADR 0002: Portable IP Networking Boundary

- Status: Accepted
- Date: 2026-08-31

## Context

The ESP32-C6 Wi-Fi adapter can scan and associate with an access point, but
association is only a link-layer operation. DHCP, DNS, sockets, cloud
protocols, and update transports all need an IP stack. If portable services
depend directly on an ESP radio interface, they cannot be reused with Ethernet,
hosted Wi-Fi, cellular, or another MCU vendor.

The Rust embedded ecosystem already provides the required boundaries.
`embassy-net-driver` represents a packet interface, `embassy-net` supplies an
allocation-free asynchronous IP stack, and `embedded-io-async` and
`embedded-nal-async` provide protocol-facing I/O contracts. The SDK only needs
to add the lifecycle and configuration state used consistently by its services,
health reporting, and telemetry.

## Decision

1. `embedded-sdk-networking` owns allocation-free, platform-independent link,
   IPv4 configuration, gateway, and DNS-server state. It does not implement an
   IP stack or sockets.
2. `embedded-sdk-networking-embassy-net` maps an `embassy-net` stack into the
   portable state model and provides bounded DNS resolution. It does not depend
   on an MCU, radio, or board.
3. The platform port owns the vendor Wi-Fi controller and exposes the station
   packet interface that implements `embassy-net-driver::Driver`.
4. Firmware owns IP-stack resources, random seed, task spawning, buffer sizes,
   retry policy, and product-specific connectivity probes.
5. Link state, valid IP configuration, configured DNS servers, and verified
   endpoint reachability are separate facts. Association must not be reported
   as IP or internet readiness.
6. The first implementation supports DHCPv4 and DNS over an ESP32-C6 station
   link. IPv6 and static address policy remain later additive work.
7. Protocol crates should consume ecosystem I/O traits or focused
   protocol-specific abstractions. The SDK does not define a universal socket
   trait.

## Consequences

- Portable services can observe the same network state across future Wi-Fi,
  Ethernet, Thread, or cellular implementations.
- The Embassy integration remains reusable across platform drivers while the
  portable facade does not require an IP-stack implementation.
- Network memory use stays visible through compile-time socket counts and
  caller-owned buffers.
- Firmware must run the radio controller and IP-stack runner concurrently and
  retain both for as long as networking is enabled.
- A DHCP lease without DNS remains useful for literal-address communication and
  is represented independently rather than treated as total network failure.
- Adding TLS, MQTT, HTTP, or OTA still requires separate security, protocol,
  resource, and recovery decisions.

## Alternatives considered

- Putting `embassy-net` types in `embedded-sdk-wifi` was rejected because IP
  networking is not specific to Wi-Fi and would mix link and network layers.
- Defining SDK-specific socket traits was rejected because established embedded
  I/O contracts already exist and a new universal interface would discard
  transport-specific capabilities.
- Keeping all state in the reference firmware was rejected because portable
  services, health policy, and future platform ports need a stable shared
  representation.
- Treating association as network readiness was rejected because DHCP, DNS, or
  endpoint reachability can fail independently after the radio link comes up.
