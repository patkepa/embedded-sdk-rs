# Repository Architecture

## Status

- Status: Proposed
- Scope: Entire `embedded-sdk-rs` repository
- Audience: Maintainers, contributors, platform integrators, and product teams

## Objective

This repository is intended to provide a professional Rust SDK for building
advanced embedded devices with Embassy. It should support reusable drivers,
networking, Bluetooth, Thread, Wi-Fi, cellular connectivity, cloud services,
device management, security, observability, and firmware updates across
multiple chip families.

The repository is a monorepo, but it must not become a single universal crate.
It should be a Cargo workspace containing small, independently testable
libraries, platform ports, board support packages, and deployable firmware
applications.

Portability comes from standard traits, capability boundaries, and dependency
direction. It must not depend on a growing collection of platform-specific
`cfg` branches in otherwise portable code.

The support promise is:

> The SDK can be ported to any platform that provides the necessary Rust
> target support, Embassy or compatible HAL integration, memory, radio, and
> security capabilities.

A literal guarantee of support for every platform is neither testable nor
maintainable. Supported targets must instead be recorded in an explicit
compatibility matrix and assigned a support tier.

## Proposed Repository Structure

```text
embedded-sdk-rs/
├── Cargo.toml                    # Virtual workspace, resolver = "3"
├── Cargo.lock                    # Committed and used for SDK releases
├── rust-toolchain.toml           # Pinned stable Rust toolchain
├── rustfmt.toml
├── deny.toml                     # License, advisory, and dependency policy
├── justfile                      # Small developer-facing command wrapper
├── LICENSE-APACHE
├── LICENSE-MIT
├── SECURITY.md
├── CONTRIBUTING.md
├── CODEOWNERS
│
├── crates/
│   ├── embedded-sdk/             # Optional convenience facade
│   ├── core/                     # Errors, IDs, capabilities, lifecycle
│   ├── runtime/                  # Embassy tasks and supervision primitives
│   ├── config/                   # Versioned configuration and validation
│   ├── storage/                  # Key-value, blob, and flash abstractions
│   ├── security/                 # Identity, credentials, crypto abstractions
│   ├── telemetry/                # Logs, metrics, traces, crash records
│   ├── provisioning/             # Factory and field provisioning
│   ├── ota/                      # Update state machine and rollback policy
│   │
│   ├── networking/
│   │   ├── core/                 # Link, IP, DNS, and socket interfaces
│   │   ├── embassy-net/          # embassy-net integration
│   │   ├── ethernet/
│   │   ├── wifi-cyw43/
│   │   ├── wifi-esp-hosted/
│   │   ├── cellular/
│   │   └── ppp/
│   │
│   ├── wireless/
│   │   ├── ble-core/             # Application-facing BLE services
│   │   ├── ble-trouble/          # TrouBLE host integration
│   │   ├── thread-core/
│   │   ├── openthread-sys/       # Audited FFI boundary
│   │   └── openthread-platform/  # OpenThread PAL implementation
│   │
│   ├── protocols/
│   │   ├── mqtt/
│   │   ├── coap/
│   │   ├── http/
│   │   ├── dns/
│   │   ├── mdns/
│   │   ├── sntp/
│   │   └── serialization/
│   │
│   ├── cloud/
│   │   ├── core/                 # Provider-independent device management
│   │   ├── aws-iot/
│   │   ├── azure-iot/
│   │   └── custom-mqtt/
│   │
│   └── drivers/
│       ├── environmental/
│       ├── motion/
│       ├── ranging/
│       ├── displays/
│       ├── storage/
│       ├── secure-elements/
│       └── power/
│
├── ports/                        # Vendor and chip-family implementations
│   ├── nrf/
│   ├── stm32/
│   ├── rp/
│   ├── esp/
│   ├── nxp/
│   └── host/                     # Linux/macOS simulation and testing
│
├── boards/                       # One package per physical board
│   ├── nrf52840-dk/
│   ├── nrf5340-dk/
│   ├── stm32h7-nucleo/
│   ├── rp-pico-w/
│   └── xiao-esp32c6/
│
├── firmware/                     # Products and reference applications
│   ├── connected-sensor/
│   ├── thread-end-device/
│   ├── ble-provisioned-node/
│   └── industrial-gateway/
│
├── examples/                     # Small, single-capability demonstrations
│   ├── ble/
│   ├── wifi/
│   ├── thread/
│   ├── cloud/
│   ├── sensors/
│   └── ota/
│
├── tests/
│   ├── host/
│   ├── integration/
│   ├── interoperability/
│   ├── hil/                      # Hardware-in-the-loop suites
│   ├── fuzz/
│   └── fixtures/
│
├── schemas/                      # Telemetry, config, and update schemas
├── tools/
│   └── xtask/                    # Build, flash, test, package, and release
│
├── docs/
│   ├── architecture/
│   ├── adr/                      # Architecture decision records
│   ├── porting/
│   ├── security/
│   ├── certification/
│   └── compatibility/
│
└── .github/
    ├── workflows/
    ├── ISSUE_TEMPLATE/
    └── dependabot.yml
```

