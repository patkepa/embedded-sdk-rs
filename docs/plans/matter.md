# Matter Support Plan

## Status

- Status: Proposed
- Date: 2026-09-07
- Initial target: Seeed Studio XIAO ESP32C6
- Initial transport: Matter over Wi-Fi with Bluetooth Low Energy commissioning
- Later transport: Matter over Thread

## Objective

Introduce Matter accessory support without duplicating the Matter protocol or
breaking the SDK's portable dependency architecture. The first vertical slice
will be a dedicated XIAO ESP32C6 On/Off Light firmware that can be commissioned
onto Wi-Fi and controlled by a Matter controller.

The implementation should use the upstream `rs-matter` ecosystem. `rs-matter`
is a `no_std`, async-first Matter implementation, and `rs-matter-embassy`
provides Embassy networking, BLE commissioning, storage, Wi-Fi, and Thread
integration. The SDK will own lifecycle, resource, platform, storage, and
product boundaries around those dependencies rather than introduce a competing
endpoint, cluster, session, or commissioning model.

Matter support is not a single protocol-crate addition. It requires a complete
vertical slice across IPv6 and UDP networking, multicast DNS discovery, BLE
commissioning, dynamic network configuration, cryptography, device attestation,
persistent fabrics, factory reset, and product-specific endpoint behavior.

## Non-goals

The first implementation will not:

- implement the Matter protocol from scratch;
- duplicate `rs-matter` endpoint or cluster types in SDK-owned abstractions;
- add Matter to the existing multipurpose XIAO reference firmware;
- run MQTT or the existing custom GATT service in the initial Matter image;
- claim production, certification, or secure-storage readiness;
- add Matter-over-Thread before the Wi-Fi vertical slice is understood;
- use shared development attestation credentials in a production build.

## Architecture

```text
Matter product firmware
  endpoints, clusters, device behavior, factory reset
                 |
        embedded-sdk Matter integration
  lifecycle, configuration, persistence adapters
                 |
  rs-matter + rs-matter-stack/rs-matter-embassy
       |                 |                 |
  embassy-net         TrouBLE         SDK storage
   IPv6/UDP          BLE GATT         flash backend
       +-----------------+-----------------+
                         |
                ESP32-C6 platform port
              Wi-Fi / BLE / later Thread
```

Dependency direction must continue to follow the repository architecture:

- firmware owns the Matter node, endpoints, application cluster handlers,
  product configuration, task policy, resource sizes, and physical reset UI;
- a portable Matter integration owns SDK lifecycle and error translation plus
  adapters that do not depend on a chip or board;
- `rs-matter` owns the Matter data model, secure channel, sessions, fabrics,
  commissioning, discovery records, and wire protocol;
- the ESP32-C6 port owns chip-specific radio and runtime integration;
- the XIAO board package owns RF-switch pins, flash regions, buttons, and
  production hardware identity;
- test credentials and test device behavior remain confined to reference
  firmware and fixtures.

An `embedded-sdk-matter` crate should be introduced only after the initial
firmware spike identifies a reusable boundary. It should not exist merely as a
placeholder or reimplement upstream APIs. Likely responsibilities are:

- bounded SDK-facing Matter configuration and lifecycle state;
- translation into SDK telemetry and service-health events;
- an adapter from `embedded_sdk_storage::KeyValueStore` to the upstream Matter
  blob-store contract;
- shared resource sizing and validation helpers;
- carefully selected re-exports that pin the upstream API version used by the
  SDK.

Matter must be opt-in in the `embedded-sdk` facade so applications that do not
use it do not acquire its dependency graph or memory cost.

## Initial firmware

Add a separate firmware package:

```text
firmware/seeed/xiao-esp32c6-matter-light/
```

The firmware will implement one Matter On/Off Light endpoint in addition to the
mandatory root endpoint. It will own both `WIFI` and `BT` through one coordinated
Matter wireless composition.

It must not try to compose the existing standalone Wi-Fi station task and custom
GATT task. The current Wi-Fi firmware receives development credentials at build
time, while a commissioned Matter device receives and applies credentials
through the Network Commissioning cluster. The current custom GATT service is
also not a Matter commissioning service.

The first firmware may use explicitly marked test device-attestation material
and a dummy store while bringing up the data path. It must print or otherwise
expose the development QR/manual pairing payload without logging commissioned
network credentials, fabric secrets, private keys, peer identities, or complete
operational certificates.

## Dependency compatibility gate

