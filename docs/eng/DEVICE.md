# 1. DEVICE — the device stack

This document describes the firmware layer. Device classification is the product
of five dimensions: **SLEEP_TYPE × COM_TYPE × PROV_TYPE × MATTER_MODE × STATE**.
The same enums live in `pkpu-proto` and in the database — see
[PROTOCOL.md](PROTOCOL.md) and [DATA.md](DATA.md).

---

## 1. Taxonomy

### SLEEP_TYPES

| Variant | Power | Radio | Command latency | Role in the mesh |
|---|---|---|---|---|
| `NONSLEEP` | mains / permanent | RX always on | < 1 s | router / FFD |
| `SLEEP` | battery | RX periodically (poll / interval) | up to one wake-up cycle | end device / SED |

Design consequences of `SLEEP`:
- commands are **queued** on the parent or in the cloud, not pushed,
- telemetry is sent in batches, not as a stream,
- no continuous TLS — the session is resumed from a session ticket / PSK,
- the energy budget is a functional requirement (see section 8).

### COM_TYPES

| Variant | Stack | Path to the cloud | Notes |
|---|---|---|---|
| `WIFI` | IP + TLS + MQTT5 | direct | highest power draw, simplest topology |
| `OT_THREAD` | 802.15.4 + 6LoWPAN + IPv6 | through a Border Router | native IP in the mesh, CoAP/DTLS |
| `ZIGBEE` | 802.15.4 + ZCL | through the coordinator | cluster model, non-IP |
| `BLUETOOTH/BLE` | GATT | through a phone or a gateway | also the provisioning channel |

### MATTER_MODE

The relation to the Matter ecosystem — a dimension **orthogonal** to `COM_TYPE`,
because Matter is an application layer above IPv6, not a link technology.

| Variant | Meaning | Cost in firmware |
|---|---|---|
| `NONE` | a device outside the Matter world | zero |
| `BRIDGED` | exposed by a bridge on the gateway | zero — the bridge lives on the gateway side |
| `DUAL` | its own Matter stack + our protocol | flash, RAM, a second radio schedule, certification per SKU |
| `NATIVE` | Matter only | no telemetry or wave campaigns of ours |

`BRIDGED` or `NONE` by default — rationale and full consequences in
[MATTER.md](MATTER.md) and ADR-012.

### PROV_TYPES

| Variant | Medium | When |
|---|---|---|
| `BLE` | GATT service, ephemeral advertising | a device with a BLE radio, onboarding from a phone |
| `NFC` | NDEF on a tag / NTAG with I²C to the MCU | a device without a UI, tap-to-provision, also out-of-box |

### STATES

The state reported to the cloud (`device.state`):

| State | Meaning | Who sets it |
|---|---|---|
| `ONLINE` | session active, the device responds | ingest on connect / keepalive |
| `DISCONNECTED` | no session outside the expected window | presence watchdog in the cloud |
| `SLEEPS` | no session, but **as scheduled** | registry, based on `SLEEP_TYPE` |

`SLEEPS` and `DISCONNECTED` are distinguished by a tolerance window:
`last_seen + expected_interval × tolerance` (×3 by default). Without that
distinction, battery devices generate false alerts.

---

## 2. Device identity

```rust
pub struct DeviceIdentity {
    pub device_id:  DeviceId,      // UUIDv7, 128-bit, assigned during manufacturing
    pub short_id:   u32,           // short form for PAN addressing, assigned by the cloud
    pub serial:     Serial,        // factory number, printed/QR/NFC
    pub model:      ModelId,       // product type
    pub hw_rev:     HwRev,
    pub sleep_type: SleepType,
    pub com_type:   ComType,
    pub prov_type:  ProvType,
    pub pubkey:     [u8; 32],      // ed25519, the private key never leaves the chip
}
```

- `device_id` is **immutable** for the entire life of the device (including
  after a factory reset and after a change of owner).
- `short_id` exists only to save bytes in radio frames; the
  `short_id -> device_id` mapping is held by the gateway and the cloud.
- Storage: OTP zone / protected flash region, protected from being wiped during
  OTA.

---

## 3. Firmware layers