This tree describes the intended destination. Directories should be introduced
when they gain an owned, tested component; empty placeholder crates should not
be created merely to mirror the design.

## Dependency Architecture

Dependencies must point inward toward portable abstractions:

```text
firmware
   │
   ├── boards ──────────────── ports and vendor HALs
   │
   └── services, cloud, and protocols
              │
       networking and wireless
              │
       portable drivers and core traits
              │
      embedded-hal, embedded-io, Embassy
```

| Area | May depend on | Must not depend on |
| --- | --- | --- |
| Core | `no_std` and stable portable traits | MCU, pins, clocks, or vendor HAL |
| Drivers | `embedded-hal` and `embedded-hal-async` | A specific board |
| Protocols | `embedded-io` and socket abstractions | A Wi-Fi chipset or MCU |
| Ports | Vendor HAL, FFI, and chip details | Product business logic |
| Boards | Pins, clocks, interrupts, memory, partitions | Cloud or application policy |
| Firmware | Components required by that product | Other boards or products |

Portable drivers should use the standard `embedded-hal` family of traits before
the SDK defines new interfaces. Protocol implementations should generally use
`embedded-io` or focused socket abstractions. SDK-specific traits are appropriate
only where an accepted ecosystem abstraction does not exist or where the SDK
needs to express lifecycle, health, security, or device-management guarantees.

## Workspace Policy

The root should become a virtual Cargo workspace:

```toml
[workspace]
resolver = "3"
members = [
    "crates/*",
    "crates/*/*",
    "ports/*",
    "boards/*",
    "firmware/*",
    "tests/*",
    "tools/*",
]
default-members = [
    "crates/embedded-sdk",
    "tests/host",
    "tools/xtask",
]

[workspace.package]
edition = "2024"
license = "MIT OR Apache-2.0"
repository = "https://github.com/your-org/embedded-sdk-rs"

[workspace.dependencies]
embedded-hal = "1"
embedded-hal-async = "1"
embedded-io = { version = "...", default-features = false }
embedded-io-async = { version = "...", default-features = false }
```

The exact dependency versions must be selected and pinned when the workspace is
implemented. The architecture document intentionally does not prescribe
versions that will become stale.

The following rules apply throughout the workspace:

- Portable crates are `no_std` by default.
- `alloc` and `std` are explicit, additive features.
- Features must add functionality; enabling one feature must not silently
  disable or replace another.
- There is no workspace-wide `stm32`, `nrf`, `rp`, or `esp` feature.
- The exact MCU is selected only by its board or platform package.
- Each board/product combination is a separate firmware package.
- Embedded dependencies use `default-features = false` unless their defaults
  have been deliberately reviewed.
- Firmware packages are built and tested independently to avoid accidental
  Cargo feature unification across incompatible targets.
- `.cargo/config.toml` must not select a global default target, because doing so
  prevents normal host builds and tests.
