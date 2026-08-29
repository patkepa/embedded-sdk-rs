# SDK — creation and development procedures

How the shared part of the firmware comes into being and how it changes. This
document is about the **process and the API rules**, not the architecture — that
is described in [DEVICE.md](DEVICE.md). Tests: [TESTING.md](TESTING.md).
Releases: [CI.md](CI.md).

---

## 1. What is SDK and what is product

| Layer | Role | Who changes it |
|---|---|---|
| `pkpu-proto` | the contract across process boundaries | changes require the crate owner's approval (CODEOWNERS) |
| `pkpu-hal` | hardware traits, the port contract | a change = a contract change for all platforms |
| `pkpu-device-core` | state machine, scheduler, OTA, storage | the SDK core, portable |
| `pkpu-link` | radio abstraction + implementations | the SDK core, portable |
| `pkpu-platform-*`, `boards/` | the implementation for the silicon | the platform owner |
| `apps/<product>` | the **product**, not the SDK | the product team |

The dividing line: **the SDK does not know what product is being built; the
product does not know what it runs on.** If a product name shows up in the SDK,
or an MCU vendor name in `apps/`, one side is leaking and that is a defect to
fix, not a detail.

---

## 2. The life cycle of a change in the SDK

```
1. NEED              raised by a product ("apps/ cannot write this
                     without a cfg / without duplication / without reaching into the HAL")
2. QUALIFICATION     is it a contract change?   -> yes: an ADR (DECISIONS.md)
                     does it touch pkpu-proto?  -> yes: wire versioning (section 5)
                     does it touch pkpu-hal?    -> yes: it touches ALL platforms
3. API DESIGN        trait / type / signature + rationale in the PR description
                     mandatory: what it looks like from the perspective of apps/
4. IMPLEMENTATION    core + pkpu-platform-mock  (host)
5. TESTS             a scenario in TESTING.md 4 + a conformance extension,
                     if the change touches the platform contract
6. PORTS             an implementation on every platform in the matrix
                     -> conformance on HIL must pass on EVERY one
7. DOCUMENTATION     rustdoc + an example + a CHANGELOG entry + a docs/ update
8. RELEASE           tag, semver per section 5
```

Step 6 is where the temptation shows up most often: "for now I'll implement it
only on nRF, the rest later". The result is always the same — `pkpu-hal` stops
being a contract and becomes a description of one platform. Hence: **a trait
entering `pkpu-hal` must have an implementation on every platform in the matrix,
or it does not enter.** If something physically does not exist on some silicon,
that is expressed by a type (`Option`, a separate optional trait,
`type Unsupported`) and not by a missing implementation.

---

## 3. API design rules

Rules that follow from the code shipping to a device you cannot visit, and
having to be portable across three MCU families (ADR-011):

| Rule | Reason |
|---|---|
| `no_std`, `alloc` optional and off by default in the core | the smallest platform in the matrix sets the budget |
| No panics on the runtime path; an error = a `Result` with a `defmt::Format` type | a panic on the device = a reset, and a reset in the field = data loss |
| `unwrap()`/`expect()` allowed only in BSP initialization | there a failure means non-working hardware anyway |
| Minimal traits; one responsibility per trait | every added element has to be implemented N times |
| Generics, not `dyn`, on hot paths | image size and no allocation |
| Async-first (`embassy`), zero blocking loops | the energy budget (DEVICE 8) |
| No allocation and no `format!` on the telemetry path | fragmentation and current spikes |
| **Cargo `features` additive, never mutually exclusive** | cargo unifies features across the graph — two mutually exclusive features are a link error for the consumer, not for the author |
| `#[non_exhaustive]` on public enums and configuration structs | adding a variant does not break consumers |
| Public API with `#![deny(missing_docs)]` | an undocumented API will be used anyway — just wrongly |
| MSRV and toolchain versions pinned in the repo | ESP32 Xtensa has its own fork; version drift breaks the matrix |

Separately: **nothing in the SDK measures system time directly.** Time goes
through `Clock` from `pkpu-hal`, because otherwise there is no way to test on the
host with simulated time ([TESTING.md](TESTING.md) 4) — and that is the
foundation of the whole test strategy.

---

## 4. Three procedures

### 4.1 Adding a platform (a new MCU family)

The contract and the checklist: [DEVICE.md](DEVICE.md) 4.4. Acceptance criteria:

1. `pkpu-platform-<x>` implements the full `Platform` — with no `todo!()`,
2. the conformance suite passes **on hardware**, not only on the mock,
3. a board sits on the HIL rig and the job is in the nightly pipeline,
4. the build target is added to the PR matrix,
5. a current measurement establishes the energy threshold for that platform,
6. an entry in [DEVICE.md](DEVICE.md) 4.2 and 4.5 (known limitations — honestly).

Without points 2–3 this is not platform support, only a declaration. A platform
without a board on the HIL rig gets removed from the matrix at the first failure
nobody can reproduce.