```
+-----------------------------------------------------------+
| apps/<product>          product logic, configuration        |
+-----------------------------------------------------------+
| pkpu-device-core        state machine, scheduler, shadow,   |
|                         telemetry buffer, OTA client        |
+---------------------------+-------------------------------+
| pkpu-link (Link trait)    | pkpu-hal (hardware traits)      |
|  wifi | thread | zigbee   |  Sensor, Actuator, PowerRail,   |
|  | ble                    |  Rng, Clock, Storage            |
+---------------------------+-------------------------------+
| pkpu-platform-{nrf,stm32,esp}   pkpu-hal trait impls        |
+-----------------------------------------------------------+
| boards/<board>          BSP: pinout, clocks, partitions     |
+-----------------------------------------------------------+
| embassy + vendor HAL (embassy-nrf/-stm32 / esp-hal)         |
+-----------------------------------------------------------+
| bootloader (A/B, signature verification, rollback)          |
+-----------------------------------------------------------+
```

Rule: `pkpu-device-core` **does not know** the radio technology or the hardware.
It knows traits only. That is what lets the same core compile for host tests
with a `Link` backed by an in-memory channel — and, for the same reason, onto
STM32, ESP32 and nRF without changing a line of code (see section 4).

The horizontal (radio) and vertical (silicon) split separates two **independent**
axes of variability: `pkpu-link` changes with the radio stack, `pkpu-platform-*`
with the MCU. A product picks one from each axis.

### The `Link` trait

```rust
pub trait Link {
    type Error;

    /// Establish connectivity (network join, DHCP, handshake, attach to a parent).
    async fn connect(&mut self, creds: &NetworkCreds) -> Result<(), Self::Error>;

    /// Send an application frame. The implementation decides on fragmentation.
    async fn send(&mut self, frame: &Frame<'_>) -> Result<(), Self::Error>;

    /// Receive a frame. For SLEEP: poll the parent; for NONSLEEP: listen.
    async fn recv<'a>(&mut self, buf: &'a mut [u8]) -> Result<Frame<'a>, Self::Error>;

    /// Link characteristics — the core tunes batch size and frequency to them.
    fn profile(&self) -> LinkProfile;   // mtu, rtt_typ, push-capable?, energy cost
}
```

`LinkProfile` is what allows a single core to behave sensibly both on a
1500 B/Wi-Fi link and on an ~80 B/Zigbee link.

---

## 4. Portability across MCU platforms

**Baseline assumption:** the SDK firmware has one **shared part, portable across
MCU families**. The same `pkpu-device-core` and the same product logic compile
unchanged on STM32, ESP32 and nRF. Choosing a platform means choosing a BSP and
compilation `features`, never branching the product code.

The practical consequence: a new product on a different MCU (because that one was
available, cheaper, or has the peripheral you need) is not new firmware — it is a
new board and a new `product.toml`.

### 4.1 What is portable and what is not

| Layer | Portable | Note |
|---|---|---|
| `apps/<product>` | yes | depends only on the traits from `pkpu-hal` and `pkpu-link` |
| `pkpu-device-core` | yes, 100% | zero `cfg(target_arch)`, zero dependencies on vendor HALs |
| `pkpu-link` | API yes, impl per **radio stack** | `thread` over Spinel works on any MCU with a co-processor |
| `pkpu-hal` | yes (traits only) | the contract definition, no implementation |
| `pkpu-platform-{nrf,stm32,esp}` | **no** — by definition | all silicon knowledge lives here |
| `boards/<board>` | **no** | pinout, clocks, flash map, regulators |
| bootloader | **no** | a different mechanism and image format per family (4.5) |

**Hard rule:** `#[cfg(target_arch)]`, `#[cfg(target_os)]` and vendor-named
features are allowed **exclusively** in `pkpu-platform-*` and `boards/`.
Such a `cfg` appearing in the core or in `apps/` is a signal that a trait is
missing from `pkpu-hal` — the fix is adding the trait, not adding the branch.

### 4.2 Target matrix

