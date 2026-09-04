# ADR 0004: Portable OpenThread Boundary

- Status: Proposed
- Date: 2026-09-04

## Context

The SDK identifies IEEE 802.15.4 and Thread as separate capabilities, but it
does not yet provide a Thread stack, portable Thread lifecycle model, durable
OpenThread settings, or an IPv6 application data path. The first implementation
must work on the existing ESP32-C6 target and prove portability on a Nordic
target without creating two vendor-specific Thread APIs.

OpenThread is already designed around a narrow platform abstraction layer. A
current Rust integration also provides a `no_std`, asynchronous wrapper around
OpenThread, a radio trait, static resources, local-radio adapters for ESP and
Nordic hardware, and a Spinel radio-co-processor path. Reimplementing these
layers in the SDK would enlarge the native and unsafe surface without improving
the application-facing portability boundary.

Thread is not merely another link driver. Applications and supervisors need to
retain Thread concepts such as operational datasets, commissioning, attachment,
device roles, network data, sleepy-device behavior, and border routing. At the
same time, application protocols should use established socket and I/O traits
rather than depend directly on a vendor radio or raw OpenThread FFI.

OpenThread also places unusual constraints on persistence and concurrency:

- its platform settings callbacks are synchronous and support multiple indexed
  values for a key;
- its C API is not generally thread-safe;
- radio acknowledgements and filtering may require tighter scheduling than the
  normal application executor provides;
- the stack owns several IPv6 addresses and dynamic Thread routes, which cannot
  be reduced to the existing single IPv4 configuration snapshot.

## Decision

1. `embedded-sdk-thread` owns the allocation-free, platform-independent Thread
   domain model. It contains bounded operational-dataset values, secret-safe
   configuration, roles, lifecycle state, snapshots, normalized error kinds,
   and focused controller capabilities. It does not depend on OpenThread,
   Embassy, an MCU HAL, or a board.
2. `embedded-sdk-thread-openthread` adapts a reviewed, pinned release of the
   upstream Rust `openthread` crate to the portable model. Upstream owns native
   compilation, generated bindings, and unsafe FFI. The SDK does not create a
   second `openthread-sys` crate unless the dependency proof finds an
   unresolvable safety, maintenance, or configuration gap.
3. The OpenThread instance has one logical owner. A `ThreadRunner` owns and
   polls it; application tasks use a `ThreadHandle` backed by bounded commands
   and watched snapshots. Raw OpenThread instance pointers and upstream handles
   do not escape the adapter.
4. Platform ports provide radio hardware, factory EUI-64 identity, qualified
   entropy, reset information, logging integration, and flash access. A local
   IEEE 802.15.4 implementation satisfies the upstream asynchronous radio
   contract. A platform without a suitable local radio may use a Spinel RCP
   over established asynchronous UART or SPI traits.
5. OpenThread persistence uses a dedicated synchronous, multi-value settings
   adapter with OpenThread's exact indexed-key semantics. The existing
   asynchronous single-value `KeyValueStore` is not used directly. A board owns
   the non-overlapping flash region; the OpenThread adapter owns key semantics,
   atomicity, sensitive-key handling, and recovery behavior.
6. OpenThread remains the source of truth for Thread IPv6 addresses, routes,
   DNS discovery, and network data. Native OpenThread UDP, DNS, and SRP adapters
   are the first application data path. An optional bare-IPv6 `embassy-net`
   bridge may be added for reusable TCP and socket-based protocols only after it
   continuously synchronizes OpenThread address and route changes.
7. `embedded-sdk-networking` gains additive, bounded IPv6 state. Thread
   attachment, IPv6 readiness, DNS readiness, and verified endpoint
   reachability remain separate facts.
8. Board and firmware packages select device role, OpenThread feature profile,
   memory limits, radio priority, storage partition, regulatory settings,
   coexistence policy, task topology, retry behavior, and product
   commissioning policy.
9. `Capabilities::IEEE_802_15_4` means usable radio hardware.
   `Capabilities::THREAD` means a complete, tested Thread implementation with
   identity, entropy, persistent settings, and attachment. A board does not
   advertise `THREAD` based on silicon capability alone.
10. The initial device profile is a Minimal Thread Device. Full Thread Device,
    Joiner, Commissioner, SRP, CLI, diagnostics, RCP, and border-routing support
    are explicit additive build capabilities with independent resource and test
    evidence.

## Consequences

- The same portable Thread API and OpenThread integration can be reused across
  ESP32-C6, nRF52840, integrated radios from other vendors, and external RCPs.
- Vendor ports remain concerned with hardware rather than datasets,
  commissioning policy, sockets, or application behavior.
- The SDK's own portable and adapter crates can continue to forbid unsafe Rust,
  while the native dependency is isolated and reviewed as a supply-chain and
  FFI boundary.
- OpenThread calls are serialized and callbacks cannot await. Callback data
  needed later must be copied into bounded Rust-owned storage before returning.
- The radio may require a higher-priority executor and a software-MAC wrapper
  when hardware does not offload acknowledgement timing or address filtering.
- Thread persistence cannot initially share the generic application KV store.
  It requires a dedicated format, partition, power-loss tests, and migration or
  factory-reset policy.
- Application protocols can use native Thread datagrams before a second IPv6
  stack is introduced. General TCP reuse through `embassy-net` remains a
  separately validated adapter rather than an assumed property of attachment.
- OpenThread native code, crypto configuration, C library shims, static
  resources, and any allocator use become explicit firmware resource and
  release inputs.
- ESP32-C6 Wi-Fi, BLE, and 802.15.4 share one RF path. Combined profiles need
  explicit coexistence validation and cannot inherit support status from a
  Thread-only image.

## Alternatives considered

- Maintaining independent ESP-IDF and Nordic Thread integrations was rejected
  because their lifecycle, error, feature, and data-plane behavior would drift
  and application code would not prove vendor portability.
- Implementing a new in-tree OpenThread FFI and platform layer was rejected as
  the default because an upstream Rust integration already owns that work. A
  local fork remains a temporary option if the dependency proof identifies a
  concrete blocker and includes an upstreaming plan.
- Treating Thread as a raw `embassy-net` IEEE 802.15.4 driver was rejected
  because OpenThread must own the MAC, 6LoWPAN, MLE, mesh routing, security, and
  Thread network data.
- Exposing the upstream `OpenThread` handle directly was rejected because it
  would leak a pre-1.0 dependency, weaken serialization guarantees, and spread
  FFI lifetime assumptions into firmware and product code.
- Adapting `KeyValueStore` directly was rejected because its asynchronous,
  single-value contract does not implement OpenThread's synchronous indexed
  settings semantics.
- Making `embassy-net` the first Thread socket path was rejected because a
  second IPv6 stack must mirror multiple dynamic OpenThread addresses and
  routes correctly. Native OpenThread UDP provides a smaller first vertical
  slice.

## References

- [OpenThread porting guide](https://openthread.io/guides/porting)
- [OpenThread platform abstraction APIs](https://openthread.io/guides/porting/implement-platform-abstraction-layer-apis)
- [Rust `openthread` integration](https://github.com/esp-rs/openthread)
- [ESP32-C6 OpenThread modes](https://docs.espressif.com/projects/esp-idf/en/latest/esp32c6/api-guides/openthread.html)
- [ESP32-C6 RF coexistence](https://docs.espressif.com/projects/esp-idf/en/stable/esp32c6/api-guides/coexist.html)
- [OpenThread nRF528xx platform](https://github.com/openthread/ot-nrf528xx)
