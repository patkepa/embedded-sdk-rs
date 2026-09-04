# OpenThread Integration Plan

## Status

- Status: Proposed
- Branch: `feat/openthread`
- Initial targets: Seeed Studio XIAO ESP32C6 and nRF52840 DK
- Initial profile: Minimal Thread Device using a local IEEE 802.15.4 radio
- Primary outcome: one portable Thread node service that forms or joins the
  same network, persists its configuration, reattaches after reset, and
  exchanges IPv6 UDP on ESP32-C6 and nRF52840

## Recommendation

Use one shared OpenThread stack and adapt hardware below it. Keep the stable
Thread domain model in `embedded-sdk-thread`, place the concrete integration in
`embedded-sdk-thread-openthread`, and let each platform port provide radio,
identity, entropy, reset, and flash primitives. Firmware owns the device
profile, resources, commissioning policy, task topology, and radio coexistence
policy.

Use the upstream Rust `openthread` crate as the initial integration candidate.
It already provides a `no_std` asynchronous safe API, static resources, an
IEEE 802.15.4 radio trait, ESP and Nordic adapters, software-MAC and
higher-priority proxy helpers, native UDP/SRP/DNS APIs, an optional
`embassy-net` packet bridge, and a Spinel RCP path. Depend on it with
`default-features = false`, pin the selected release in `Cargo.lock`, and keep
all of its types behind the SDK adapter.

The dependency remains pre-1.0 and builds native OpenThread code. Its exact
release, feature set, compiler requirements, generated bindings, crypto
configuration, libc shims, allocation behavior, licenses, advisories, and
memory cost must pass the dependency proof before its API shapes SDK contracts.

This plan implements [ADR 0004](../adr/0004-portable-openthread.md).

## Preconditions and non-goals

The repository already provides the intended ownership model, Embassy runtime,
portable hardware capabilities, asynchronous storage primitives, networking
state, and an ESP32-C6 port whose silicon advertises raw IEEE 802.15.4.

The first Thread slice must not:

- place portable Thread datasets or lifecycle behavior in an ESP or Nordic
  port;
- expose raw OpenThread FFI or upstream crate types through `embedded-sdk`;
- equate an IEEE 802.15.4 radio with a working Thread capability;
- run separate vendor Thread stacks behind superficially similar APIs;
- use the generic asynchronous KV contract as if it satisfied synchronous
  OpenThread settings;
- treat attachment as DNS or internet readiness;
- claim simultaneous ESP32-C6 Wi-Fi, BLE, and Thread support without a tested
  coexistence profile;
- include Matter, border routing, production commissioning, or Thread
  certification in the first vertical slice.

## Dependency and ownership model

```text
firmware and product services
        │
        ├── board package
        │      pins, clocks, RF path, storage partition, regulatory data
        │
        ├── embedded-sdk-thread
        │      portable datasets, roles, lifecycle, state, error categories
        │
        ├── protocol-facing data plane
        │      native OT UDP/DNS/SRP first; optional embassy-net bridge later
        │
        └── embedded-sdk-thread-openthread
               actor, state mapping, resources, settings, upstream adapter
                         │
                    openthread crate
                         │
                  asynchronous Radio
              ┌──────────┼───────────┐
              │          │           │
          ESP radio  Nordic PHY  Spinel UART/SPI
```

Dependency arrows point inward. Portable application and protocol crates do
not depend on platform ports. Platform ports do not own product policy.

## Proposed packages and APIs

### `embedded-sdk-thread`

Create `crates/thread` as a `no_std`, allocation-free portable package. It
depends only on portable utility crates deliberately selected by the SDK.

It should own:

- `DeviceRole`: `Disabled`, `Detached`, `Child`, `Router`, and `Leader`;
- `ThreadState`: stopped, starting, uncommissioned, attaching, attached,
  detached, leaving, and failed lifecycle states;
- `ThreadSnapshot`: commissioned state, role, channel, PAN ID, extended PAN ID,
  partition ID, RLOC16, and bounded diagnostic summaries;
- bounded `OperationalDatasetTlvs` and validated network identity values;
- secret-bearing dataset and key types with redacted `Debug`, no `Display`, and
  zeroization on drop;