Before SDK abstractions are added, perform a time-boxed dependency and resource
spike. The SDK currently pins:

- `esp-hal` 1.1.1;
- `esp-radio` 0.18.0;
- `esp-rtos` 0.3.0;
- `trouble-host` 0.6.0;
- `embassy-net` 0.9.1.

Current upstream ESP32-C6 examples use newer or Git-patched ESP radio and
TrouBLE revisions. Peripheral types from incompatible HAL releases cannot be
mixed across the SDK and Matter integration. The spike must decide between:

1. using a released `rs-matter-embassy` version compatible with the SDK pins;
2. making one reviewed, workspace-wide Espressif dependency upgrade; or
3. retaining the pins and implementing the required upstream platform traits
   against the SDK's existing adapters.

The selected versions must be pinned centrally. If a Git revision is necessary,
record its exact revision, reason, upgrade/removal condition, license review,
and advisory implications. Do not leave floating branches in release inputs.

The compatibility gate passes when:

- the existing host suite and all existing firmware still build;
- the Matter light cross-compiles for `riscv32imac-unknown-none-elf`;
- there is only one compatible active ESP HAL/radio type family in the Matter
  binary;
- the dependency tree contains no unexplained duplicate crypto or networking
  stacks;
- flash, static RAM, heap, and largest-future measurements are recorded.

## Networking work

The current portable network model and Embassy configuration are IPv4-only and
enable DHCPv4, DNS, and TCP. Matter needs an IPv6-capable UDP path, link-local
and multicast operation, and DNS-SD/mDNS discovery.

Required changes are:

1. Enable the reviewed `embassy-net` IPv6, UDP, and multicast-related features.
2. Extend the portable network snapshot with bounded IPv6 address state.
3. Preserve independent facts for link-up, IPv4 readiness, IPv6 readiness, DNS
   readiness, and application reachability.
4. Add conversion and readiness tests to the Embassy adapter.
5. Reserve explicit socket counts and caller-owned UDP buffers in firmware.
6. Use the embedded mDNS implementation supplied by the Matter integration for
   Matter DNS-SD records rather than creating another SDK-specific record
   model.

The Matter firmware may initially use the upstream Embassy network composition
directly. The reusable SDK networking model should nevertheless gain IPv6 state
before Matter is promoted from experimental support.

## Bluetooth and Wi-Fi work

Matter BLE commissioning requires a protocol-specific GATT service and hands
Wi-Fi credentials to the Matter network-control implementation. For the first
firmware, `rs-matter-embassy` should own this combined flow.

Longer term, the platform port should expose controller primitives compatible
with the upstream `GattPeripheral` and Wi-Fi `NetCtl` boundaries. Portable
Matter code must not depend directly on `esp-hal`, `esp-radio`, or raw ESP32-C6
peripheral tokens.

The radio owner must document:

- whether commissioning and Wi-Fi operation run concurrently;
- controller, L2CAP, packet-pool, and connection capacities;
- coexistence initialization and failure recovery;
- behavior when Wi-Fi credentials are rejected or the access point disappears;
- commissioning-window timeout and reopening policy;
- BLE shutdown or reduced operation after commissioning.

## Persistence and factory reset

Matter operational state must survive reboot. This includes fabrics,
operational credentials, access-control state, groups, and commissioned network
configuration as required by the selected upstream stack.

Add an adapter with this direction:

```text
embedded_sdk_storage::KeyValueStore
                  |
                  v
       rs_matter persistent blob store
```

The adapter should reserve a stable Matter namespace and translate complete
blob reads, atomic replacements, deletions, capacity failures, and corruption
without exposing backend errors to the protocol layer.

Before persistence is enabled on XIAO hardware:

- define a reviewed, erase-aligned flash partition owned by the board/product;
- add an ESP flash implementation of the ecosystem NOR traits;
- size the store for the configured fabric, ACL, group, and credential limits;
- test reboot recovery, interrupted writes, compaction, corruption, and full
  storage;
- define schema/version migration policy for upstream Matter upgrades;
- define a physical factory-reset gesture and reset confirmation behavior;
- delete Matter and network state without erasing unrelated SDK namespaces.

The generic SDK store does not provide confidentiality, authentication, secure
erase, or rollback protection. Device-attestation private keys and other
factory secrets must use eFuse, a secure element, or another reviewed protected
backend. Fabric secrets also require an explicit storage threat model.

## Identity, attestation, and cryptography

Development builds may use upstream test credentials only when the image and
logs clearly state that the device is uncertified and not production-ready.

