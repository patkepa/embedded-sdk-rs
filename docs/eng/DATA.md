# DATA — data model and database

**PostgreSQL 16 + TimescaleDB** as the only operational database: registry,
shadow, telemetry, events and audit in one place. Access through `sqlx`
(queries verified at compile time).

The rationale for one database instead of Postgres + a separate TSDB: up to
~10⁹ measurement points and ~10⁵ devices, Timescale copes without trouble, and
joins between telemetry and device metadata are the rule in this system, not the
exception. Splitting them adds eventual consistency where we do not need it.

---

## 1. Entity diagram

```
tenants ──< sites ──< devices >── device_types ──< device_type_channels
                        │  │
                        │  ├──< device_shadow      (1:1, desired/reported)
                        │  ├──< commands
                        │  ├──< device_events
                        │  └──< telemetry          (hypertable)
                        │
factory_devices ────────┘   (factory registry, before claiming)

firmware_versions ──< ota_campaigns ──< ota_device_status
users >── user_tenant_roles ── tenants
audit_log
```

---

## 2. Enum types

Generated from `pkpu-proto` (see PROTOCOL.md section 7) — not written by hand.

```sql
CREATE TYPE sleep_type   AS ENUM ('NONSLEEP','SLEEP');
CREATE TYPE com_type     AS ENUM ('WIFI','OT_THREAD','ZIGBEE','BLE');
CREATE TYPE prov_type    AS ENUM ('BLE','NFC');
CREATE TYPE device_state AS ENUM ('ONLINE','DISCONNECTED','SLEEPS');
CREATE TYPE platform     AS ENUM ('NRF52','NRF53','STM32','ESP32_RISCV','ESP32_XTENSA');
CREATE TYPE matter_mode  AS ENUM ('NONE','BRIDGED','DUAL','NATIVE');
CREATE TYPE command_state AS ENUM ('PENDING','SENT','ACKED','REJECTED','EXPIRED');
CREATE TYPE campaign_state AS ENUM ('DRAFT','RUNNING','PAUSED','DONE','ROLLED_BACK');
```

---

## 3. Registry

```sql
CREATE TABLE tenants (
    tenant_id   uuid PRIMARY KEY,
    name        text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE sites (
    site_id     uuid PRIMARY KEY,
    tenant_id   uuid NOT NULL REFERENCES tenants,
    name        text NOT NULL,
    timezone    text NOT NULL DEFAULT 'Europe/Warsaw',
    location    geography(Point,4326)          -- optional, PostGIS
);

-- product type; generated from apps/<product>/product.toml
CREATE TABLE device_types (
    model            text PRIMARY KEY,          -- 'PKPU-TH-01'
    hw_rev           text NOT NULL,
    platform         platform NOT NULL,         -- MCU family, decides the OTA artifact
    matter_mode      matter_mode NOT NULL DEFAULT 'NONE',
    matter_vid       int,                       -- NULL until assigned by the CSA
    matter_pid       int,
    sleep_type       sleep_type NOT NULL,
    com_type         com_type   NOT NULL,
    prov_types       prov_type[] NOT NULL,
    expected_interval_s int NOT NULL,           -- basis for the presence watchdog
    max_silence_s       int NOT NULL,
    spec             jsonb NOT NULL             -- the rest of product.toml
);

-- measurement channels: ChannelId -> name, unit, scale
CREATE TABLE device_type_channels (
    model      text REFERENCES device_types,
    channel_id int  NOT NULL,
    name       text NOT NULL,        -- 'temperature'
    unit       text NOT NULL,        -- 'degC'
    scale      real NOT NULL DEFAULT 1.0,
    kind       text NOT NULL,        -- gauge | counter | state
    PRIMARY KEY (model, channel_id)
);

-- factory registry: exists before the device has an owner
CREATE TABLE factory_devices (
    device_id    uuid PRIMARY KEY,
    serial       text UNIQUE NOT NULL,
    model        text NOT NULL REFERENCES device_types,
    pubkey       bytea NOT NULL,
    produced_at  timestamptz NOT NULL,
    batch        text,
    revoked_at   timestamptz,
    revoke_reason text
);

CREATE TABLE devices (
    device_id        uuid PRIMARY KEY REFERENCES factory_devices,
    tenant_id        uuid NOT NULL REFERENCES tenants,
    site_id          uuid REFERENCES sites,
    short_id         int  NOT NULL,
    model            text NOT NULL REFERENCES device_types,
    label            text,
    fw_version       text,
    state            device_state NOT NULL DEFAULT 'DISCONNECTED',
    last_seen        timestamptz,
    battery_pct      smallint,
    rssi_dbm         smallint,
    gateway_id       uuid REFERENCES devices(device_id),   -- NULL for WIFI
    claimed_at       timestamptz NOT NULL DEFAULT now(),
    tags             text[] NOT NULL DEFAULT '{}'
);

CREATE INDEX ON devices (tenant_id, state);
CREATE INDEX ON devices (tenant_id, site_id);
CREATE INDEX ON devices (model, fw_version);      -- OTA cohorts
CREATE UNIQUE INDEX ON devices (tenant_id, short_id);
```

