# Local telemetry stack

Run the entire development stack from the workspace:

```sh
cargo xtask telemetry
```

Requires Rust and running Docker with Compose v2. First startup downloads images
and builds the Rust collector. The command validates every registered contract,
generates dashboards, and returns when Mosquitto, the collector's MQTT
subscriptions, and Grafana are healthy. Services keep running in the background.

Open [Grafana](http://localhost:3000), then **Dashboards → Embedded SDK**.
Anonymous viewing is enabled; the local admin login is `admin` / `admin`.
The SQLite datasource and a dashboard per contract are already provisioned.
No SQL, datasource setup, or manual dashboard import is needed.

```sh
cargo xtask telemetry demo     # start the stack and publish each contract's example
cargo xtask telemetry status
cargo xtask telemetry logs     # Ctrl-C exits logs; services continue
cargo xtask telemetry down     # stop services; keep data
cargo xtask telemetry prepare  # validate/generate only, without Docker
```

## Connect the reference firmware

The XIAO default firmware already publishes a heartbeat every 30 seconds.
Find the backend computer's LAN address and configure it at flash time:

```sh
WIFI_SSID='your-network' WIFI_PASSWORD='your-passphrase' \
MQTT_HOST='192.168.1.20' MQTT_PORT='1883' \
MQTT_CLIENT_ID='xiao-desk-01' MQTT_PLAINTEXT_FIXTURE='1' \
  cargo xtask run xiao-esp32c6
```

Use a unique client ID for each device. `localhost` on a device does not refer
to your computer. The computer and device must be mutually reachable, with
TCP 1883 allowed through the computer's firewall. The firmware reconnects when
the backend starts, so flashing first is fine. Samples from before a connection
exists are not historical uploads: the current firmware has no persistent queue.

The registered contract is
[`firmware/seeed/xiao-esp32c6/telemetry.json`](../firmware/seeed/xiao-esp32c6/telemetry.json).
It accepts `embedded-sdk/test/{device}/telemetry` with
`{"version":1,"kind":"heartbeat"}`. Grafana shows message counts, last seen,
recent payloads, and validation failures. This fixture contains no sensor values.
The Beetle Wi-Fi scanner publishes through a USB serial-to-MQTT bridge; see the
[scanner guide](connectivity/wifi-scanner.md). The beacon variants still output
only over serial.

## Give another firmware its own contract

Add a JSON file beside its `Cargo.toml` and register the relative path:

```toml
[package.metadata.embedded-sdk.firmware]
board = "your-board"
variant = "your-variant"
telemetry-contract = "telemetry.json"
```

For example, a sensor firmware could own this contract:

```json
{
  "id": "room-sensor-v1",
  "title": "Room sensor",
  "version": 1,
  "topic": "embedded-sdk/room-sensor/v1/{device}/telemetry",
  "equals": { "/kind": "reading" },
  "metrics": [
    {
      "key": "temperature",
      "title": "Temperature",
      "pointer": "/temperature_c",
      "unit": "celsius"
    },
    {
      "key": "rssi",
      "title": "Wi-Fi signal",
      "pointer": "/rssi_dbm",
      "unit": "dBm",
      "required": false
    }
  ],
  "example": { "version": 1, "kind": "reading", "temperature_c": 23.5, "rssi_dbm": -54 }
}
```

Make the firmware publish matching JSON to that topic, then rerun
`cargo xtask telemetry`. It discovers contracts through Cargo metadata, validates
their examples, updates subscriptions, and provisions new metric panels.
Contracts are owned by firmware types, while `{device}` identifies each physical
instance. Devices appear in the dashboard selector after their first accepted
message; refresh the dashboard/time range to refresh the selector.

- One `{device}` placeholder must occupy an entire topic level. MQTT wildcards
  and overlapping contract routes are rejected.
- IDs and metric keys use 1–32 ASCII letters, digits, `_`, or `-`.
- `version` is a required positive integer in both the contract and payload.
- `equals` contains required values addressed with JSON Pointer; it defaults to `{}`.
- Metrics use JSON Pointer and must be finite JSON numbers. Missing optional
  fields are skipped, but explicit `null` or strings are rejected. Unknown JSON
  fields are preserved in the raw payload and otherwise ignored.
- The collector timestamps arrival in UTC milliseconds. It does not interpret
  device uptime as wall-clock time. Grafana displays local time.
- Metric graphs average into Grafana's chosen time buckets; raw samples remain
  available in SQLite. Units use Grafana unit IDs.
- Give incompatible schemas a new contract ID and versioned topic. Old data
  remains until retention expires; keep the old contract registered if its
  dashboard must remain available.

The format intentionally covers flat/nested numeric readings and constant
discriminators. It does not generate firmware serializers or expand arrays of
scan results. Publish one reading per message or extend the contract model when
adding those device types.

## Storage and delivery

The Rust collector embeds SQLite through `rusqlite`; there is no database server.
It stores raw accepted JSON in `messages`, numeric series in `samples`, and
invalid input in `rejected` with a reason. A transaction commits the raw message
and every extracted metric together before acknowledging MQTT QoS 1 delivery.
Malformed messages are persisted and acknowledged so they do not loop forever.

Collection uses a persistent MQTT session, automatic reconnect, WAL, full
synchronous commits, and timestamp indexes. QoS 1 can deliver duplicates,
especially after a crash between commit and acknowledgement; samples are not
deduplicated. This is a development recorder, not an exactly-once event store.
The broker's queue is bounded at 10,000 messages per offline session and broker
persistence is periodic; this is not a guarantee against all power-loss scenarios.
Contract revisions start a new session to avoid retaining removed subscriptions.

Default retention is seven days, pruned at startup and every minute. Expired raw
messages and their metric rows are deleted together. SQLite reuses freed pages;
retention does not immediately shrink the file or impose a hard disk-size limit.
Database failures stop the collector; Docker restarts it and unacknowledged
QoS 1 messages can be redelivered. Payloads are bounded at 64 KiB.

The Compose volumes `embedded-sdk-dev_telemetry-data`,
`embedded-sdk-dev_mqtt-data`, and `embedded-sdk-dev_grafana-data` survive `down`,
rebuilds, and `cargo clean`. Generated contracts/dashboards live in
`target/dev-backend` and are rebuilt on the next startup. SQLite is at
`/data/telemetry.sqlite` inside the collector. Grafana mounts the same Linux
volume read-only, avoiding SQLite WAL locking across Docker Desktop host mounts.
Use SQLite's backup API or stop the stack before copying the database together
with any WAL sidecar; copying just a live `.sqlite` file can miss committed data.

## Local configuration

```sh
GRAFANA_PORT=3300 BACKEND_MQTT_PORT=1884 RETENTION_DAYS=14 cargo xtask telemetry
```

Use the same overrides on subsequent commands. Retention accepts 1–3650 days.
MQTT binds to all host IPv4 interfaces so physical devices can reach it.
`BACKEND_MQTT_BIND=127.0.0.1` restricts it to local testing. Grafana binds only
to loopback. This broker intentionally uses anonymous plaintext MQTT, matching
the firmware's explicit isolated-fixture mode. Use it on a trusted development
LAN, without reusable credentials or sensitive payloads; do not expose it to
the internet.

If startup fails, check `cargo xtask telemetry logs` and `status`. Common causes
are Docker not running, occupied ports, a failed first-time plugin download,
or invalid contract JSON. `up --wait` failures leave services available for
inspection; `cargo xtask telemetry down` stops them.

## Why SQLite

SQLite fits low-volume development telemetry: embedded, portable, no database
daemon, and standard SQL over indexed timestamps. Its
[WAL mode](https://www.sqlite.org/wal.html) permits concurrent readers, and the
[Grafana SQLite plugin](https://grafana.com/grafana/plugins/frser-sqlite-datasource/)
supports time series queries. It is not a specialized compressed time-series
engine; retention and bucketing are explicit here.

[Turso](https://turso.tech/local-first) provides local-first database and sync
capabilities, which this single-computer workflow does not need. A dedicated
server such as [QuestDB](https://questdb.com/docs/getting-started/quick-start/)
is an option if ingestion volume and analytical requirements outgrow this setup.

## Verification

```sh
cargo test -p embedded-sdk-dev-backend -p xtask
cargo clippy -p embedded-sdk-dev-backend -p xtask --all-targets -- -D warnings
cargo xtask telemetry demo
python3 infra/backend/smoke.py
```

Host integration tests cover route ambiguity, malformed contracts, wire fixture
compatibility, validation/quarantine, atomic metric insertion, persistence,
retention, and execution of generated dashboard SQL with quoted device IDs.
`demo` exercises the real broker/collector/database path without flashing hardware.
The optional Python stdlib smoke test sends MQTT 5 messages and checks stored
data, quarantined invalid input, device discovery, and every provisioned panel
through Grafana's actual SQLite plugin. It leaves a uniquely named smoke device
in the development data until retention removes it.
