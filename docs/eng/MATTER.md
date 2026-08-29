# MATTER — ecosystem compatibility

Matter is the only decision in this documentation that changes the **application
layer of the device**, not just the transport. That is why it gets its own
document rather than a paragraph in [DEVICE.md](DEVICE.md).

**Baseline decision (ADR-012):** Matter is a **compatibility surface**, not the
internal model of the platform. The default route is a **bridge** on the gateway;
native mode only for products in which the ecosystem is a value in itself.

---

## 1. What Matter gives and what it does not

| Area | Matter handles it | What stays on our side |
|---|---|---|
| Local control from ecosystems | yes — Apple, Google, Amazon, SmartThings without a per-ecosystem integration | — |
| Onboarding through someone else's app | yes — QR / pairing code, one flow | our provisioning for features outside the Matter model |
| Local security | yes — device attestation, operational certificates, CASE sessions | identity towards **our** cloud |
| Device type interoperability | yes — for types from the Matter library | everything the library does not have |
| **Historical telemetry** | **no** — the model is stateful, not measurement-oriented | all of it: buffers, backfill, retention, aggregates |
| **Fleet management** | **no** | registry, OTA campaigns, cohorts, presence, alerts |
| **Multi-tenancy, RBAC, audit** | **no** | all of it ([CLOUD.md](CLOUD.md)) |
| The manufacturer's cloud | does not replace it | still needed |

The conclusion that shapes the rest of this document: **Matter is not an
alternative to this platform — it is an interface to it.** A product without the
manufacturer's cloud loses telemetry, history, rules and wave OTA. A product
without Matter loses ecosystem integration. These are disjoint values, and that
is why both entities have to coexist.

---

## 2. The clash of data models

| Our model ([PROTOCOL.md](PROTOCOL.md)) | Matter | Compatibility |
|---|---|---|
| `DeviceId` (UUIDv7, ours) | Node ID (per fabric) + VID/PID | different namespaces — a mapping, not a replacement |
| `Shadow.desired` / `reported` | cluster attributes + subscriptions | conceptually close, but Matter has no "desired" — an attribute write is immediate |
| `Telemetry { channel, value }` | an attribute of a typed cluster (e.g. `MeasuredValue`) | our channels are arbitrary, clusters are not |
| `Command` with `command_id`, TTL, dedup | a cluster command, with no TTL and no idempotency by definition | our semantics are richer; with a bridge they have to be closed off |
| Events | Events | convergent |
| Measurement history | **none** | exclusively on our side |

The practical design consequence: **when defining a measurement channel we first
check whether a corresponding Matter cluster exists, and we adopt its unit and
scale.** Then the bridge is a table lookup. If we invent our own scale for
temperature, the bridge becomes the place for conversions, rounding and bugs —
and that kind of code always reaches production last and least tested.

---

## 3. Three integration models

### A. Bridge — **the default**

The gateway exposes our devices as Bridged Devices within a single certified
bridge node. The end devices stay native (our protocol, our radio, our
provisioning).

- (+) **one certification instead of certifying every SKU**,
- (+) battery devices pay for Matter neither in flash nor in current,
- (+) it works for Zigbee, which cannot enter Matter any other way,
- (+) our architecture stays unchanged,
- (−) it requires a gateway — so it does not work for `WIFI` direct-to-cloud
  products,
- (−) the device is "seen" by the ecosystem only while the bridge is alive,
- (−) ecosystems treat bridged devices as second-class citizens (limited types,
  some features missing).

### B. Dual-stack — for selected SKUs

The device speaks Matter (locally, to the ecosystems) **and** our protocol (to
the cloud: telemetry, OTA, fleet).

- (+) a full-fledged Matter device without losing telemetry and fleet management,
- (−) two stacks in one image: flash, RAM, two radio schedules,
- (−) two onboarding flows that users confuse — it requires carefully thought-out
  UX,
- (−) certification per SKU, recertification on changes,
- (−) a real energy cost — for `SLEEP` it has to be calculated, not assumed.

### C. Native Matter — only when the ecosystem **is** the product

The device speaks Matter only; the manufacturer's cloud (if any) reaches it
through a controller.

- (+) the simplest consumer product, the lowest barrier of entry for the user,
- (−) we lose historical telemetry, wave campaigns, presence and multi-tenancy —
  that is, everything this platform brings,
- (−) our SDK shrinks to a thin layer over someone else's stack.

