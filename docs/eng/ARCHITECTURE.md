# General architecture

## 1. System view

```
  +---------------------------- EDGE ----------------------------+
  |                                                              |
  |  [WIFI device] ------------ TLS/MQTT --------------+         |
  |                                                    |         |
  |  [OT_THREAD device] --+                            |         |
  |  [ZIGBEE device] -----+--- radio --- [GATEWAY]-----+         |
  |  [BLE device] --------+              (Rust, Linux) |         |
  |                                                    |         |
  |  [BLE device] --- GATT --- [MOBILE APP] -----------+         |
  +----------------------------------------------------+---------+
                                                       |
  +------------------------ CLOUD ----------------------v--------+
  |                                                              |
  |   +----------+    +--------------+    +------------------+   |
  |   |  broker  |--->|    ingest    |--->|  NATS JetStream  |   |
  |   |  (MQTT5) |<---|   (Rust)     |<---|   (event bus)    |   |
  |   +----------+    +------+-------+    +----+-------+-----+   |
  |                          |                 |       |         |
  |            +-------------v--+   +----------v--+  +-v------+  |
  |            | device-registry|   | rules/alert |  |  ota   |  |
  |            |   + shadow     |   |             |  |        |  |
  |            +-------+--------+   +-------------+  +---+----+  |
  |                    |                                 |       |
  |   +----------------v-----------------+        +------v-----+ |
  |   |  PostgreSQL + TimescaleDB        |        |  S3/MinIO  | |
  |   |  (registry, shadow, telemetry)   |        | (firmware) | |
  |   +----------------------------------+        +------------+ |
  |                    ^                                         |
  |            +-------+--------+                                |
  |            |   api (axum)   |<-- OIDC/JWT -- [WEB] [MOBILE]  |
  |            +----------------+                                |
  +--------------------------------------------------------------+
```

## 2. Responsibility boundaries

| Layer | Responsible for | NOT responsible for |
|---|---|---|
| **Device** | measurement, local control, offline buffering, secure boot | business logic spanning many devices, aggregations |
| **Gateway** | radio <-> IP bridging, offline backhaul, local fallback rules, Matter bridge | durable data storage, user authorization |
| **Cloud** | identity, shadow, rules, history, OTA, API | real-time measurement (<100 ms) |
| **Mobile** | provisioning, UI, local BLE control | source of truth about state |

**Rule:** the device must keep working correctly without the cloud for the period
defined in the product profile (default: 72 h of telemetry buffering, unlimited
local control).

## 3. Deployment model: two connectivity paths

**Path A — direct-to-cloud** (`COM_TYPE = WIFI`)
The device has an IP stack and connects directly to the broker over mTLS.
Simple topology, higher power draw, requires the Wi-Fi credentials of the user.

**Path B — via gateway** (`COM_TYPE = OT_THREAD | ZIGBEE | BLE`)
The device speaks PAN radio only. The gateway (Linux + Rust) translates to MQTT.
Low power draw, mesh, but requires selling/installing a gateway.

The application layer is **identical on both paths** — see
[PROTOCOL.md](PROTOCOL.md). The difference is confined to the `Link` trait.

**Path C — ecosystem (Matter)** is not a third route into our cloud, but a
parallel local control interface. By default it is implemented as a **bridge on
the gateway**: a single certified node exposes our devices to the ecosystems,
while telemetry, fleet management and OTA still travel over path A or B. See
[MATTER.md](MATTER.md) and ADR-012.

## 4. Technology stack — default decisions

### Device

| Element | Choice | Rationale |
|---|---|---|
| Async runtime | `embassy` | de facto standard, `no_std`, zero-alloc, good nRF/ESP/STM32 support |
| MCU (mesh/BLE) | nRF52840 / nRF5340 | best Rust support, multiprotocol |
| MCU (Wi-Fi) | ESP32-C6 / ESP32-S3 | `esp-hal` + `esp-wifi`, Wi-Fi 6 + 802.15.4 in the C6 |
| MCU (industrial/low-power, peripherals) | STM32 WB55 / U5 / L4 | `embassy-stm32`, wide availability, secure enclave in the U5 |
| BLE host | `trouble` | pure Rust, integrates with embassy |
| Thread | OpenThread as RCP/NCP | no mature Thread stack in Rust — see ADR-004 |
| Zigbee | vendor stack as NCP | no Zigbee stack in Rust — see ADR-004 |
| Storage | `sequential-storage` | log-structured KV on raw flash |
| Serialization | `postcard` | compact, `no_std`, the same `serde` as in the cloud |