| Family | Example MCU | Rust target | HAL | Toolchain |
|---|---|---|---|---|
| nRF52 | nRF52840 | `thumbv7em-none-eabihf` | `embassy-nrf` | stable |
| nRF53 | nRF5340 | `thumbv8m.main-none-eabihf` | `embassy-nrf` | stable |
| STM32 (M4) | STM32WB55, L4 | `thumbv7em-none-eabihf` | `embassy-stm32` | stable |
| STM32 (M33) | STM32U5, WBA | `thumbv8m.main-none-eabihf` | `embassy-stm32` | stable |
| ESP32 RISC-V | ESP32-C6, H2 | `riscv32imac-unknown-none-elf` | `esp-hal` | stable |
| ESP32 RISC-V | ESP32-C3 | `riscv32imc-unknown-none-elf` | `esp-hal` | stable |
| ESP32 Xtensa | ESP32-S3 | `xtensa-esp32s3-none-elf` | `esp-hal` | **esp-rs fork** (`espup`) |
| Host (tests) | — | native | mock/sim | stable |

Xtensa requires a compiler fork — the only entry in the matrix that breaks "one
`rustup`, one CI". That is why we target the **RISC-V** ESP32 variants
(C6/C3/H2) by default; the S3 only when a product needs its horsepower or PSRAM.

### 4.3 The common denominator

Portability does not come from discipline, but from the fact that all three
families share a common set of abstractions:

| Shared layer | Role | What changes per platform |
|---|---|---|
| `embassy-executor` | async model, tasks | nothing |
| `embassy-time` | timers, `Timer::after` | only the time driver in the BSP |
| `embedded-hal` 1.0 / `-async` | SPI, I²C, GPIO | the implementation in the vendor HAL |
| `embedded-io-async` | UART, TCP, channel to the RCP/NCP | the implementation in the vendor HAL |
| `embedded-storage-async` | flash | geometry and driver |
| `critical-section` | critical sections | the impl is supplied by the BSP |
| `defmt` + RTT | logs | nothing (apart from the transport) |

The price of that choice: we are tied to the embassy ecosystem. An MCU without
embassy/`embedded-hal` support drops out of the matrix or requires writing a HAL
— that is a silicon selection criterion, not an implementation detail.

### 4.4 The port contract

Porting to a new platform = supplying an implementation of the set below.
Nothing beyond it; if more is needed, the contract is incomplete.

```rust
/// Everything pkpu-device-core requires from the silicon.
/// Implemented in pkpu-platform-*, assembled in boards/.
pub trait Platform {
    type Rng:      CryptoRng;              // hardware TRNG
    type Flash:    NorFlash;               // storage partition (embedded-storage-async)
    type Clock:    Clock;                  // monotonic + RTC wall-clock + wake-up
    type Identity: IdentityStore;          // read device_id/key, sign (SE or OTP)
    type Reset:    ResetController;        // reboot, reset reason, watchdog
    type Ota:      OtaSlots;               // A/B slot addresses, mark_boot_ok, rollback
    type Power:    PowerControl;           // entering low-power mode, rails
}
```

Port checklist:

| Step | Outcome |
|---|---|
| 1. `pkpu-platform-<x>`: implement the `Platform` traits | the core links |
| 2. `memory.x` / partitions + bootloader integration | the image boots |
| 3. `embassy-time` driver (usually ready in the vendor HAL) | timers work |
| 4. `Link` for the radio available on that platform | the device talks to the cloud |
| 5. a job in the CI matrix and one board on the HIL rig | the port does not rot |

### 4.5 Where portability really hurts

Differences that **cannot** be hidden behind a trait — they have to be designed
deliberately:

| Area | nRF | STM32 | ESP32 |
|---|---|---|---|
| Flash | internal, uniform pages | internal, uneven sectors, dual-bank in U5/H7 | **external QSPI** + partition table, writes require cache handling |
| A/B bootloader | `embassy-boot-nrf` | `embassy-boot-stm32` | ESP's own bootloader / MCUboot — **a different image format** |
| Crypto / key | CryptoCell, KMU | RNG + PKA (some families) | RNG, HMAC/DS peripheral, eFuse + flash encryption |
| Wi-Fi | none (co-processor) | none (co-processor) | native (`esp-wifi`) |
| 802.15.4 / BLE | native | WB/WBA | C6/H2 |
| Deep sleep | single-digit µA | single-digit µA (Stop2/Standby) | markedly higher — sometimes disqualifying for `SLEEP` |