Production support requires a factory process that provisions unique or
product-authorized values as applicable:

- setup passcode and discriminator;
- vendor ID, product ID, serial number, and hardware version;
- Device Attestation Certificate and private key;
- Product Attestation Intermediate certificate;
- Certification Declaration;
- rotating device identity or other privacy data required by the selected
  Matter release;
- an auditable association between manufacturing records and device identity.

The crypto provider must be fed from a cryptographically secure, reseeding RNG.
Hardware acceleration is optional, but any hardware-backed provider needs
known-answer tests, failure handling, side-channel review appropriate to the
product, and a software fallback policy. Secret values must be redacted from
`Debug`, telemetry, panic output, and commissioning diagnostics.

## Resource budget

Matter resource use must remain explicit and bounded. Measure at least:

- application image size and selected app partition utilization;
- `.bss` and other static RAM;
- configured radio heap and measured high-water mark;
- Matter future/bump-arena size;
- task stack or largest Embassy future;
- UDP socket count and RX/TX buffers;
- BLE packet pool, connections, and L2CAP channels;
- peak usage while commissioning;
- steady-state usage with active subscriptions;
- repeated commissioning, Wi-Fi loss, and controller reconnect behavior.

The current general XIAO firmware uses a 96 KiB radio heap. Upstream examples
reserve additional static Matter state, a future arena, and a larger total
heap. Do not infer that the current allocation is sufficient. The first Matter
image should exclude MQTT and the custom GATT service so its baseline is
understandable.

All compile-time capacity selections must be documented. Capacity exhaustion
must return a controlled error or reject new work rather than panic, corrupt
persistent state, or silently drop security-relevant operations.

## Thread follow-up

Matter-over-Thread is a later milestone. The ESP32-C6 port already advertises
the silicon's IEEE 802.15.4 capability, but correctly does not advertise Thread
support.

Thread integration must follow the existing repository architecture:

- isolate native OpenThread bindings in `openthread-sys`;
- isolate the audited unsafe FFI and Platform Abstraction Layer in an
  OpenThread platform crate;
- let the ESP32-C6 port provide radio, alarms, entropy, settings, reset, and
  logging;
- preserve Thread operational datasets, roles, attachment, and network
  diagnostics instead of reducing Thread to a generic socket;
- use the OpenThread UDP and discovery integration expected by the selected
  Matter stack;
- validate BLE/802.15.4 coexistence before claiming normal controller support.

Wi-Fi Matter support must not be delayed while Thread radio, OpenThread FFI,
and three-radio coexistence are still being validated.

## Verification strategy

### Host tests

- Matter configuration bounds and invalid setup data;
- node descriptor, endpoint, device-type, and cluster metadata;
- storage adapter complete-or-error behavior;
- persistent namespace stability;
- corruption, missing-record, and capacity mappings;
- factory-reset selection of Matter-owned keys;
- IPv6 readiness and Embassy conversion;
- lifecycle and telemetry redaction.

### Compile and size tests

- all current default workspace members;
- every existing XIAO firmware variant;
- the Matter light for `riscv32imac-unknown-none-elf`;
- minimal and intended production capacity feature sets;
- dependency duplication and license policy;
- image and static-memory regression budgets.

### Hardware and interoperability tests

- commissioning with `chip-tool` over BLE to Wi-Fi;
- On/Off read, write, invoke, and subscription behavior;
- commissioning-window expiry and reopening;
- reboot with fabric and network state retained;
- factory reset followed by recommissioning;
- invalid credentials and recovery without reboot;
- access-point loss and restoration;
- repeated controller disconnects and subscriptions;
- bounded resource behavior across repeated commissioning attempts;
- interoperability with representative Apple, Google, Alexa, Home Assistant,
  Samsung, and other target controllers;
- applicable official ConnectedHomeIP integration and certification test cases.

Passing consumer-controller smoke tests does not constitute Matter
certification. Certification readiness additionally requires the applicable
Matter specification, PICS declarations, test plan, product identity,
attestation chain, Alliance process, and release-specific compliance evidence.

## Delivery phases

### Phase 0: Dependency and memory spike

- Pin one upstream `rs-matter` integration set.
- Cross-compile an upstream-style ESP32-C6 light in this workspace.
- Resolve ESP HAL, radio, TrouBLE, Embassy, crypto, and allocator versions.
- Record flash and RAM costs.
- Decide the reusable SDK boundary in an ADR.

Exit gate: reproducible build without regressing existing packages, with an
accepted dependency and ownership design.