- Target runners, linker arguments, and flash commands are associated with an
  explicit board through `xtask` or board-scoped configuration.
- Libraries do not select panic behavior. Firmware binaries select the panic
  handler and release profile appropriate to the product.

## Core SDK Responsibilities

The core SDK should remain small. It may define:

- Device, hardware, firmware, and build identities.
- A capability model that lets applications discover supported functions.
- Common lifecycle and health states.
- Stable error categories without erasing useful source errors.
- Time, cancellation, shutdown, and service-health interfaces where ecosystem
  traits are insufficient.
- Versioned wire and persistent-data formats.

It should not contain concrete sensors, pin mappings, network stacks, cloud
providers, or board initialization.

## Runtime and Service Model

The runtime layer should provide common patterns around Embassy rather than
hide Embassy behind an imitation executor API. It should cover:

- Static allocation of task resources.
- Explicit task startup and ownership.
- Cancellation and controlled shutdown where supported.
- Watchdog health voting for critical tasks.
- Bounded channels and documented backpressure behavior.
- Restart policies for recoverable services.
- Consistent reporting of fatal and degraded states.

Every long-running task must document its memory ownership, queue capacity,
failure behavior, and watchdog expectations. Unbounded queues and silent task
termination are not permitted.

## Connectivity Boundaries

Bluetooth, Wi-Fi, Thread, Ethernet, and cellular links must not be forced into a
single lowest-common-denominator interface.

- Wi-Fi, Ethernet, cellular, and Thread can provide IP connectivity.
- BLE GATT is a service and attribute transport with a separate application API.
- BLE commissioning may configure an IP interface, but BLE is not itself an IP
  socket interface.
- Thread APIs must preserve Thread concepts such as operational datasets,
  joining, roles, network attachment, and border routing.
- Network management should coordinate link availability, addressing, DNS,
  routing, and reconnection without taking ownership of application protocols.

### Bluetooth

The preferred structure separates reusable application-facing GATT services
from the host stack and controller adapter. A TrouBLE-based integration belongs
in `wireless/ble-trouble`, while controller selection belongs to the platform or
board package.

### OpenThread

OpenThread integration should be an isolated FFI subsystem:

- `openthread-sys` owns generated bindings, native build integration, and the
  smallest possible unsafe surface.
- `openthread-platform` implements the OpenThread Platform Abstraction Layer.
- The relevant platform port supplies radio, time, entropy, persistent settings,
  reset, logging, and optional bus integration.
- Board packages select radio configuration, storage regions, regulatory
  settings, and device-specific identifiers.
- FFI callbacks must have documented ownership, threading, and lifetime rules.

## Cloud Architecture

Cloud support should be divided into provider-neutral device behavior and thin
provider adapters.

The provider-neutral layer should own concepts such as:

- Device identity and enrollment state.
- Desired/reported configuration reconciliation.
- Command handling and acknowledgement.
- Telemetry envelopes and batching.
- Connection state, retry policy, and offline buffering.
- Firmware update notifications and status reporting.

AWS IoT, Azure IoT, or another cloud adapter should translate those concepts to
provider-specific MQTT topics, payloads, authentication, provisioning, and
device-management conventions. The networking and TLS implementation must not
be embedded directly in application business logic.

## Driver Policy

The repository should not rewrite every available sensor driver. For a new
device, maintainers should follow this order:

1. Use a maintained `embedded-hal` driver directly.
2. Add a thin SDK adapter when lifecycle, telemetry, power management, or error
   normalization is required.
3. Contribute generally useful fixes to the upstream project.
4. Add an in-tree driver only when no suitable implementation exists or when
   proprietary hardware requires repository ownership.

Every in-tree driver must provide:

- Blocking and async operation where both are meaningful.
- No Embassy dependency unless concurrency or timing genuinely requires it.
- Host tests using fake or recorded buses.
- Supported part numbers, chip revisions, and datasheet revision.
- Defined initialization, reset, timeout, and power-state behavior.
- Error types that preserve the underlying bus or pin error.
- At least one hardware-in-the-loop validation on a supported board.