**Portable SDK core:** `pkpu-device-core` and the product logic compile
unchanged on **all three** families (nRF, ESP32, STM32). Silicon is chosen per
product — by availability, price and peripherals, not at the cost of rewriting
the firmware. The differences are confined to `pkpu-platform-*` and `boards/`;
the port contract, the target matrix and known limitations —
[DEVICE.md](DEVICE.md) section 4, [ADR-011](DECISIONS.md).

### Cloud

| Element | Choice | Rationale |
|---|---|---|
| HTTP/WS API | `axum` + `tower` | tokio ecosystem, middleware |
| MQTT broker | `rumqttd` (as a library) | Rust, embeddable in our ingest process |
| Event bus | NATS JetStream | operationally simpler than Kafka, enough for ~100k msg/s |
| DB | PostgreSQL 16 + TimescaleDB | one database for everything, hypertable for telemetry |
| DB access | `sqlx` (compile-time checked) | queries verified at `cargo build` |
| Cache/presence | Valkey (Redis) | presence TTL, rate limiting, command dedup |
| Object storage | S3 / MinIO | firmware artifacts, exports |
| Observability | `tracing` + OTLP -> Grafana/Tempo/Loki | one API for logs and traces |

### Mobile

| Element | Choice |
|---|---|
| Core | `pkpu-core` (Rust) exported via **UniFFI** |
| UI | Kotlin/Compose + Swift/SwiftUI (thin) |
| BLE | native platform APIs, called through callbacks from the core |

## 5. Repository layout

A monorepo with **three workspaces** (different compilation targets do not
sensibly share a single `Cargo.lock`):

```
pkpu/
├── proto/                    # workspace 1 — contracts (no_std)
│   ├── pkpu-proto/           #   message types, DeviceId, enums from DEVICE.md
│   ├── pkpu-crypto/          #   ed25519 signatures, KDF, OTA manifest format
│   └── pkpu-schema/          #   generator: JSON Schema + OpenAPI from Rust types
│
├── firmware/                 # workspace 2 — no_std, target thumbv7em / riscv32
│   ├── pkpu-device-core/     #   PORTABLE: state machine, scheduler, OTA, storage
│   ├── pkpu-link/            #   Link trait + impl per radio stack: wifi, thread, zigbee, ble
│   ├── pkpu-hal/             #   hardware traits (Sensor, Actuator, PowerRail, Platform)
│   ├── platform/             #   NON-PORTABLE: trait impls per silicon
│   │   ├── pkpu-platform-nrf/
│   │   ├── pkpu-platform-stm32/
│   │   └── pkpu-platform-esp/
│   ├── boards/               #   BSP per board (pinout, clock, flash partitions)
│   └── apps/                 #   product binaries (e.g. apps/sensor-th/)
│
├── cloud/                    # workspace 3 — std, tokio, target x86_64 / aarch64
│   ├── pkpu-ingest/          #   broker + validation + publication onto the bus
│   ├── pkpu-registry/        #   device registry + device shadow
│   ├── pkpu-provisioning/    #   attestation, certificate issuance, claiming
│   ├── pkpu-ota/             #   campaigns, rollout, manifests
│   ├── pkpu-rules/           #   rules, alerts, webhooks
│   ├── pkpu-api/             #   REST/WS for web and mobile
│   ├── pkpu-gateway/         #   binary for the Linux edge (RPi / CM4) + Matter bridge
│   └── pkpu-cli/             #   operational and factory tooling
│
├── mobile/                   # pkpu-core (UniFFI) + iOS/Android shells
├── .github/workflows/        # pipelines per workspace (see CI.md)
└── docs/                     # this documentation
```

`proto/` is pulled into `firmware/` and `cloud/` through `path = "../proto/..."`,
not through a registry. A single commit changes the contract and both sides at
once.

Inside `firmware/` the same rule applies vertically: everything above
`platform/` is shared across nRF, STM32 and ESP32. Dependencies point downwards
only — `pkpu-device-core` does not know the name of any vendor.