### Phase 1: On-network Matter light

- Add IPv6, UDP, and multicast networking support.
- Run a minimal light on preconfigured Wi-Fi.
- Prove operational mDNS discovery, secure sessions, and On/Off interaction.
- Add `xtask` build, run, and size support for the new firmware variant.

Exit gate: repeatable `chip-tool` pairing over IP and cluster interaction on
controlled hardware.

### Phase 2: BLE-to-Wi-Fi commissioning

- Add the Matter BLE GATT service.
- Implement dynamic Wi-Fi scan, credential application, and failure reporting.
- Generate development QR and manual pairing payloads.
- Validate Wi-Fi/BLE coexistence and commissioning-window policy.

Exit gate: fresh device commissioning succeeds repeatedly without build-time
Wi-Fi credentials.

### Phase 3: Persistent operation

- Define the XIAO flash partition.
- Implement the storage adapter and ESP flash backend.
- Retain fabric and network state across reset.
- Implement product-owned factory reset and recommissioning.
- Add interrupted-operation and corruption coverage.

Exit gate: commissioned operation survives power cycles and factory reset
removes only the intended state.

### Phase 4: SDK integration and interoperability

- Extract the proven reusable Matter crate and platform adapters.
- Add lifecycle, health, and telemetry integration.
- Set explicit capacity defaults and resource regression budgets.
- Run official test cases and the representative controller matrix.
- Update the compatibility matrix without overstating the support tier.

Exit gate: documented experimental or Tier 2 Matter support with HIL evidence.

### Phase 5: Production security and certification preparation

- Implement protected device-attestation key provisioning.
- Replace all test identity and commissioning material.
- Complete provisioning, storage, reset, debug-port, and update threat models.
- Prepare certification declarations and release-specific test evidence.

Exit gate: an explicitly selected product can enter the formal certification
process. The generic SDK alone is not a certified Matter product.

### Phase 6: Matter over Thread

- Add the isolated OpenThread subsystem and ESP32-C6 platform layer.
- Add Thread network commissioning and persistent operational dataset support.
- Validate border-router interoperability and BLE/Thread coexistence.
- Add a separate Thread Matter firmware variant and compatibility entry.

Exit gate: commissioning, persistence, recovery, and controller operation pass
the Thread HIL and interoperability suite.

## First implementation pull request

The first implementation pull request should contain:

- an accepted Matter architecture ADR;
- the pinned dependency compatibility result;
- `firmware/seeed/xiao-esp32c6-matter-light` using test-only credentials;
- the minimum IPv6/UDP feature changes needed by the spike;
- an `xtask` firmware variant;
- host metadata tests and a cross-compile CI job;
- flash, static RAM, heap, and future-size measurements;
- documentation that labels persistence, attestation, HIL, interoperability,
  and certification as incomplete.

It should not yet add Matter to the default `embedded-sdk` facade or claim
production support. Extract reusable crates only after the vertical slice shows
which abstractions remain stable.

## Open decisions

1. Which released or pinned revisions of `rs-matter`, `rs-matter-stack`, and
   `rs-matter-embassy` form the initial supported set?
2. Can that set use the current ESP dependency family, or is a workspace-wide
   upgrade required?
3. Should the long-term ESP adapter use upstream `EspWifiDriver` directly or
   implement upstream platform traits over SDK-owned Wi-Fi and BLE primitives?
4. What IPv6 state belongs in the portable networking snapshot without making
   it Matter-specific?
5. What flash partition size and store capacities cover the supported fabric,
   ACL, group, subscription, and network limits?
6. Where will production device-attestation private keys live, and how will the
   factory inject them?
7. What physical gesture and authorization policy opens a commissioning window
   and performs factory reset?
8. Which Matter specification version and device types are release targets?

## References

- `rs-matter`: <https://github.com/project-chip/rs-matter>
- `rs-matter-stack`: <https://github.com/sysgrok/rs-matter-stack>
- `rs-matter-embassy`: <https://github.com/sysgrok/rs-matter-embassy>
- Embassy ESP examples:
  <https://github.com/sysgrok/rs-matter-embassy/tree/master/examples/esp>
- ConnectedHomeIP: <https://github.com/project-chip/connectedhomeip>
- `chip-tool` commissioning guide:
  <https://github.com/project-chip/connectedhomeip/blob/master/docs/development_controllers/chip-tool/chip_tool_guide.md>
- Matter specifications:
  <https://csa-iot.org/developer-resource/specifications-download-request/>

