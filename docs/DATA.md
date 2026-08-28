# DATA — model danych i baza

**PostgreSQL 16 + TimescaleDB** jako jedyna baza operacyjna: rejestr, shadow,
telemetria, zdarzenia i audyt w jednym miejscu. Dostęp przez `sqlx`
(zapytania weryfikowane przy kompilacji).

Uzasadnienie jednej bazy zamiast Postgres + osobna TSDB: do ~10⁹ punktów
pomiarowych i ~10⁵ urządzeń Timescale radzi sobie bez problemu, a joiny między
telemetrią a metadanymi urządzenia są w tym systemie regułą, nie wyjątkiem.
Rozdzielenie dodaje spójność eventualną tam, gdzie jej nie potrzebujemy.

---

## 1. Diagram encji

```
tenants ──< sites ──< devices >── device_types ──< device_type_channels
                        │  │
                        │  ├──< device_shadow      (1:1, desired/reported)
                        │  ├──< commands
                        │  ├──< device_events
                        │  └──< telemetry          (hypertable)
                        │
factory_devices ────────┘   (rejestr fabryczny, przed claimowaniem)

firmware_versions ──< ota_campaigns ──< ota_device_status
users >── user_tenant_roles ── tenants
audit_log
```

---

## 2. Typy enum

Generowane z `pkpu-proto` (patrz PROTOCOL.md sekcja 7) — nie pisane ręcznie.

```sql
CREATE TYPE sleep_type   AS ENUM ('NONSLEEP','SLEEP');
CREATE TYPE com_type     AS ENUM ('WIFI','OT_THREAD','ZIGBEE','BLE');
CREATE TYPE prov_type    AS ENUM ('BLE','NFC');
CREATE TYPE device_state AS ENUM ('ONLINE','DISCONNECTED','SLEEPS');
CREATE TYPE command_state AS ENUM ('PENDING','SENT','ACKED','REJECTED','EXPIRED');
CREATE TYPE campaign_state AS ENUM ('DRAFT','RUNNING','PAUSED','DONE','ROLLED_BACK');
```

---

## 3. Rejestr

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
    location    geography(Point,4326)          -- opcjonalnie, PostGIS
);

-- typ produktu; generowany z apps/<produkt>/product.toml
CREATE TABLE device_types (
    model            text PRIMARY KEY,          -- 'PKPU-TH-01'
    hw_rev           text NOT NULL,
    sleep_type       sleep_type NOT NULL,
    com_type         com_type   NOT NULL,
    prov_types       prov_type[] NOT NULL,
    expected_interval_s int NOT NULL,           -- baza dla watchdoga presence
    max_silence_s       int NOT NULL,
    spec             jsonb NOT NULL             -- reszta product.toml
);

-- kanały pomiarowe: ChannelId -> nazwa, jednostka, skala
CREATE TABLE device_type_channels (
    model      text REFERENCES device_types,
    channel_id int  NOT NULL,
    name       text NOT NULL,        -- 'temperature'
    unit       text NOT NULL,        -- 'degC'
    scale      real NOT NULL DEFAULT 1.0,
    kind       text NOT NULL,        -- gauge | counter | state
    PRIMARY KEY (model, channel_id)
);

-- rejestr fabryczny: istnieje zanim urządzenie ma właściciela
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
    gateway_id       uuid REFERENCES devices(device_id),   -- NULL dla WIFI
    claimed_at       timestamptz NOT NULL DEFAULT now(),
    tags             text[] NOT NULL DEFAULT '{}'
);