- backend-independent `ErrorKind` values while adapters preserve their source
  errors;
- attachment retry/backoff policy only where OpenThread does not already own
  the protocol behavior;
- bounded IPv6 and neighbor summaries suitable for health and telemetry.

The base controller should expose only behavior shared by supported nodes:

```text
install/read active dataset
enable/disable IPv6 interface
start/stop Thread
detach gracefully
leave and erase persistent network state
read snapshot
wait for state change
```

Optional behavior belongs in focused capabilities rather than a universal
interface full of `Unsupported` results:

- `ThreadJoiner`
- `ThreadCommissioner`
- `ThreadRouter`
- `ThreadSrpClient`
- `ThreadDiagnostics`

The portable facade may export `embedded_sdk::thread`. It must not export raw
radio, FFI, or upstream OpenThread types.

### `embedded-sdk-thread-openthread`

Create `crates/thread-openthread`. It depends on `embedded-sdk-thread` and the
reviewed OpenThread integration, but not on an MCU or board.

The adapter should:

- map portable datasets, roles, state-change events, diagnostics, and errors;
- own the only live OpenThread instance and all resources borrowed by it;
- serialize every OpenThread API call;
- convert synchronous OpenThread callbacks into bounded state changes without
  awaiting or retaining borrowed pointers;
- expose stable handles instead of upstream clones or C instance pointers;
- provide native UDP and selected DNS/SRP adapters through established network
  traits;
- optionally produce a bare-IP packet interface for a later `embassy-net`
  adapter;
- expose counters needed to distinguish radio, MAC, MLE, IPv6, settings,
  capacity, and application failures;
- document every unsafe invariant inherited from the upstream boundary even
  though SDK-owned Rust continues to forbid unsafe code.

Use an actor-shaped runtime:

```text
ThreadHandle(s)
      │ bounded commands / watched snapshots
      ▼
ThreadRunner ── OpenThread tasklets and alarms
      │
      └── Radio runner, optionally proxied to a high-priority executor
```

The command capacity, response slots, event subscribers, radio queues, UDP
sockets, SRP entries, and native heap selection are compile-time resource
contracts owned by firmware.

### Thread settings adapter

OpenThread settings calls are synchronous and support multiple values with the
same key. Add an adapter-internal `ThreadSettingsStore` with exact operations:

```text
init(sensitive_keys)
get(key, index, output)
set(key, value)
add(key, value)
delete(key, optional_index)
wipe()
```

The first durable implementation should use:

- a dedicated, board-owned flash region;
- a bounded RAM index or mirror loaded before OpenThread starts;
- an append-only CRC-protected journal;
- at least one pre-erased recovery/compaction page;
- atomic visibility of complete old or new values after interruption;
- deferred or explicitly scheduled compaction outside radio-critical timing;
- sensitive-key classification connected to the product security policy;
- a versioned persistent format and explicit migration or factory-reset rules.

Do not claim encrypted secret storage merely because the records have CRCs.
The product threat model must choose flash encryption, authenticated records,
or a secure element where required.

### IPv6 and application networking

Extend `embedded-sdk-networking` additively rather than introducing a
Thread-specific general socket API. The portable network model needs:

- a bounded list of IPv6 addresses and prefix lengths;
- address origin and scope where reliably known;
- default-route and DNS discovery state;
- independent Thread attachment, IPv6 readiness, DNS readiness, and endpoint
  reachability;
- snapshots that can represent IPv4 and IPv6 concurrently.

OpenThread remains authoritative for its addresses, routes, and network data.
Use native OpenThread UDP for the first end-to-end slice and adapt it to an
established datagram trait. Use native OpenThread DNS and SRP APIs when their
Thread-specific semantics matter.

An optional `embassy-net` integration can later pass naked IPv6 packets between
OpenThread and `embassy-net`, enabling reuse of general TCP/UDP protocols. It
is accepted only when a state synchronizer:

- follows every OpenThread address and route change;
- handles multiple Thread IPv6 addresses without silently selecting the wrong
  source address;
- updates link state during detach and reattach;
- has defined DNS behavior;
- survives partition changes, border-router loss, and network-data updates;
- passes source-address, multicast, and route interoperability tests.

