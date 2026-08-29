# CI/CD — pipelines and releases

What runs, when and with what; how the artifact that reaches a device and a
server comes into being. The scope of the tests is described in
[TESTING.md](TESTING.md), the SDK development rules — [SDK.md](SDK.md).

---

## 1. Principles

1. **Nothing is built by hand.** An artifact that reaches production or a device
   comes exclusively from the pipeline. A build from a laptop has no right to be
   signed.
2. **The pipeline lives in the repo and is versioned together with the code.** A
   pipeline change goes through review like code.
3. **A fast signal before the full one.** A PR gets a result in ~10 min; the
   heavy things (HIL, soak, fuzzing) run overnight, not in the developer's loop.
4. **Reproducibility.** `--locked`, pinned toolchain versions
   (`rust-toolchain.toml`), actions pinned by SHA, images by digest. Building the
   same tag a year from now has to yield the same bytes — otherwise a field
   incident cannot be investigated.
5. **Short-lived secrets.** OIDC to the KMS/registry instead of long-lived
   tokens. The firmware signing key never leaves the HSM — CI sends a hash and
   gets back a signature.
6. **`main` always green.** A merge queue: a PR is tested against the merge
   result, not against its own stale base.

---

## 2. Runner topology

| Runner | For what | Notes |
|---|---|---|
| linux x86_64 (cloud) | builds, host tests, cross-builds, containers | the backbone; scalable, replaceable |
| linux aarch64 | cloud images on ARM, `pkpu-gateway` builds | optional, when the edge target is aarch64 |
| **HIL (self-hosted)** | tests on boards, current measurement | a machine with probe-rs, boards and a PPK2; the only "stateful" runner |
| macOS | XCFramework and the iOS bindings smoke test | the mobile pipeline only |

The HIL runner is a shared resource and a bottleneck — it queues jobs rather than
parallelizing them. That is why HIL is not on the PR path (see section 4).

---

## 3. Splitting into pipelines

A monorepo with three workspaces ([ARCHITECTURE.md](ARCHITECTURE.md) 5) →
pipelines triggered by **paths**, so that a change in the cloud does not build
six embedded targets:

| Pipeline | Triggered by a change in |
|---|---|
| `proto` | `proto/**` — and it **forces** `firmware`, `cloud` and `mobile` to run |
| `firmware` | `firmware/**`, `proto/**` |
| `cloud` | `cloud/**`, `proto/**`, migrations |
| `mobile` | `mobile/**`, `proto/**` |
| `e2e` | any of the above, after a merge into `main` |

A change in `proto/` touches everything by definition — that is the price of
"one contract" and it is meant to be visible in CI time rather than discovered
after a deployment.

---

## 4. The PR pipeline

The order is arranged so that the cheapest gates fail first.

```
[lint]        fmt --check | clippy -D warnings | typos | cargo-machete
                  |
[contract]    proto tests | golden vectors | enum agreement with SQL
              cargo-semver-checks (public SDK API) | OpenAPI/JSON Schema snapshot
                  |
        +---------+-----------------------------+------------------+
        |                                       |                  |
[firmware]                               [cloud]              [mobile]
  host test (core + mock platform)         unit test            core test on host
  conformance on the mock                  testcontainers:      bindings build
  target matrix build (DEVICE 4.2)           PG+Timescale,      (iOS/Android smoke
  image size report + threshold               NATS, MinIO        on main only)
  energy test (simulated profile)          RLS isolation tests
                                           migration up + backward test
        +---------+-----------------------------+------------------+
                  |
[security]        cargo-deny (licenses, CVEs, sources) | dependency audit
                  |
[merge queue]     rebuild against the merge result -> merge
```

Time budget: **10 min** for a single workspace path, 20 min when the change
touches `proto/`. Exceeding the budget is treated as a regression — the job gets
split or the cache gets fixed (`sccache`, registry and `target/` caches).

What is **not** in the PR and why: HIL (one runner, a queue), fuzzing (time),
soak (time), load tests (cost). Instead — section 5.

---

## 5. `main` and the nightly pipeline

After a merge:

- E2E on the simulator ([TESTING.md](TESTING.md) 9): provisioning → telemetry →
  command → OTA → rollback, on compose with the full cloud stack,
- container image builds and publication under the tag = git SHA,
- automatic deployment to **staging** + a smoke E2E against staging.

Overnight (`main`, scheduled):

| Job | Time | Outcome |
|---|---|---|
| HIL: conformance + OTA + rollback on every MCU family | ~40 min | a report per platform |
| Current measurement on the reference boards | ~30 min | the energy trend per commit |
| Fuzzing of the `Envelope` decoder | 30 min, persistent corpus | a crash → an automatic issue + a regression test |
| Virtual fleet soak (10k devices, 8 h) | overnight | memory leaks, `seq` drift, presence stability |
| Ingest load tests | ~20 min | p99 vs the SLO from [CLOUD.md](CLOUD.md) 10 |
| `cargo deny` + audit against a fresh CVE database | ~2 min | new vulnerabilities in dependencies |

A red nightly job blocks a **release**, not a merge. The distinction is
deliberate: nightly tests catch classes of bugs that cannot be attributed to a
single PR, but they must not halt day-to-day work.

---

## 6. Cloud release