CREATE INDEX ON devices (tenant_id, state);
CREATE INDEX ON devices (tenant_id, site_id);
CREATE INDEX ON devices (model, fw_version);      -- kohorty OTA
CREATE UNIQUE INDEX ON devices (tenant_id, short_id);
```

`gateway_id` wskazuje na inne urządzenie (gateway też jest w `devices`) — dzięki
temu awaria gatewaya jest widoczna jako korelacja, a nie 200 niezależnych alertów.

---

## 4. Device shadow i komendy

```sql
CREATE TABLE device_shadow (
    device_id  uuid PRIMARY KEY REFERENCES devices,
    desired    jsonb NOT NULL DEFAULT '{}',
    reported   jsonb NOT NULL DEFAULT '{}',
    version    bigint NOT NULL DEFAULT 0,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE commands (
    command_id  uuid PRIMARY KEY,               -- UUIDv7, klucz idempotencji
    device_id   uuid NOT NULL REFERENCES devices,
    tenant_id   uuid NOT NULL,
    op          text NOT NULL,
    args        jsonb NOT NULL,
    state       command_state NOT NULL DEFAULT 'PENDING',
    issued_by   uuid,                           -- user lub NULL dla reguły
    issued_at   timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    acked_at    timestamptz,
    result      jsonb
);

CREATE INDEX ON commands (device_id, state) WHERE state IN ('PENDING','SENT');
```

Kolejka komend dla urządzeń `SLEEP` to po prostu wiersze w stanie `PENDING` —
bez osobnego systemu kolejkowego. Po `Hello` registry pobiera je jednym zapytaniem.

---

## 5. Telemetria — hypertable

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

**Model wąski (long), nie szeroki**: jeden wiersz = jeden pomiar jednego kanału.
Dodanie nowego czujnika nie wymaga migracji `ALTER TABLE`. Kompresja Timescale
sprowadza narzut wąskiego modelu do ~1–2 bajtów na punkt.

### Zapis

`pkpu-ingest` -> NATS -> sink batchujący. Wsad co 200 ms lub 5000 wierszy,
`COPY BINARY`. Pojedyncze `INSERT`-y na wiersz to najczęstsza przyczyna
niewydolności systemów IoT — nie robimy tego.

### Agregaty ciągłe

```sql
CREATE MATERIALIZED VIEW telemetry_5m
WITH (timescaledb.continuous) AS
SELECT time_bucket('5 minutes', ts) AS bucket,
       device_id, channel_id,
       avg(val) AS avg, min(val) AS min, max(val) AS max, count(*) AS n
FROM telemetry GROUP BY bucket, device_id, channel_id;

-- analogicznie telemetry_1h, telemetry_1d
```

API wybiera źródło automatycznie na podstawie zakresu czasu:
`< 24 h` -> `telemetry`; `< 30 dni` -> `telemetry_5m`; `< 1 rok` -> `telemetry_1h`;
dalej -> `telemetry_1d`.

### Retencja

| Dane | Okres |
|---|---|
| `telemetry` (raw) | 90 dni |
| `telemetry_5m` | 13 miesięcy |
| `telemetry_1h` | 3 lata |
| `telemetry_1d` | bezterminowo |
| `device_events` | 2 lata |
| `audit_log` | 7 lat |

Okresy konfigurowalne per tenant (plan taryfowy) — polityka Timescale
z parametrem, nie sztywna.

---

## 6. Zdarzenia i audyt

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

`audit_log` jest append-only: `REVOKE UPDATE, DELETE` dla roli aplikacji.

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
    manifest    jsonb NOT NULL,              -- podpisany
    released_at timestamptz,
    PRIMARY KEY (model, version)
);

CREATE TABLE ota_campaigns (
    campaign_id uuid PRIMARY KEY,
    tenant_id   uuid REFERENCES tenants,     -- NULL = globalna
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

## 8. Multi-tenancy przez RLS

```sql
ALTER TABLE devices ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON devices
    USING (tenant_id = current_setting('app.tenant_id')::uuid);
```

Każde połączenie z puli ustawia `SET LOCAL app.tenant_id` na początku transakcji.
Serwisy systemowe (`ingest`, `ota` globalne) używają osobnej roli z `BYPASSRLS` —
świadomie i w wąskim zakresie.

To jest zabezpieczenie **drugiego poziomu**. Warstwa aplikacji i tak filtruje;
RLS istnieje po to, żeby błąd w warstwie aplikacji nie kończył się wyciekiem.

---

## 9. Zapytania wzorcowe

```sql
-- ostatni pomiar każdego kanału urządzenia
SELECT DISTINCT ON (channel_id) channel_id, ts, val
FROM telemetry WHERE device_id = $1 AND ts > now() - interval '7 days'
ORDER BY channel_id, ts DESC;

-- przegląd stanu floty
SELECT state, count(*) FROM devices WHERE tenant_id = $1 GROUP BY state;

-- kohorta OTA
SELECT device_id FROM devices
WHERE model = $1 AND fw_version = $2 AND state <> 'DISCONNECTED'
  AND (battery_pct IS NULL OR battery_pct >= $3);

-- urządzenia „ciche" ponad tolerancję
SELECT d.device_id FROM devices d JOIN device_types t USING (model)
WHERE d.last_seen < now() - (t.max_silence_s * interval '1 second');
```

---

## 10. Backup i odtwarzanie

- WAL archiving + `pgBackRest`, PITR z retencją 30 dni.
- Test odtworzenia **raz w miesiącu**, automatyczny, na osobnej instancji.
  Backup, którego nie odtworzono, nie istnieje.
- JetStream (retencja 7 dni) jako druga linia: pozwala odegrać telemetrię
  od momentu ostatniego spójnego snapshotu.
- Rejestr fabryczny (`factory_devices`) — osobny, dodatkowy backup offline.
  Jego utrata oznacza brak możliwości uwierzytelnienia wyprodukowanych urządzeń.
