# 2. CLOUD — the cloud stack

All services in Rust, `tokio` + `tracing`, one `cloud/` workspace.
Every service is a separate binary, but they share the `pkpu-proto` and
`pkpu-cloud-common` crates (configuration, telemetry, connection pools, errors).

---

## 1. Services

| Service | Responsibility | Scaling |
|---|---|---|
| `pkpu-ingest` | MQTT broker + mTLS termination + frame validation + publication onto NATS | horizontal, sticky per device |
| `pkpu-registry` | device registry, device shadow, presence, command reconcile | horizontal, stateless |
| `pkpu-provisioning` | factory registry, attestation, certificate issuance, claiming | low QPS, 2 instances |
| `pkpu-ota` | artifacts, manifests, campaigns, wave rollout | low QPS |
| `pkpu-rules` | rule evaluation, alerts, webhooks, integrations | horizontal per tenant partition |
| `pkpu-api` | REST + WebSocket for web and mobile, OIDC authorization | horizontal, stateless |
| `pkpu-gateway` | edge binary (Linux), PAN <-> MQTT bridge, Matter bridge | 1 per installation |

We deliberately do **not** start with finely split microservices — `registry`,
`provisioning` and `ota` can initially be a single binary with three modules.
The split is designed now and deployed when it starts to hurt.

---

## 2. `pkpu-ingest` — the data gateway

An MQTT 5 broker embedded as a library (`rumqttd`) inside the ingest process,
not a separate broker. The gain: validation and authorization happen without a
network hop, and we have full control over the connect/publish hooks.

```
TLS accept
  -> extract CN/SAN from the client certificate = device_id
  -> lookup in the registry (Valkey cache, TTL 60 s): active? not revoked?
  -> ACL: the topic MUST start with dev/{device_id}/
  -> postcard decoding -> Envelope
  -> validation: v, seq (gaps -> metric), ts (drift -> correction)
  -> publication onto NATS JetStream
  -> presence update in Valkey (SETEX device:{id}:seen)
```

Backpressure: when JetStream cannot keep up, ingest stops ACK-ing QoS 1.
Devices buffer locally (see DEVICE.md section 7). **We never drop data
silently** — either an ACK, or the device retransmits.

### NATS subjects

```
tel.{tenant}.{model}.{device_id}      telemetry
evt.{tenant}.{model}.{device_id}      events
state.{tenant}.{device_id}            reported state
ack.{tenant}.{device_id}              command acknowledgements
cmd.{tenant}.{device_id}              commands (cloud -> ingest -> device)
presence.{tenant}.{device_id}         ONLINE/DISCONNECTED/SLEEPS transitions
```

JetStream streams with 7-day retention — this allows a sink to be rebuilt after
a database failure without data loss.

---

## 3. `pkpu-registry` — registry and shadow

- The source of truth about which devices exist, who owns them and what state
  they are in.
- Maintains the `Shadow` (see PROTOCOL.md section 6) in Postgres, cached in
  Valkey.
- **Presence watchdog**: a periodic task that promotes devices from `ONLINE` to
  `DISCONNECTED` or `SLEEPS` based on `last_seen`, `sleep_type` and
  `expected_interval`:

```rust
let deadline = last_seen + expected_interval * TOLERANCE;   // TOLERANCE = 3
let state = if now < deadline           { Online }
            else if sleep_type == Sleep { Sleeps }
            else                        { Disconnected };
```

- **Command reconcile**: a command has the states `pending -> sent -> acked |
  expired | rejected`. For `SLEEP` devices the command waits in the queue until
  the next `Hello`.

---

## 4. `pkpu-provisioning`

Three registries, deliberately kept separate:

| Registry | Contains | Who writes |
|---|---|---|
| **factory** | `device_id`, `serial`, `pubkey`, `model`, `hw_rev`, production date | the production line via `pkpu-cli` (mTLS, a separate CA) |
| **operational** | assignment to `tenant` / `site` / owner | the claiming process |
| **revocation** | blocked devices (theft, RMA, key compromise) | the operator |

The claiming flow — see DEVICE.md section 6.1. On the cloud side:

```
POST /provisioning/challenge  { device_id, nonce_device }
   -> check the factory registry and the revocation registry
   -> check that the device is not already claimed by another tenant
   -> return { claim_token, sig_cloud }   (ed25519, valid for 5 min)

POST /provisioning/complete   { device_id, proof }   // the device's signature
   -> verify the signature with the public key from the factory registry
   -> issue a client certificate (operational CA, 2-year validity, auto-renew)
   -> entry in the operational registry + audit_log
```

**Two CAs**: the factory one (offline, key in an HSM) and the operational one
(online, rotatable). Compromising the operational CA does not invalidate the
hardware identity.

---

## 5. `pkpu-ota`

- Artifacts in S3/MinIO, addressed by hash: `fw/{model}/{version}/{sha256}.bin`.
- The manifest is signed in CI with a key from the HSM — **never** by hand on a
  laptop.
- A campaign:

```rust
struct Campaign {
    id:          CampaignId,
    cohort:      CohortFilter,   // model, hw_rev, from_version, site, tag
    target:      FirmwareVersion,
    waves:       Vec<Wave>,      // [1%, 10%, 50%, 100%]
    gate:        Gate,           // max_failure_pct, min_soak_minutes
    window:      Option<TimeWindow>,
    state:       CampaignState,  // draft|running|paused|done|rolled_back
}
```

- The gate between waves: if the share of devices in the cohort that did not
  report `mark_boot_ok` within `soak` exceeds the threshold, the campaign moves
  to `paused` automatically.
- Fleet rollback: a new campaign with the previous version, not "undoing" the
  old one.

---

## 6. `pkpu-rules`

Rules per tenant, stored as data (not code):

```
WHEN  tel.channel(TEMP) > 30  FOR 5m
AND   device.site = "hall-1"
THEN  notify(email, webhook) , command(device, "set_output", {relay: 1})
```

- Engine: a NATS consumer with an in-memory time window plus state in Valkey.
- Version 1: threshold and time-based rules cover 90% of cases.
  A DSL/scripting (rhai, WASM) only when genuinely needed.
- Every rule firing -> an entry in `events` + optionally a command through the
  registry (never straight onto the broker — commands always go through the
  shadow so they end up in the audit trail).

---

## 7. `pkpu-api`

- `axum`, OpenAPI generated from the types (`utoipa`), versioning via the `/v1/`
  path.
- Authorization: OIDC Bearer JWT. The `tenant_id` claim + roles.
- WebSocket `/v1/stream` — subscription to live telemetry and state changes,
  filtered by `site` / `device_id`, backed by NATS.
- Rate limiting with `tower-governor` per token.
- Row Level Security in Postgres: the connection sets `SET LOCAL app.tenant_id`
  and the RLS policies filter. A bug in the API code cannot leak another
  tenant's data.

Endpoint outline:

```
GET    /v1/devices?site=&state=&model=
GET    /v1/devices/{id}
PATCH  /v1/devices/{id}/desired        // write shadow.desired
POST   /v1/devices/{id}/commands
GET    /v1/devices/{id}/telemetry?ch=&from=&to=&agg=
GET    /v1/sites  /v1/sites/{id}/devices
POST   /v1/provisioning/challenge  |  /complete
GET    /v1/ota/campaigns  |  POST /v1/ota/campaigns
GET    /v1/events?severity=&from=
WS     /v1/stream
```

---

## 8. `pkpu-gateway` (edge)

A binary for Linux (RPi CM4 / any aarch64), with the role:

1. Maintains the PAN stack: an OpenThread Border Router (Thread) or a Zigbee
   coordinator — both as C processes/libraries, driven from Rust over
   Spinel/EZSP.
2. Maps `short_id` <-> `device_id` and attaches identity to frames.
3. Holds a single mTLS session to the cloud for the whole subnet.
4. **Store-and-forward**: without internet it buffers to disk (sled/redb) and
   later sends the data with `backfill = true`.
5. Local fallback rules — a minimal subset of `pkpu-rules`, so the installation
   keeps working without the cloud.
6. It is itself a device in the registry: it has a `device_id`, a state and OTA.
7. **Matter bridge** (ADR-012): exposes the subnet's devices as Bridged Devices
   to ecosystem fabrics. One certified node instead of certifying every SKU —
   see [MATTER.md](MATTER.md). The Matter stack is a process alongside our
   binary here, not inside it.

---

## 9. Deployment

- **Eventually**: Kubernetes (k3s is enough), Helm/Kustomize, distroless images.
- **To start**: docker-compose on a single VPS — Postgres+Timescale, NATS,
  Valkey, MinIO and all the Rust binaries. Migration to k8s without code
  changes.
- Database migrations: `sqlx migrate`, run as a job before the deploy.
- Configuration: environment variables + a TOML file, validated at startup
  (`figment` + `serde`), fail-fast on bad configuration.
- CI: `cargo clippy -- -D warnings`, `cargo test`, `cargo deny check`, image
  builds, integration tests on `testcontainers` — the full pipeline and release
  rules are in [CI.md](CI.md), the test scope in [TESTING.md](TESTING.md).
- Migrations must be backward compatible only (expand/contract): during a
  rollout the old and the new binary work against the same schema.

---

## 10. Observability and SLOs

| Metric | Target |
|---|---|
| ingest p99 latency (from MQTT publish to NATS ack) | < 50 ms |
| command delivery to a `NONSLEEP` device | < 1 s p95 |
| API availability | 99.9 % |
| telemetry loss | 0 (buffering + retransmission) |
| database rebuild time from JetStream | < 30 min |

- `tracing` + OpenTelemetry, with `trace_id` propagated from the API request to
  the command and back to the `CommandAck`.
- Fleet metrics as a first-class dashboard: state distribution
  (`ONLINE`/`DISCONNECTED`/`SLEEPS`), firmware version distribution, RSSI/LQI,
  battery level, OTA failure rate.
- Alerts on fleet anomalies (a sudden rise in `DISCONNECTED` within one `site`
  = a gateway or internet failure, not a device failure).