## 6. Critical flows

### 6.1 Telemetry (device -> cloud)

```
sensor -> device-core (buffers in RAM/flash)
       -> Link::send(Frame::Telemetry)
       -> [gateway?] -> MQTT broker   topic: dev/{device_id}/tel
       -> ingest: identity verification + postcard decoding
       -> NATS: tel.{tenant}.{device_type}.{device_id}
       -> sink: TimescaleDB (batch COPY) | rules: evaluation | ws: push to UI
```

### 6.2 Command (cloud -> device)

```
API -> registry: write the desired state into the shadow (versioned)
    -> NATS: cmd.{device_id}
    -> ingest -> broker   topic: dev/{device_id}/cmd
    -> [SLEEP? queue until the next wake-up / poll]
    -> device executes -> publishes the reported state
    -> registry: reconcile desired vs reported, close the command
```

Commands are **idempotent** (`command_id` + dedup in Valkey) and have a TTL.
For `SLEEP_TYPE = SLEEP` the default TTL = 4 × the reporting interval.

### 6.3 Provisioning

See [DEVICE.md](DEVICE.md) — the full BLE and NFC sequence.

### 6.4 OTA

```
CI builds the firmware -> signs it with ed25519 -> uploads the artifact to S3
  -> pkpu-ota: campaign (cohort = filter on model / hw_rev / fw_version / site)
  -> wave rollout: 1% -> 10% -> 50% -> 100%, gated on the error metric
  -> device: downloads into slot B, verifies signature + hash, boots, confirms
  -> no confirmation within N boots -> the bootloader rolls back to slot A
```

## 7. Security — baseline model

- **Device identity**: the private key is generated **on the device** during
  manufacturing and never leaves the chip. The public key + `device_id` go into
  the factory registry. A secure element (ATECC608 / nRF CryptoCell) is preferred.
- **Transport**: mTLS to the broker (Wi-Fi / gateway), DTLS or OSCORE, or
  link-layer security within the PAN (Thread: MLE + AES-CCM; Zigbee: TC link key).
- **User authorization**: OIDC (self-hosted Keycloak or an external IdP),
  JWT carrying `tenant_id`, RBAC at the `site` level.
- **Multi-tenancy**: `tenant_id` in every table, enforced by Row Level Security
  in PostgreSQL — not by the application layer alone.
- **Secrets**: nothing in the repo. SOPS/age for configuration, Vault optionally.
- **Audit**: every desired-state change, every OTA campaign, every device
  claiming -> the `audit_log` table (append-only).

## 8. Verification and delivery

Three documents close the cycle: [TESTING.md](TESTING.md) — what we test and
where (host by default, hardware only where it has to be), [CI.md](CI.md) —
pipelines, releases and artifact signing, [SDK.md](SDK.md) — how the shared part
of the firmware evolves without the contract drifting apart.

Three rules tie them to the rest of the architecture:

- **a firmware release is not a rollout** — the signed artifact is produced in
  CI, shipping it to the fleet is a separate decision with wave gates,
- **a platform port is finished by conformance on hardware**, not by a
  successful build,
- **the wire contract is versioned separately from the crates** — devices in the
  field outlive backend releases.

---

## 9. Build order

| Stage | Scope | Outcome |
|---|---|---|
| 0 | `pkpu-proto` + DB schema + ADRs | frozen contract |
| 1 | `pkpu-ingest` + Timescale + one Wi-Fi device (ESP32-C6) | end-to-end telemetry |
| 2 | `pkpu-registry` + shadow + commands | bidirectional control |
| 3 | BLE provisioning + mobile `pkpu-core` | user onboarding |
| 4 | OTA + A/B bootloader | field serviceability |
| 5 | Thread/Zigbee + `pkpu-gateway` + second silicon (nRF52840) | battery devices (SLEEP), verification of core portability |
| 6 | Rules, alerts, dashboard, multi-tenancy | product |

Stages 1–4 on a single device type. Only then the second radio technology —
otherwise the `Link` abstraction will be built on guesswork rather than
experience. The same goes for portability: we design `pkpu-hal` so that a port
is possible (the "port contract" section), but we really bring up the second MCU
family in stage 5. Three platforms at once from day one give an abstraction
based on imagination.