## Platform and Board Separation

A platform port represents a chip or chip family. It may configure the vendor
HAL, clocks, interrupts, time driver, radio controller, DMA, flash, and other
chip-level services.

A board package represents a physical PCB. It owns:

- Pin assignments and alternate functions.
- External oscillators and clock assumptions.
- Attached sensors, radios, storage, and secure elements.
- Flash partition and bootloader layout.
- Power rails, enables, and wake sources.
- Board revision differences.
- Probe, runner, and flashing metadata.

A firmware package represents a deployable product. It composes a board with
portable services and product policy. Board packages must not decide which
cloud provider, telemetry interval, or business behavior the product uses.

## Configuration and Secrets

Configuration should be divided into three categories:

- Build-time hardware configuration: board wiring, memory maps, and enabled
  peripherals.
- Product configuration: protocol selection, resource limits, and feature set.
- Runtime configuration: provisioned network, cloud, and user settings.

Persistent configuration must be versioned and migration-tested. Resource
limits such as queue depth, connection count, and packet size should be
compile-time constants where that improves determinism.

Credentials and private keys must never be committed to the repository or
encoded in Cargo features. Production provisioning must be separated from
normal firmware development and should use a secure element or protected MCU
storage when the platform provides it.

## Support Tiers

Every supported board and platform must have an owner and tier:

### Tier 1: Product Supported

- Built in continuous integration.
- Tested on real hardware for every release.
- Covered by documented flash and RAM budgets.
- Security and update paths are release-gated.
- Regressions block a release.

### Tier 2: Maintained

- Built in continuous integration.
- Tested periodically on real hardware.
- Core functionality is expected to work.
- A regression may be allowed temporarily with a documented issue and owner.

### Tier 3: Community

- Best-effort compile or community testing.
- No guaranteed release validation.
- Limitations and ownership are explicitly documented.

Unsupported boards may exist as examples, but must not be presented as
production-ready.

## Verification Strategy

### Host Tests

Host tests should cover portable state machines, protocol behavior,
configuration migrations, serialization, retry logic, storage recovery, and
drivers through fake `embedded-hal` implementations.

### Compile Matrix

Continuous integration should compile each supported board and firmware package
independently. It should include:

- Debug and production profiles where relevant.
- Feature-minimal builds.
- `no_std` validation.
- Documentation builds for public crates.
- Flash and RAM size budget checks.

### Integration and Interoperability

Integration suites should exercise real protocol peers and cloud test accounts.
Interoperability coverage should include applicable BLE, Thread, IP, MQTT,
CoAP, TLS, and OTA scenarios.

### Hardware-in-the-Loop

The hardware lab should test:

- Boot and reset reason reporting.
- Radio connection, loss, and reconnection.
- Suspend, low-power entry, wake, and clock recovery.
- Watchdog handling and service failure.
- Brownout and power loss during storage writes.
- Power loss at each stage of an OTA update.
- Rollback and recovery from invalid firmware.
- Sensor presence, timeout, and bus recovery.

### Fuzzing and Fault Injection

Fuzz targets should cover externally controlled input, especially network
packets, message payloads, persistent records, update manifests, and FFI
boundaries. Storage and update tests must include truncated, corrupted,
duplicated, reordered, and interrupted operations.

## Security Requirements

Security is a system property and must be part of the initial architecture.
The repository should include:

- A published vulnerability reporting policy.
- Threat models for provisioning, connectivity, storage, boot, and update.
- Signed firmware verification and rollback protection.
- Unique device identity and secure credential provisioning.
- Cryptographically secure entropy requirements per platform.
- Debug-port policy for development and production devices.
- Protection and controlled erasure of secret material.
- Dependency advisory, license, source, and duplicate-version policy.
- Review requirements for cryptography, native code, and unsafe Rust.
- Security event and reset telemetry that does not disclose secrets.

Portable crates should forbid unsafe Rust. Unsafe code should be restricted to
small port, HAL adapter, or `-sys` crates, with documented safety invariants and
designated reviewers.