**Recommendation:** A as the default, B for consumer products with real demand
for the ecosystem, C only for a single SKU where the platform was not needed
anyway.

---

## 4. Impact on the taxonomy

Matter **is not** a `COM_TYPE`. It is an application layer above IPv6 (Thread,
Wi-Fi, Ethernet), and BLE serves in it only for commissioning. Adding `MATTER` to
`ComType` would be a category error and would corrupt an enum whose values are
eternal ([PROTOCOL.md](PROTOCOL.md) 2).

Instead — **a separate, orthogonal dimension**:

```rust
#[repr(u8)]
pub enum MatterMode {
    None   = 0,   // a device outside the Matter world
    Bridged = 1,  // exposed by a bridge on the gateway
    Dual   = 2,   // its own Matter stack + our protocol
    Native = 3,   // Matter only
}
```

In `product.toml`:

```toml
[matter]
mode        = "bridged"        # none | bridged | dual | native
device_type = "0x0302"         # type from the Matter library, if applicable
vid         = "0xFFF1"         # test value until one is assigned by the CSA
pid         = "0x8001"
```

Device classification becomes the product of five dimensions:
`SLEEP_TYPE × COM_TYPE × PROV_TYPE × MATTER_MODE × STATE`.

---

## 5. Two identities on one device

This is where Matter touches our security model most sharply
([ARCHITECTURE.md](ARCHITECTURE.md) 7, ADR-007).

| | Our identity | Matter identity |
|---|---|---|
| Key | ed25519 generated on the chip | DAC — the device key + certificate |
| Chain | our factory CA → operational CA | DAC → PAI → PAA, an entry in the DCL |
| Who issues it | us | us, but within a chain recognized by the CSA |
| What for | authentication towards our cloud | attestation when commissioning into a fabric |
| Life cycle | the whole life of the device | the whole life of the device, plus a NOC per fabric (replaceable) |

Coexistence is possible and necessary, but **the production line has to write two
sets of credentials** — ours and Matter's (DAC + CD). That is a real cost of the
provisioning station, requiring an own PAA/PAI and a DAC issuance procedure, so
the decision about Matter is taken **before** production starts, not after.

The `Bridged` variant removes that complication: only the bridge has a DAC.

---

## 6. Commissioning and multiple administrators

Matter brings an onboarding flow we do not control: a user can pair a device from
a third-party app and never open ours.

Consequences to design for:

- **A device may be in an ecosystem fabric and not be claimed with us.**
  The registry has to allow that state instead of treating it as an error.
- **Multi-admin is built into Matter** — the user is entitled to add another
  ecosystem. Our cloud is not the only administrator and must not assume it is.
- **A device being removed (decommissioned) from a third-party app** has to be
  detected, not show up as a silent loss of functionality.
- The Matter pairing code (QR / NFC) and our provisioning code
  ([DEVICE.md](DEVICE.md) 6) have to be **one** print on the enclosure. Two
  different QR codes on one device is a guaranteed user error and an avalanche of
  support tickets.

---

## 7. Battery devices: ICD versus our `SLEEP`

Matter has its own answer for sleeping devices — **ICD** (Intermittently
Connected Devices), with short- and long-interval variants. The mapping:

| Ours | Matter | Note |
|---|---|---|
| `SLEEP_TYPE = NONSLEEP` | an always-reachable device | convergent |
| `SLEEP_TYPE = SLEEP` | ICD (SIT / LIT) | the ICD parameters have to follow from `expected_interval` |
| the `SLEEPS` state | the ICD reachability window | the same idea: no response ≠ a failure |
| command queueing at the parent | the ICD mechanics | convergent |

The good news: Matter solves this problem the same way we do, so `SLEEPS` from
ADR-009 is not in conflict. The bad news: the ICD parameters are **negotiated
with the fabric** and may drift away from our energy budget. If an ecosystem
forces more frequent windows than `product.toml` assumes, the declared battery
life stops holding — and that has to be caught by a test, not by a complaint.

---

## 8. Two OTA paths, one source of truth

In `Dual` and `Native` mode there is a second update route: the OTA
Requestor / Provider cluster, with its own image header format.

Rules to stop this exploding:

1. **One artifact, one source of the version.** The image is produced once in our
   pipeline ([CI.md](CI.md) 7) and wrapped in the Matter format — we never build
   two images of the same version.
