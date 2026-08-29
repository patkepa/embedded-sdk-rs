# DECISIONS — architecture decision log

Format: short ADRs. Status: `proposed` (to be confirmed) / `accepted` /
`superseded`. Everything below has the status `proposed` — these are proposals
to be discussed, not settled matters.

---

## ADR-001 — Rust across the whole stack

**Status:** proposed
**Decision:** firmware, cloud, gateway and the mobile core in Rust.
**Consequences:**
- (+) one type contract across all boundaries, no schema drift,
- (+) a whole class of memory bugs disappears from the firmware and from the
  network parsers,
- (−) radio stacks (Thread, Zigbee) do not exist in Rust — FFI is required
  (ADR-004),
- (−) a smaller talent pool on the market than C/Python/Go.

---

## ADR-002 — Monorepo, three workspaces

**Status:** proposed
**Decision:** one repo, separate `proto/`, `firmware/` and `cloud/` workspaces.
**Alternatives:** a separate repo per component (contract version drift),
a single workspace (target conflicts, `no_std` vs `std` in one `Cargo.lock`).

---

## ADR-003 — `postcard` on the wire, JSON on the API

**Status:** proposed
**Decision:** the same `serde` type, serialized as binary towards the device and
as JSON towards the UI.
**Alternatives:** CBOR (more standard, ~20% larger), Protobuf (a separate IDL
and code generation — a duplicated source of truth).

---

## ADR-004 — Thread and Zigbee as a radio co-processor (RCP/NCP)

**Status:** proposed
**Problem:** there are no production-grade Thread or Zigbee stacks in Rust.
**Decision:** the application in Rust on the host, the certified vendor stack on
a radio co-processor, communication over Spinel (Thread) / EZSP (Zigbee).
**Consequences:**
- (+) Thread Group / Zigbee Alliance certification is realistically achievable,
- (+) `unsafe`/C isolated behind a hardware boundary, not inside our process,
- (−) two chips = a higher BOM and higher power draw,
- (−) the single-chip alternative requires FFI to a C stack in the same MCU
  (cheaper, but it mixes C and Rust in one image).
**To be settled:** whether for battery-powered `SLEEP` devices we accept the
energy cost of two chips, or go for single-chip FFI.

---

## ADR-005 — MQTT 5 as the device–cloud protocol

**Status:** proposed
**Decision:** MQTT 5 + mTLS, with the broker embedded in `pkpu-ingest`.
**Alternatives:**
- CoAP + DTLS — more natural for Thread and `SLEEP` devices, less overhead, but
  a weaker tooling ecosystem,
- HTTP/3 + QUIC — good session resumption for battery devices, immature on MCUs.
**Open:** whether we expose a CoAP endpoint in parallel for `OT_THREAD`.

---

## ADR-006 — PostgreSQL + TimescaleDB as the only database

**Status:** proposed
**Decision:** registry, shadow, telemetry and audit in one instance.
**Revision threshold:** > 500k points/s of writes **or** > 10 TB of data after
compression. At that point: split telemetry out into ClickHouse, the registry
stays in Postgres.
**Alternatives rejected at this stage:** InfluxDB (weak joins with metadata),
ClickHouse from the start (two systems to maintain from day one).

---

## ADR-007 — Device identity based on a key generated on the chip

**Status:** proposed
**Decision:** an ed25519 private key generated on the device during
manufacturing, never leaving the chip; two CAs (factory offline, operational
online).
**Consequences:** the production line has to have a secured provisioning station
with access to the factory registry (mTLS, separate credentials).

---

## ADR-008 — A narrow (long) telemetry model

**Status:** proposed
**Decision:** one row = one measurement of one channel; numeric channels, with
the metadata in `device_type_channels`.
**Alternative:** a wide row per model (fewer rows, but a migration for every new
sensor and sparse columns for models with different sensor sets).

---

## ADR-009 — Distinguishing `SLEEPS` from `DISCONNECTED`

**Status:** proposed
**Decision:** the presence watchdog computes the window from
`expected_interval × tolerance` based on `sleep_type` from `device_types`.
**Rationale:** without it, battery devices generate constant false alarms, which
in practice leads the operator to switch the alerts off — that is, to losing the
entire value of the monitoring.

---

## ADR-010 — Build order: Wi-Fi first

**Status:** proposed
**Decision:** the first full end-to-end flow on an ESP32-C6 over Wi-Fi, and only
then Thread/Zigbee and the gateway.
**Rationale:** the `Link` abstraction designed on the basis of one working
implementation and a second genuinely implemented one — not on the basis of
predictions about four technologies at once.

---

## ADR-011 — A portable firmware core (one SDK on nRF, STM32 and ESP32)

**Status:** proposed
**Decision:** `pkpu-device-core`, `pkpu-hal`, `pkpu-link` and the product code in
`apps/` are portable across MCU families. All the silicon knowledge lives in
`pkpu-platform-{nrf,stm32,esp}` and `boards/`. The common denominator is
`embassy` + `embedded-hal` 1.0; choosing a platform means choosing a BSP and
compilation features.
**Rationale:**
- silicon availability is sometimes an imposed decision (price, lead time, a
  peripheral, a customer requirement) — it must not cost a firmware rewrite,