## Observability

All firmware should expose a consistent minimum diagnostic set:

- SDK, firmware, bootloader, board, and hardware revision.
- Build identifier and source revision.
- Reset and boot reason.
- Current lifecycle and connectivity state.
- Watchdog and task-health status.
- Bounded counters for dropped messages, reconnects, storage failures, and
  update attempts.
- A persisted crash record where the platform permits it.

Logging backends such as `defmt` or `log` should be adapters. Portable services
should emit structured events through SDK interfaces rather than depend on a
specific transport.

## Dependency and Supply-Chain Policy

- Prefer stable releases from crates.io.
- Pin the Rust toolchain and commit `Cargo.lock` for reproducible SDK builds.
- Temporary Git dependencies require an owner, reason, pinned revision, and
  planned removal condition.
- Forks require documented divergence and an upstreaming strategy.
- Vendored dependencies are generated release inputs, not the normal source of
  truth.
- Releases produce an SBOM, hashes, signed artifacts, and provenance metadata.
- Automated dependency updates must pass the full compile and test matrix.
- Native C/C++ dependencies require the same license, vulnerability, and
  reproducibility controls as Rust crates.

## Release Model

The repository should use an SDK release train:

- Tag the complete repository, for example `sdk-v1.4.0`.
- Publish reusable public crates with semantic versioning.
- Keep board, test, and product firmware crates unpublished.
- Define a support window for each SDK release.
- Publish migration notes for breaking API, storage, wire-format, bootloader,
  or provisioning changes.
- Treat bootloader/application compatibility as an explicit versioned contract.

Each release should generate a machine-readable compatibility manifest that
records:

- SDK and toolchain versions.
- Source revision and dependency lock hash.
- Supported boards and platform tiers.
- Bootloader and partition-layout versions.
- Enabled protocol versions.
- Flash, RAM, and persistent-storage usage.
- Test and certification evidence.

## Documentation Set

As the repository develops, documentation should include:

- Architecture overview and dependency rules.
- Architecture decision records for consequential choices.
- A porting guide for new chip families.
- A board integration guide.
- A driver authoring guide.
- Security architecture and threat models.
- Provisioning and manufacturing procedures.
- OTA and recovery design.
- Compatibility and support-tier matrix.
- Release and long-term support policy.
- Example-led guides for each major capability.

## Initial Delivery Plan

Breadth should follow proven end-to-end architecture. The first two reference
implementations should be complete vertical slices.

### Phase 1: Workspace Foundation

- Convert the root package into a virtual workspace.
- Add core, runtime, host port, host tests, and `xtask` packages.
- Establish formatting, linting, dependency policy, documentation, and CI.
- Define support tiers and the compatibility manifest format.

### Phase 2: First Vertical Slice

Use one well-supported board, preferably Nordic, to implement:

```text
BLE provisioning
    -> Wi-Fi or Thread attachment
    -> MQTT connection
    -> structured telemetry
    -> signed OTA
    -> rollback and recovery
```

### Phase 3: Portability Proof

Implement the same portable application services on a board from another chip
vendor. Product logic and cloud behavior should remain largely unchanged; only
the board and platform composition should differ.

### Phase 4: Expansion

Add new drivers, transports, cloud adapters, and platforms incrementally. A new
component is considered supported only when it has an owner, documentation,
tests, resource budgets, and an assigned support tier.

## Architectural Success Criteria

The architecture is working when:

- Portable application and service code is reused across different vendors.
- A new board can be added without editing portable crates.
- A new sensor driver can be tested on a host without an MCU.
- Cloud adapters can change without changing link drivers.
- BLE and Thread retain their protocol-specific capabilities.
- Firmware resource usage is deterministic and release-gated.
- Power loss, malformed input, network failure, and failed updates have defined
  recovery behavior.
- Every production claim is backed by automated or hardware test evidence.

Enterprise grade means that supported combinations are explicit, secure,
observable, reproducible, and continuously verified. It does not mean that
every possible platform or peripheral is claimed to work without evidence.