Current-draw figures are treated as an order of magnitude to be confirmed by
measurement (see section 8) — not as design inputs.

Two conclusions binding on the rest of the system:

1. **The OTA manifest must carry `platform`**, not just `model` and `hw_rev` —
   images are not interchangeable between families. See section 9.
2. **The energy budget is not portable.** The same code on a different MCU means
   a different battery life. Choosing the platform for a `SLEEP` product is a
   hardware decision, not a cosmetic one.

### 4.6 Verifying portability

Portability that is declared and untested falls apart in the first sprint.
That is why it is enforced mechanically:

- every PR builds `pkpu-device-core` and a sample `app` **on all** targets from
  matrix 4.2 — a build failure on any of them blocks the merge,
- `cargo test` of the core on the host (in-memory `Link`, simulated time) — this
  is the main safety net, see section 10,
- CI lint: `cfg(target_arch|target_os)` outside `pkpu-platform-*` and `boards/`
  is an error,
- an image size report per target with a regression threshold — portability must
  not mean flash bloat on the smallest platform,
- nightly HIL: at least one board from **every** family in the matrix.

The minimal sensible scope to start with: two families (ESP32-C6 for Wi-Fi,
nRF52840 for 802.15.4/BLE). A third platform added as the third one, rather than
anticipated up front, is the proper test of whether the abstraction is real —
exactly as with `Link` (see ADR-010, ADR-011).

---

## 5. Device state machine

```
        power-on
           |
           v
     [SELF_TEST] --fail--> [FAULT] --(watchdog)--> reset
           |
           v
    creds in flash? --no--> [PROVISIONING] --ok--> write creds
           | yes                 ^                      |
           v                     |                      |
      [CONNECTING] --timeout/backoff--                   |
           |  ok                 ^                      |
           v                     |                      |
      [OPERATIONAL] <------------+----------------------+
        |   |   |
        |   |   +--> [OTA]        (download, verification, reboot)
        |   +------> [LOW_POWER]  (only SLEEP_TYPE=SLEEP)
        +----------> [FACTORY_RESET] --> wipe creds, keep identity
```

- Backoff in `CONNECTING`: exponential with jitter, capped at 15 min. Without
  jitter the fleet creates a thundering herd after a cloud outage.
- `FAULT` does not mean a reset loop: after N failed boots the bootloader rolls
  back to the previous slot.
- A factory reset **erases** the network credentials and the owner assignment,
  and **keeps** the `DeviceIdentity`.

---

## 6. Provisioning

### 6.1 BLE sequence

```
1. A device without creds -> advertises `PKPU-PROV`,
   payload: model, serial (shortened), nonce.
2. The mobile app -> connect, GATT service 0xPKPU:
     char PROV_INFO   (read)   identity + capabilities
     char PROV_CHAL   (read)   challenge from the device
     char PROV_RESP   (write)  the cloud's response (signature) + creds
     char PROV_STATE  (notify) progress / error
3. The app passes the challenge to the cloud.
   The cloud checks device_id against the factory registry and signs a claim token.
4. The app writes into PROV_RESP: {claim_token, network_creds, mqtt_endpoint}.
   The device verifies the cloud's signature -> only then does it accept the creds.
5. Device -> CONNECTING -> first report -> the cloud closes the claim.
```

The key point: **the device verifies the cloud, not only the cloud the device.**
Without step 4, any phone in range can inject arbitrary credentials.

### 6.2 NFC sequence

- An NTAG tag with an I²C interface to the MCU (not a plain passive tag) — this
  allows bidirectional exchange without powering the main radio.
- The NDEF contains: `device_id`, `serial`, `model`, a deep-link URL to the app.
- The phone reads the tag -> opens the app -> from there the path is as in BLE
  from step 3, except that the creds reach the MCU through the tag's memory.
- The "out-of-box" variant: the tag is written at the factory, for identification
  only; the credential transfer still goes over BLE.

### 6.3 PAN network provisioning

| COM_TYPE | What reaches the device |
|---|---|
| `WIFI` | SSID + PSK (or WPA3 SAE), the MQTT endpoint, the cloud CA |
| `OT_THREAD` | Thread Operational Dataset (network key, PAN ID, channel) |
| `ZIGBEE` | pairing mode (Install Code preferred over a well-known key) |
| `BLE` | none — the connectivity is GATT itself |