- Wi-Fi practically leads to ESP32, and low-power 802.15.4/BLE to nRF — without
  a shared core we would end up with two code bases in one product anyway,
- the same set of traits gives us a host build and core unit tests for free.

**Consequences:**
- (+) one state machine, one OTA client, one telemetry buffer — one set of bugs
  to fix instead of three,
- (+) a port to a new family is a finite checklist (DEVICE.md section 4.4), not a
  project from scratch,
- (−) a tie to the embassy ecosystem: an MCU without its support drops out of the
  matrix,
- (−) a fixed CI cost: building the whole target matrix on every PR,
- (−) the lowest common denominator — platform-specific peripherals (CryptoCell,
  PKA, DS) are available only through a trait or not at all,
- (−) Xtensa (ESP32-S3) requires a compiler fork; hence the RISC-V variants
  (C6/C3/H2) are the default.

**Deliberately non-portable:** the image format and the bootloader (hence
`platform` in the OTA manifest), the flash map, the energy budget.

**To be settled:** whether STM32 enters the matrix from the start, or only once a
product requires it. Maintaining a port without a board on the HIL rig is a
declaration of portability, not portability.

---

## ADR-012 — Matter as a compatibility surface, not an internal model

**Status:** proposed
**Problem:** Matter gives ecosystem integration (Apple, Google, Amazon) and
standard onboarding, but it has its own data model, its own identity, its own
commissioning and its own OTA. Adopting it as the internal model would mean
giving away the application layer of the device and losing what this platform
brings: historical telemetry, wave campaigns, multi-tenancy and audit.
**Decision:** Matter is an interface **to** the platform, not its core.
The default mode is a **bridge** on `pkpu-gateway` — one certified node exposing
many of our devices. `Dual` mode (its own Matter stack + our protocol) only for
selected consumer SKUs; `Native` mode — when the ecosystem is the entire value of
the product and the platform is not needed.
**Consequences:**
- (+) one certification instead of certifying every SKU,
- (+) battery and Zigbee devices enter the ecosystems at no cost in flash or in
  current,
- (+) the SDK core (ADR-011) stays untouched — the bridge lives on the gateway,
- (−) the bridge requires a gateway, so `WIFI` direct-to-cloud products stay
  outside the ecosystems or have to go `Dual`,
- (−) bridged devices have a limited feature set in the ecosystems,
- (−) a third breach of ADR-001: the Matter stack is C++ (`connectedhomeip`) or a
  vendor SDK; `rs-matter` is not a base for certification today.
**Binding already now, at near-zero cost:** measurement channels defined against
Matter cluster units and scales, `MatterMode` in the taxonomy, a registry that
allows a device paired into a third-party fabric and not claimed with us, a
`SoftwareVersion` derivable from our version.
**Irreversible if neglected:** the VID, the PAA/PAI chain and what the production
line writes. A device without a DAC cannot be retrofitted remotely — see
[MATTER.md](MATTER.md).
**To be settled:** whether any product in the plan is consumer-oriented enough to
justify `Dual` and full per-SKU certification.

---

# Open questions

To be settled before the code starts. Ordered by impact on the architecture.

### 1. Scale and business model
- Order of magnitude of the eventual fleet: hundreds, thousands, hundreds of
  thousands of devices?
- Our own product or a platform for many customers (is multi-tenancy genuinely
  needed from the start)?
- B2B (installations, the gateway included in the price) or B2C (Wi-Fi, no
  gateway)?

### 2. Cloud: self-hosted or managed
- Our own VPS/colocation vs AWS/GCP/Hetzner? It affects the choice between
  self-hosted NATS/Postgres and managed services.
- Are there data location requirements (GDPR, data kept in PL/EU)?

### 3. Hardware
- Has an MCU family already been chosen, or are we designing from scratch?
- Which families enter the portability matrix **from the start** (ADR-011)?
  Every added platform is a CI job and a board on the HIL rig — a fixed cost,
  not a one-off.
- Does any product require an ESP32-S3 (Xtensa, a toolchain fork), or are the
  RISC-V variants enough?
- Is any product battery-powered for long enough (2+ years) that the energy
  budget forces a single-chip architecture instead of an RCP (ADR-004)?
- Do we plan a secure element, or identity in the MCU flash?

### 4. Certification and compliance
- Do we plan Thread / Zigbee certification? For Matter the direction is chosen
  (ADR-012: a bridge instead of an internal model); what remains open is whether
  any product justifies `Dual` mode and per-SKU certification.
- When do we apply for a VID at the CSA, and do we build our own PAA/PAI chain or
  use the silicon vendor's chain? The decision precedes the first batch.
- CE/RED, cybersecurity (EN 18031 / Cyber Resilience Act) — the CRA requires,
  among other things, secure boot, updates and an SBOM. Worth designing in from
  the start.

### 5. Scope of the first product
- What is the first real use case? Without it, stage 1 of the roadmap has no
  concrete goal.

### 6. Team
- How many people, and what experience in Rust and in embedded? It determines
  whether we go RCP (simpler, more expensive in BOM) or single-chip FFI (cheaper,
  harder).