`gateway_id` points at another device (a gateway is in `devices` too) — thanks
to which a gateway failure shows up as a correlation rather than as 200
independent alerts.

---

## 4. Device shadow and commands

```sql
CREATE TABLE device_shadow (
    device_id  uuid PRIMARY KEY REFERENCES devices,
    desired    jsonb NOT NULL DEFAULT '{}',
    reported   jsonb NOT NULL DEFAULT '{}',
    version    bigint NOT NULL DEFAULT 0,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE commands (
    command_id  uuid PRIMARY KEY,               -- UUIDv7, idempotency key
    device_id   uuid NOT NULL REFERENCES devices,
    tenant_id   uuid NOT NULL,
    op          text NOT NULL,
    args        jsonb NOT NULL,
    state       command_state NOT NULL DEFAULT 'PENDING',
    issued_by   uuid,                           -- user, or NULL for a rule
    issued_at   timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    acked_at    timestamptz,
    result      jsonb
);

CREATE INDEX ON commands (device_id, state) WHERE state IN ('PENDING','SENT');
```

The command queue for `SLEEP` devices is simply rows in the `PENDING` state —
no separate queueing system. After `Hello` the registry fetches them with a
single query.

---

## 5. Telemetry — hypertable

```sql
CREATE TABLE telemetry (
    ts         timestamptz NOT NULL,
    device_id  uuid        NOT NULL,
    tenant_id  uuid        NOT NULL,
    channel_id int         NOT NULL,
    val        double precision,
    val_bool   boolean,
    backfill   boolean NOT NULL DEFAULT false
);

SELECT create_hypertable('telemetry', 'ts', chunk_time_interval => INTERVAL '1 day');
SELECT add_dimension('telemetry', 'device_id', number_partitions => 8);

CREATE INDEX ON telemetry (device_id, channel_id, ts DESC);
ALTER TABLE telemetry SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'device_id, channel_id',
    timescaledb.compress_orderby   = 'ts DESC'
);
SELECT add_compression_policy('telemetry', INTERVAL '7 days');
```

**A narrow (long) model, not a wide one**: one row = one measurement of one
channel. Adding a new sensor does not require an `ALTER TABLE` migration.
Timescale compression brings the overhead of the narrow model down to ~1–2 bytes
per point.

### Writes

`pkpu-ingest` -> NATS -> a batching sink. A batch every 200 ms or every 5000
rows, `COPY BINARY`. Individual per-row `INSERT`s are the most common cause of
IoT systems falling over — we do not do that.

### Continuous aggregates

```sql
CREATE MATERIALIZED VIEW telemetry_5m
WITH (timescaledb.continuous) AS
SELECT time_bucket('5 minutes', ts) AS bucket,
       device_id, channel_id,
       avg(val) AS avg, min(val) AS min, max(val) AS max, count(*) AS n
FROM telemetry GROUP BY bucket, device_id, channel_id;

-- telemetry_1h, telemetry_1d analogously
```

The API picks the source automatically based on the time range:
`< 24 h` -> `telemetry`; `< 30 days` -> `telemetry_5m`; `< 1 year` ->
`telemetry_1h`; beyond that -> `telemetry_1d`.

### Retention

| Data | Period |
|---|---|
| `telemetry` (raw) | 90 days |
| `telemetry_5m` | 13 months |
| `telemetry_1h` | 3 years |
| `telemetry_1d` | indefinitely |
| `device_events` | 2 years |
| `audit_log` | 7 years |