Until then, attachment must not be presented as a generic `embassy-net`-ready
interface.

## Platform work

### ESP32-C6

Add an opt-in `thread` feature and a `thread` module to
`embedded-sdk-platform-esp32c6`. The module composes the ESP IEEE 802.15.4
radio with the shared OpenThread adapter and supplies:

- factory-derived IEEE EUI-64 in the byte order required by OpenThread;
- qualified hardware entropy rather than an example-generated identity;
- regulatory region and channel/power bounds;
- reset-reason and logging integration;
- radio initialization and error mapping;
- coexistence hooks supported by the selected `esp-radio` release.

Do not add Thread to the current default Wi-Fi/BLE feature bundle. Introduce
explicit build profiles for Thread-only and any validated coexistence
combination. Invalid or unvalidated feature combinations should fail at compile
time or remain outside supported firmware packages.

The first ESP firmware should be a dedicated package such as
`firmware/seeed/xiao-esp32c6-thread`. It should not enlarge the existing
multi-radio reference image before standalone Thread behavior is reliable.

### nRF52840

Add `ports/nordic/nrf52840` and an nRF52840 DK board package. The port supplies:

- Embassy nRF IEEE 802.15.4 PHY integration;
- factory identity and hardware RNG;
- internal-flash implementation for the board-owned settings region;
- interrupt bindings and clock requirements;
- a software-MAC wrapper for missing ACK/filter capabilities;
- a proxy radio runner on a high-priority executor when necessary to meet ACK
  deadlines.

The nRF reference firmware must reuse the portable Thread service and the same
dataset and application behavior as ESP32-C6. Platform-specific code should be
limited to initialization, resources, radio, flash, identity, and task
placement.

### Integrated radios from other vendors

A new platform with a usable IEEE 802.15.4 radio implements the reviewed
OpenThread radio contract plus the platform services listed above. A porting
guide must document:

- PHY standard and Thread-version constraints;
- ACK, address-filter, source-match, security, retry, energy-scan, and timing
  capabilities;
- cancellation behavior for receive and transmit futures;
- radio queue overflow behavior;
- EUI-64 provenance;
- entropy qualification;
- sleep-clock behavior and wake latency;
- flash atomicity, erase time, and endurance;
- regulatory and transmit-power mapping.

### Spinel RCP

Use the upstream RCP abstraction for platforms without a local radio or for a
dual-SoC product. Transport adapters consume `embedded-io-async` UART or
`embedded-hal-async` SPI plus the required interrupt/reset pins.

RCP is a distinct build and recovery profile. It requires protocol-version
negotiation, transport framing tests, RCP reset detection, firmware compatibility
metadata, and host/RCP recovery policy. nRF5340 network-core designs should be
evaluated through this boundary instead of being treated as a single-core
nRF52840 variant.

## Features and resource profiles

Start with a minimal MTD configuration. Select features explicitly and keep
defaults off. Proposed additive SDK-facing features include:

- `thread`: base MTD stack;
- `ftd`: router-capable Full Thread Device;
- `joiner`;
- `srp-client`;
- `dns-client`;
- `cli` for development and certification tooling;
- `diagnostics` for controlled manufacturing builds;
- `rcp-host` for a Spinel-connected radio;
- `embassy-net` for the optional bare-IP bridge.

Commissioner and border router are later product capabilities, not aliases for
FTD. Border routing also needs an infrastructure interface, route exchange,
service discovery, NAT64/DNS64 policy where applicable, and substantially
different resource and interoperability evidence.

Record for every firmware profile:

- native OpenThread and crypto flash;
- Rust adapter and radio flash;
- static OpenThread resources and heap;
- task stacks and executor placement;
- radio and command queue capacities;
- settings RAM index and flash partition;
- UDP/SRP/DNS resources;
- optional `embassy-net` sockets and packet queues;
- peak heap, stack high-water marks, and worst-case callback latency.

## Capability and support reporting

Keep capability reporting conservative:

- `IEEE_802_15_4` is advertised after a usable radio driver exists;
- `THREAD` is advertised only after stack initialization, qualified entropy,
  stable identity, durable settings, reboot reattachment, and HIL attachment
  pass on that board;