### 4.2 Adding a radio technology

1. An implementation of `Link` + an honest `LinkProfile` (MTU, RTT,
   push-capable, energy cost) — the core tunes itself to those numbers, so a lie
   in the profile shows up as "strange" telemetry behaviour,
2. `conformance::link` passes, including agreement between the declared profile
   and the measurement,
3. a link degradation scenario in the host tests ([TESTING.md](TESTING.md) 4),
4. the PAN network provisioning path described in [DEVICE.md](DEVICE.md) 6.3,
5. the mapping onto the path to the cloud: direct or through a gateway
   ([ARCHITECTURE.md](ARCHITECTURE.md) 3).

### 4.3 Adding a product

1. `apps/<product>/product.toml` — `model`, `platform`, `board`, the taxonomy,
   the budgets ([DEVICE.md](DEVICE.md) 11),
2. measurement channels defined in `pkpu-proto` and the generated
   `device_types` + `device_type_channels` entry ([DATA.md](DATA.md)),
3. a BSP in `boards/` if the board is new,
4. E2E on the simulator before the first hardware unit,
5. an energy test against the threshold from `product.toml`,
6. a firmware release pipeline for that model ([CI.md](CI.md) 7).

A product does **not** add code to `pkpu-device-core`. If it has to, that means
an extension point is missing in the SDK and the right change is the cycle from
section 2.

---

## 5. Versioning and compatibility

Three independent axes, deliberately kept apart:

| Axis | Versioning | Whoever breaks it, pays |
|---|---|---|
| SDK crates | semver, `cargo-semver-checks` in CI | the consumer = our own `apps/` — a controlled cost |
| **The wire protocol** | the `v` field in `Envelope`, separate from semver | the consumer = devices in the field — an uncontrolled cost |
| The database schema | expand/contract migrations ([CI.md](CI.md) 6) | the consumer = a running cloud |

The rules for the protocol are the strictest, because a device in the field
lives for years and **cannot be updated at the same instant as the cloud**:

- fields are **added**, never removed and never given a new meaning,
- enum values are appended at the end; an existing value is eternal
  ([PROTOCOL.md](PROTOCOL.md) 2),
- **the cloud supports every wire version that is in the field** — until the last
  device with a given version is retired from the registry, we do not remove
  support for it,
- **firmware never requires a newer cloud**: a new device has to work with an
  older backend version for the duration of the rollout,
- a breaking change requires a `v` bump, compatibility vectors and an ADR. It is
  not forbidden — it is expensive and it is meant to be visible.

Deprecating an SDK API: mark it `#[deprecated]` naming the replacement → at
least one full release cycle of coexistence → removal at the major bump. For the
protocol the window is counted in years of hardware life, not in releases.

---

## 6. Definition of Done for a change in the SDK

The list checked in review — a missing item is grounds for not merging:

- [ ] host tests cover the new scenario (not just the happy path),
- [ ] conformance extended, if the platform contract changes,
- [ ] an implementation on **all** platforms in the matrix,
- [ ] the impact on image size in the CI report, justified if > threshold,
- [ ] the impact on current draw measured, if the change touches the radio,
      sleep or flash,
- [ ] rustdoc on the public API + a usage example from the perspective of `apps/`,
- [ ] wire compatibility intact, or a `v` bump + vectors + an ADR,
- [ ] CHANGELOG,
- [ ] the relevant document in `docs/` updated — documentation that drifts from
      the code is worse than none, because it gets quoted.

---

## 7. Roles and reviews

| Area | Decision owner | What needs approval |
|---|---|---|
| `pkpu-proto` | the contract owner | every change to fields, enums, wire versions |
| `pkpu-hal` | the SDK owner | adding/changing a trait — it touches all platforms |
| `pkpu-platform-*` | the owner of that platform | local changes without the SDK owner's approval |
| `apps/` | the product team | freely, within the bounds of the SDK API |
| Firmware release, signing | a person with release rights | approval in a protected environment ([CI.md](CI.md) 9) |

With a one- or two-person team the roles converge on a single person — the
mechanism then is not review but the **ADR**: the decision written down at the
moment it is made. In six months nobody will reconstruct why a trait looks the
way it does, and it will need reconstructing at the first port.

---

## 8. What the SDK deliberately does not do

- **It does not hide differences that cannot be hidden**: the energy budget, the
  image format, the flash map ([DEVICE.md](DEVICE.md) 4.5). Pretending they are
  portable is worse than exposing them openly.
- **It does not wrap the vendor HAL wholesale.** A trait comes into being when
  the core needs it — not "in advance", because we anticipate a future use.
- **It contains no product logic.** An alarm threshold, sensor calibration, a
  channel name — that is `apps/`.
- **It does not support platforms without a board on the HIL rig.** Support that
  nobody measures is support on paper only.