The periods are configurable per tenant (pricing plan) — a Timescale policy with
a parameter, not a hard-coded one.

---

## 6. Events and audit

```sql
CREATE TABLE device_events (
    ts         timestamptz NOT NULL,
    device_id  uuid NOT NULL,
    tenant_id  uuid NOT NULL,
    kind       text NOT NULL,      -- 'alarm', 'boot', 'ota_done', 'rule_fired'
    severity   smallint NOT NULL,  -- 0 info .. 3 critical
    payload    jsonb NOT NULL DEFAULT '{}'
);
SELECT create_hypertable('device_events', 'ts');

CREATE TABLE audit_log (
    id         bigserial PRIMARY KEY,
    ts         timestamptz NOT NULL DEFAULT now(),
    tenant_id  uuid,
    actor      text NOT NULL,      -- user:uuid | rule:uuid | system
    action     text NOT NULL,      -- 'device.claim', 'shadow.desired.set', ...
    target     text NOT NULL,
    before     jsonb,
    after      jsonb,
    trace_id   text
);
```

`audit_log` is append-only: `REVOKE UPDATE, DELETE` for the application role.

---

## 7. OTA

```sql
CREATE TABLE firmware_versions (
    model       text NOT NULL REFERENCES device_types,
    version     text NOT NULL,               -- semver
    hw_rev_min  text, hw_rev_max text,
    sha256      bytea NOT NULL,
    size_bytes  int   NOT NULL,
    s3_key      text  NOT NULL,
    manifest    jsonb NOT NULL,              -- signed
    released_at timestamptz,
    PRIMARY KEY (model, version)
);

-- The image is built for a specific platform. The firmware core is portable
-- across MCU families, the binary artifact is not — the release is rejected
-- if `manifest->>'platform'` does not match `device_types.platform`.

CREATE TABLE ota_campaigns (
    campaign_id uuid PRIMARY KEY,
    tenant_id   uuid REFERENCES tenants,     -- NULL = global
    model       text NOT NULL,
    target_ver  text NOT NULL,
    cohort      jsonb NOT NULL,
    waves       jsonb NOT NULL,
    state       campaign_state NOT NULL DEFAULT 'DRAFT',
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE ota_device_status (
    campaign_id uuid REFERENCES ota_campaigns,
    device_id   uuid REFERENCES devices,
    wave        smallint NOT NULL,
    status      text NOT NULL,   -- queued|downloading|verifying|rebooting|ok|failed
    from_ver    text, to_ver text,
    error       text,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (campaign_id, device_id)
);
```

---

## 8. Multi-tenancy through RLS

```sql
ALTER TABLE devices ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON devices
    USING (tenant_id = current_setting('app.tenant_id')::uuid);
```

Every connection from the pool sets `SET LOCAL app.tenant_id` at the start of the
transaction. System services (`ingest`, global `ota`) use a separate role with
`BYPASSRLS` — deliberately and narrowly.

This is a **second-level** safeguard. The application layer filters anyway; RLS
exists so that a bug in the application layer does not end in a leak.

---

## 9. Reference queries

```sql
-- the latest measurement of every channel of a device
SELECT DISTINCT ON (channel_id) channel_id, ts, val
FROM telemetry WHERE device_id = $1 AND ts > now() - interval '7 days'
ORDER BY channel_id, ts DESC;

-- fleet state overview
SELECT state, count(*) FROM devices WHERE tenant_id = $1 GROUP BY state;

-- OTA cohort
SELECT device_id FROM devices
WHERE model = $1 AND fw_version = $2 AND state <> 'DISCONNECTED'
  AND (battery_pct IS NULL OR battery_pct >= $3);

-- devices "silent" beyond tolerance
SELECT d.device_id FROM devices d JOIN device_types t USING (model)
WHERE d.last_seen < now() - (t.max_silence_s * interval '1 second');
```

---

## 10. Backup and recovery

- WAL archiving + `pgBackRest`, PITR with 30-day retention.
- A restore test **once a month**, automated, on a separate instance.
  A backup that has never been restored does not exist.
- JetStream (7-day retention) as a second line: it allows telemetry to be
  replayed from the last consistent snapshot.
- The factory registry (`factory_devices`) — a separate, additional offline
  backup. Losing it means being unable to authenticate the devices that have
  been manufactured.