2. **The Matter `SoftwareVersion` is derived from our version** deterministically
   (Matter requires a monotonic number, we use semver — the mapping has to be a
   function, not a spreadsheet).
3. **The wave campaigns stay ours.** Matter OTA has no cohorts, no gates and no
   fleet rollback. The ecosystem route is a backup channel, not the primary one.
4. **A/B rollback stays in our bootloader** — regardless of which route the image
   arrived by.

---

## 9. Stack and platforms

A hard fact: **there is no production-certified Matter stack in Rust.** The
reference SDK (`connectedhomeip`) is C++, and silicon support comes through
vendor SDKs (nRF Connect SDK, `esp-matter`, ST). The Rust `rs-matter` exists and
is worth watching, but it is not a base for certifying a product — today.

This is exactly the same situation as Thread and Zigbee (ADR-004), so the answer
is the same and consistent:

| Mode | Implementation |
|---|---|
| `Bridged` | the Matter stack **on the gateway only** (Linux, the C++ SDK as a process alongside our binary) — the devices stay clean |
| `Dual` / `Native` | the vendor stack in the MCU image + our logic, the boundary through FFI, or Matter on a co-processor |

The impact on ADR-001 ("Rust across the whole stack"): Matter is the **third**
breach after Thread and Zigbee. Three breaches in one place are a signal that the
rule in reality reads: *Rust everywhere there is no certified third-party
radio/application stack in the way*. Better to write it down honestly in that
form than to pretend the exceptions are incidental.

The impact on portability (ADR-011): a bridge does not touch the SDK core at all.
`Dual` mode touches it heavily — the availability and maturity of the Matter SDK
becomes **another criterion for choosing silicon**, alongside embassy support.

---

## 10. Certification — the real cost

| Element | What it means |
|---|---|
| CSA membership | an annual fee; the tier decides access to the specification and to a VID |
| VID / PID | the manufacturer identifier from the CSA; until it is assigned we work with test values |
| PAA / PAI | our own attestation chain + a DCL entry, or using the silicon vendor's chain |
| Product certification | tests in an authorized laboratory, per device type |
| Recertification | on significant changes and when moving to new specification versions |
| Maintenance | the Matter specification releases new versions regularly — compliance is a process, not a one-off event |

That is why the bridge is the default: **one certified artifact instead of N**,
while keeping the whole product range visible in the ecosystems.

---

## 11. Tests and CI

Additions to [TESTING.md](TESTING.md) and [CI.md](CI.md) if Matter comes in:

- **A bridge conformance suite**: for every supported device type, the mapping
  our channel ↔ cluster attribute is tested in both directions, units and scale
  included. This is where a conversion bug shows up as "the thermometer reads
  21°C in our app and 69.8 in Apple Home".
- **A multi-administrator test**: pairing into two fabrics at once, removal from
  one, the other preserved.
- **A state divergence test**: a change from the ecosystem versus a change from
  our cloud at the same instant — the resolution has to be defined, not
  accidental.
- **An ICD versus energy budget test**: current measured with the parameters
  imposed by the fabric, not with our defaults.
- **The CSA Test Harness** in the nightly pipeline — certification must not be
  the first moment we run the official tests.
- **DAC artifacts** handled like production keys: HSM, no access from a laptop,
  an audit of issuances ([CI.md](CI.md) 9).

---

## 12. What we do now and what we defer

Matter compatibility today does not require a single line of Matter code — it
requires **not cutting off the route**:

| Now (near-zero cost) | Deferred |
|---|---|
| Measurement channels defined against Matter cluster units and scales | the bridge implementation |
| `MatterMode` in the taxonomy and in `product.toml` | the Matter stack in the firmware |
| The registry allows a device paired into a third-party fabric and not claimed with us | multi-admin in the UI |
| One field on the enclosure for the pairing code | printing the Matter codes |
| The VID/PAA decision **before** production starts | certification |
| A `SoftwareVersion` derivable from our version | Matter OTA |

Staging relative to [ARCHITECTURE.md](ARCHITECTURE.md) 9: the bridge arrives
together with `pkpu-gateway` (stage 5), because only then does a node exist that
can carry it. `Dual` mode — no earlier than after stage 6 and only with a
concrete product in hand.

The thing that **must not** be deferred: if Matter is ever to appear, the
decisions about the VID, the PAA and what the production line writes are made
before the first batch. A device without a DAC cannot be retrofitted remotely.