- future router, sleepy-device, RCP, or border-router capabilities require
  separate flags or compatibility-matrix columns;
- silicon support alone never raises the board's support tier.

The compatibility matrix should track at least radio, MTD, FTD/router, sleepy
end device, RCP, coexistence profile, persistent reattachment, IPv6 UDP, and
HIL status.

## Implementation sequence

### 1. ADR and dependency proof

- Accept or revise ADR 0004.
- Pin an `openthread` release candidate with default features disabled.
- Compile a minimal MTD stack against the workspace's current Embassy,
  `esp-radio`, Rust, and target toolchains.
- Compile the same shared stack for nRF52840.
- Audit native source provenance, bindings, C/C++ compilers, crypto selection,
  libc symbols, allocator use, licenses, advisories, and duplicate dependencies.
- Measure a baseline ELF, static RAM, and heap before designing resource APIs.

This checkpoint may select a pinned fork or a local FFI implementation only if
it records the concrete upstream blocker, owner, divergence, and removal plan.

### 2. Portable model and host tests

- Implement bounded dataset and identity values.
- Implement secret redaction and zeroization tests.
- Add roles, lifecycle, snapshots, error categories, and capability traits.
- Add state-machine tests for start, attach, detach, leave, and failure.
- Export only `embedded-sdk-thread` from the facade.

### 3. ESP32-C6 Thread-only bring-up

- Add the platform radio adapter and dedicated firmware.
- Use a factory-derived EUI-64 and hardware RNG.
- Install a controlled test dataset, start Thread, and observe role changes.
- Verify form/join, ICMPv6, native UDP echo, detach, and reattach.
- RAM-only settings are permitted at this checkpoint, but the board remains
  without `Capabilities::THREAD` and the guide must call the image experimental.

### 4. Durable settings and service ownership

- Implement the synchronous settings journal and recovery scanner.
- Add the `ThreadRunner`/`ThreadHandle` actor boundary.
- Verify reboot reattachment, interrupted write recovery, full-store behavior,
  wipe, and factory reset.
- Add health snapshots and bounded failure counters.

### 5. Nordic portability proof

- Add the nRF52840 port, board, and dedicated reference firmware.
- Put timing-critical PHY/MAC work on the appropriate executor.
- Run the same dataset, lifecycle, persistence, and UDP application tests.
- Confirm that portable Thread and application code has no vendor-specific
  branches.

### 6. Portable application data plane

- Adapt native OpenThread UDP, DNS, and SRP to established protocol-facing
  traits.
- Add bounded IPv6 state to `embedded-sdk-networking`.
- Add the optional `embassy-net` bare-IP bridge and continuous state
  synchronizer only after native operation is stable.
- Prove TCP-based protocol reuse separately from native UDP readiness.

### 7. Power, coexistence, and expansion

- Add sleepy end-device polling, parent supervision, suspend, wake, and clock
  recovery tests.
- Validate explicit ESP32-C6 Thread/BLE and Thread/Wi-Fi profiles; publish their
  limitations and resource deltas independently.
- Add FTD/router support and router-specific HIL.
- Add Spinel RCP recovery and compatibility testing.
- Treat commissioner, border routing, Matter, and certification as separate
  reviewed vertical slices.

### 8. Support gate

- Update board capabilities and the compatibility matrix only after all Tier 2
  requirements for the selected profile pass.
- Publish resource budgets, OpenThread revision, build configuration, settings
  format version, and test evidence in the release compatibility manifest.

## Verification matrix

### Host and static checks

- portable crates compile with default features and remain `no_std`;
- SDK-owned portable and adapter crates forbid unsafe Rust;
- dataset TLV parsing rejects truncation, invalid lengths, duplicates where
  prohibited, and values above configured bounds;
- secret values never appear in `Debug`, `Display`, logs, or snapshots;
- lifecycle and error mapping cover every upstream role and relevant error;
- settings tests cover indexed values, replacement, deletion, wipe, capacity,
  corruption, truncation, duplicate records, interrupted compaction, and
  migration;