---

## 7. Persistent state and buffering

- `sequential-storage` on a dedicated flash partition, with the keys:
  `identity`, `net_creds`, `shadow_reported`, `ota_state`, `tel_buffer`.
- Telemetry buffer: a ring buffer of `postcard` entries with a timestamp.
  On loss of connectivity it is written to flash; on recovery it is sent as a
  batch with the `backfill = true` flag so the cloud does not interpret it as
  live data.
- Write budget: NOR flash lasts ~100k cycles. We do not write telemetry to flash
  while it fits in the RAM buffer — only on overflow.
- Clock: RTC + time synchronization on every connection. A device without
  synchronized time sends `uptime_ms` instead of a timestamp and the cloud
  reconstructs the time — we never send a made-up wall clock.

---

## 8. Energy budget (SLEEP_TYPE = SLEEP)

A requirement recorded in the product profile and verified by measurement:

```
target: CR2032 220 mAh -> 24 months
daily budget: 220 mAh / 730 days = ~0.30 mAh/day = ~12.5 uA on average
split: sleep 4 uA | measurement 2 uA | radio TX/RX 5 uA | reserve 1.5 uA
```

Consequences for the firmware:
- no busy-waiting, everything on `embassy` timers and interrupts,
- the radio is switched on only for the TX window plus the expected reply,
- aggregation: N measurements -> 1 transmission,
- event-driven transmission (change > threshold) instead of a fixed interval,
  with a heartbeat once per `max_silence`.

---

## 9. OTA and the bootloader

- Flash layout: `bootloader | slot A | slot B | storage | identity(OTP)`.
- The manifest is ed25519-signed and contains: `model`, `platform`,
  `hw_rev_min/max`, `version`, `size`, `sha256`, `min_battery_pct`,
  `requires_reboot`. `platform` is mandatory and verified before writing to the
  slot — the core is portable, the binary image is not (see section 4.5).
- Signature verification happens **in the bootloader**, with the public key in a
  protected region.
- Rollback: a boot attempt counter; the application must call `mark_boot_ok()`
  after a successful connection to the cloud, otherwise the previous slot is
  restored.
- `SLEEP` devices: OTA only when `battery > min_battery_pct` and within the
  service window; the transfer is resumable (block-wise, with an offset).
- Delta OTA — optional, only once image size becomes a real problem.

---

## 10. Testability

| Level | How |
|---|---|
| Unit | `pkpu-device-core` compiled for the host, in-memory `Link`, simulated time |
| Portability | build of the whole target matrix (section 4.2) on every PR, image size report |
| Integration | QEMU / `embassy` sim + a fake MQTT broker |
| HIL | probe-rs + `defmt` over RTT, a board **from every MCU family** on the rig, current measurement (Otii/PPK2) |
| Fleet | staging tenant, a 1% "canary" cohort before every rollout |

`defmt` instead of `log` — compact logs, decoded on the host side, they eat
neither flash nor time.

The full strategy — mandatory scenarios, the conformance suite for platforms,
energy tests as regression tests and merge-blocking gates — is in
[TESTING.md](TESTING.md).

---

## 11. Product profile

Every product (`apps/<name>/product.toml`) declares:

```toml
model      = "PKPU-TH-01"
hw_rev     = "B"
platform   = "nrf52840"        # silicon choice; the SDK core is identical for stm32/esp32
board      = "th01-rev-b"
sleep_type = "SLEEP"
com_type   = "OT_THREAD"
prov_type  = ["BLE", "NFC"]

[matter]
mode        = "bridged"        # none | bridged | dual | native
device_type = "0x0302"         # type from the Matter library, if applicable
vid         = "0xFFF1"         # test value until one is assigned by the CSA
pid         = "0x8001"

[telemetry]
interval_s       = 600
max_silence_s    = 3600
buffer_entries   = 4096

[power]
battery         = "CR2032"
target_months   = 24
avg_current_ua  = 12.5

[ota]
min_battery_pct = 30
window          = "02:00-05:00"
```

This file is the source of truth for the firmware **and** for the record in the
cloud registry — the `device_type` entry is generated from it. See
[DATA.md](DATA.md).