```
tag cloud-vX.Y.Z
  -> distroless image build (--locked), the digest recorded in the release notes
  -> SBOM (CycloneDX) + a provenance attestation of the artifact
  -> deploy to staging -> E2E -> a 1 h soak
  -> human approval -> deploy to production (rolling)
  -> migrations as a separate job BEFORE deploying the binaries
```

Hard rules:

- **Backward-compatible migrations only** (expand → migrate → contract). During
  the rollout the old and the new binary work against the same schema at the
  same time; `DROP COLUMN` is a separate release, after the old version has been
  retired.
- **Rollback = the previous tag**, not "reversing" a migration. A reversible
  migration is the exception, not the rule — hence the rule above.
- The configuration is validated at startup (fail-fast), so a bad configuration
  stops one pod rather than the fleet.

---

## 7. Firmware release

The most sensitive pipeline in the system: its output ships to hardware that
cannot be visited.

```
tag fw-<model>-vX.Y.Z
  -> build --locked for the platforms declared in product.toml
  -> HIL: conformance + OTA + forced rollback + current measurement on the target board
  -> manifest: {model, platform, hw_rev_min/max, version, size, sha256,
                min_battery_pct, requires_reboot, sbom_digest}
  -> ed25519 signature with a key from the HSM (protected environment, requires approval)
  -> artifact upload: fw/{model}/{version}/{sha256}.bin
  -> an entry in firmware_versions (DATA.md 7)
  -> [END]  a release != a rollout
```

- **A release is not a rollout.** Producing a signed artifact does not send it to
  any device. An OTA campaign is a separate, deliberate operator decision with
  wave gates ([CLOUD.md](CLOUD.md) 5).
- **Signing happens in a protected environment** with manual approval.
  Compromising a developer account must not result in signed firmware.
- `platform` in the manifest is verified at release time and again by the
  bootloader — the core is portable, the image is not
  ([DEVICE.md](DEVICE.md) 4.5).
- Artifacts, manifests and SBOMs are kept for the whole product life cycle plus
  a margin — without them an incident cannot be investigated and the reporting
  obligation cannot be met (section 9).

---

## 8. Mobile core release

```
tag core-vX.Y.Z -> build pkpu-core -> UniFFI -> XCFramework (macOS runner) + AAR
                -> publication to the artifact registry, semver versioning
                -> the iOS/Android shells consume a version, not a local path
```

A shell never builds the core from source on an app developer's machine —
otherwise "works on my machine" comes back in the worst possible place, namely
during provisioning at the customer.

---

## 9. Pipeline security and compliance

| Requirement | Implementation |
|---|---|
| No long-lived secrets | OIDC to the cloud/KMS/registry; static secrets only where there is no alternative |
| Signing key | HSM/KMS, the signing operation is remote, the key never leaves the module |
| Dependency integrity | `Cargo.lock` in the repo, `cargo deny` (licenses, CVEs, sources), actions pinned by SHA, no `curl \| sh` in steps |
| Artifact provenance | a provenance attestation + an SBOM (CycloneDX) for every firmware and cloud release |
| Separation of roles | firmware signing and production deployment behind an approval gate; the author of a change does not approve their own release |
| Evidence retention | artifacts, manifests, SBOMs and pipeline logs for the product life cycle |

The regulatory context (CRA / EN 18031, [DECISIONS.md](DECISIONS.md), open
questions 4): the obligation of secure updates, an SBOM and vulnerability
reporting means that the above is not hygiene "for later" — it is a product
requirement. Bolting it on afterwards is more expensive than doing it right away.

---

## 10. Repo conventions

- **Conventional commits** → a generated CHANGELOG per workspace.
- **Prefixed tags**: `proto-v*`, `fw-<model>-v*`, `cloud-v*`, `core-v*`.
  One monorepo, four independent release cycles.
- **Branch protection** on `main`: required review, green PR gates, a merge
  queue, linear history.
- **CODEOWNERS**: `proto/` has an owner — a contract change requires their
  approval, because by design it breaks the build of every consumer
  ([ARCHITECTURE.md](ARCHITECTURE.md), the one-contract principle).
- A change in `proto/` without a CHANGELOG entry and without a compatibility
  vector does not pass review — this is not a formality, it is the only trace
  from which, two years later, someone will reconstruct why a device on firmware
  1.3 speaks differently from one on 1.9.

---

## 11. The order of building CI itself

We do not build the whole thing from day one — the order follows the stages from
[ARCHITECTURE.md](ARCHITECTURE.md) 9:

| Project stage | What we add to CI |
|---|---|
| 0 (contract) | lint, proto tests, golden vectors, `cargo deny` |
| 1 (E2E telemetry) | testcontainers for the cloud, a firmware build on one target, images + staging |
| 2 (commands) | E2E on the simulator, a merge queue |
| 3 (provisioning) | the mobile pipeline, a bindings smoke test |
| 4 (OTA) | **the firmware release pipeline with HSM signing** + HIL (OTA/rollback) |
| 5 (second platform) | the full target matrix, conformance on HIL, energy tests |
| 6 (product) | soak, load tests, game days, evidence retention |

The firmware release pipeline has to exist **before** the first device leaves the
desk. Signing "temporarily by hand" is the kind of temporary arrangement that
stays for years and that later cannot be told apart from a compromise.