- fake-radio tests cover transmit cancellation, missing ACK, CCA failure,
  receive queue overflow, malformed frames, sleep, and energy scan;
- each board firmware builds independently for its exact feature profile;
- dependency and license checks include the complete native source input.

### Interoperability

Use a pinned OpenThread Border Router fixture and record its version and active
dataset. Test:

1. ESP forms a network and Nordic joins it, then reverse the roles.
2. Both devices join an independently created OTBR network.
3. Link-local and mesh-local IPv6 UDP work in both directions.
4. Border-router loss does not corrupt the dataset or settings.
5. Partition change and leader loss converge without restarting unrelated
   services.
6. Repeated detach, leave, recommission, and factory reset produce the expected
   persistent state.
7. Mixed OpenThread versions within the supported compatibility window
   interoperate.

### Hardware-in-the-loop

For both initial boards, capture:

- cold boot and warm reset reattachment;
- at least 20 attach/loss/recovery cycles;
- power removal during every settings mutation and compaction stage;
- invalid or exhausted settings storage;
- low-signal attachment, packet loss, and parent change;
- watchdog and health behavior when the radio runner stalls;
- resource high-water marks and callback latency;
- sleepy-device current and wake behavior when that profile is enabled;
- ESP32-C6 coexistence behavior for every advertised radio combination.

## File-level change map

| File or directory | Planned change |
| --- | --- |
| `docs/adr/0004-portable-openthread.md` | Record stack, ownership, persistence, networking, and capability decisions. |
| `crates/thread/` | Portable bounded Thread model and host-tested state. |
| `crates/thread-openthread/` | Shared OpenThread actor, mappings, resources, settings, and data-plane adapters. |
| `crates/networking/` | Add bounded IPv6 state without making it Thread-specific. |
| `crates/networking-embassy-net/` | Add IPv6 mapping required by the optional bridge. |
| `crates/embedded-sdk/` | Export only the portable Thread package. |
| `ports/espressif/esp32c6/` | Add opt-in IEEE 802.15.4/OpenThread hardware composition and coexistence constraints. |
| `ports/nordic/nrf52840/` | Add radio, identity, entropy, clocks, reset, and flash integration. |
| `boards/seeed/xiao-esp32c6/` | Own Thread RF/regulatory and settings-partition configuration. |
| `boards/nordic/nrf52840-dk/` | Define board hardware and storage layout. |
| `firmware/seeed/xiao-esp32c6-thread/` | Dedicated ESP MTD vertical slice. |
| `firmware/nordic/nrf52840-dk-thread/` | Equivalent Nordic MTD vertical slice. |
| `tests/host/` | Portable types, state, error, settings, and fake-radio coverage. |
| `tests/interoperability/` | Two-node and OTBR scenarios with pinned fixture metadata. |
| `tests/hil/` | Reattachment, persistence, RF, power, coexistence, and resource evidence. |
| `tools/xtask/` | Explicit build/flash/test profiles and dependency/resource reporting. |
| README, porting guides, compatibility matrix | Advertise only implemented and validated capability levels. |

## Acceptance criteria

The first portable milestone is complete when:

- one unchanged portable application forms or joins a Thread network on both
  ESP32-C6 and nRF52840;
- the only differences are board and platform composition;
- both targets retain their network data and reattach after power loss;
- both exchange IPv6 UDP with each other and a controlled OTBR;
- identity, entropy, settings, memory, and radio timing meet documented
  contracts;
- failures are observable without leaking datasets or credentials;
- support documentation states exact profiles and coexistence limitations;
- neither target advertises more capability than the tested evidence supports.

## Upstream references

- [OpenThread](https://github.com/openthread/openthread)
- [OpenThread porting guide](https://openthread.io/guides/porting)
- [Rust `openthread` integration](https://github.com/esp-rs/openthread)
- [OpenThread nRF528xx platform](https://github.com/openthread/ot-nrf528xx)
- [ESP32-C6 OpenThread guide](https://docs.espressif.com/projects/esp-idf/en/latest/esp32c6/api-guides/openthread.html)
- [ESP32-C6 RF coexistence guide](https://docs.espressif.com/projects/esp-idf/en/stable/esp32c6/api-guides/coexist.html)
