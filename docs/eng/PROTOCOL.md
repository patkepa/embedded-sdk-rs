# PROTOCOL — the shared contract (`pkpu-proto`)

A single source of truth for everything that crosses a process boundary.
A `no_std + alloc` crate; dependencies: `serde`, `postcard`, `heapless`.
It compiles on Cortex-M, on the server and into the mobile library.

---

## 1. Identifiers

```rust
/// UUIDv7 — 128 bit, sortable by creation time.
/// Text form: Crockford base32, 26 characters, no dashes.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId([u8; 16]);

/// Short form for addressing inside the PAN and in length-limited topics.
pub struct ShortId(u32);

/// Tenant identifier — present in every control message.
pub struct TenantId(Uuid);
```

Why UUIDv7 rather than a database sequence: devices are given their `device_id`
**during manufacturing**, offline, with no access to the database. Sortability by
time also gives good locality in Postgres indexes as a side effect.

---

## 2. Enums from DEVICE.md

```rust
#[derive(Copy, Clone, Serialize, Deserialize)]
#[repr(u8)]
pub enum SleepType { NonSleep = 0, Sleep = 1 }

#[repr(u8)]
pub enum ComType { Wifi = 0, OtThread = 1, Zigbee = 2, Ble = 3 }

#[repr(u8)]
pub enum ProvType { Ble = 0, Nfc = 1 }

#[repr(u8)]
pub enum DeviceState { Online = 0, Disconnected = 1, Sleeps = 2 }

/// Silicon family. The firmware core is portable, the binary image is not —
/// this is the only place where the platform leaks into the contract.
/// Used exclusively to pick the OTA artifact and for the fleet inventory,
/// NEVER as a condition in application logic.
#[repr(u8)]
pub enum Platform { Nrf52 = 0, Nrf53 = 1, Stm32 = 2, Esp32Riscv = 3, Esp32Xtensa = 4 }

/// The relation of the device to the Matter ecosystem. A dimension ORTHOGONAL
/// to ComType: Matter is an application layer above IPv6, not a link technology.
/// See MATTER.md section 4.
#[repr(u8)]
pub enum MatterMode { None = 0, Bridged = 1, Dual = 2, Native = 3 }
```

`#[repr(u8)]` with explicit values: these numbers end up in the database and on
the radio. **We never change an existing value — we only append new ones at the
end.**

---

## 3. Application frame

```rust
pub struct Envelope<'a> {
    pub v:        u8,            // protocol version
    pub device:   DeviceId,
    pub seq:      u32,           // monotonic counter, for gap detection
    pub ts:       Timestamp,     // Unix ms  |  Uptime ms if unsynchronized
    pub body:     Frame<'a>,
}

pub enum Frame<'a> {
    Hello(Hello),                    // on session establishment
    Telemetry(Telemetry<'a>),        // measurements
    Event(Event),                    // discrete events (alarm, button)
    StateReport(StateReport<'a>),    // reported shadow
    CommandAck(CommandAck),          // command acknowledgement/rejection
    OtaStatus(OtaStatus),
    Command(Command<'a>),            // cloud -> device
    StateDesired(StateDesired<'a>),  // cloud -> device
    OtaOffer(OtaOffer),              // cloud -> device
    Time(TimeSync),                  // cloud -> device
}
```

### Telemetry

```rust
pub struct Telemetry<'a> {
    pub backfill: bool,              // data from the offline buffer
    pub samples:  &'a [Sample],
}

pub struct Sample {
    pub ch:  ChannelId,   // u16 — measurement channel, defined per model
    pub ts:  i64,         // ms offset relative to Envelope.ts
    pub val: Value,
}

pub enum Value { I32(i32), F32(f32), Bool(bool), U8(u8) }
```

Channels (`ChannelId`) are **numeric, not textual** — the name and the unit live
in the `device_type_channel` registry in the database. The radio frame does not
carry strings.

### Command

```rust
pub struct Command<'a> {
    pub id:      CommandId,   // UUIDv7 — idempotency key
    pub expires: i64,         // Unix ms; after this the device rejects it
    pub op:      &'a str,     // e.g. "set_output", "reboot", "factory_reset"
    pub args:    &'a [u8],    // postcard, schema depends on `op`
}

pub struct CommandAck {
    pub id:     CommandId,
    pub result: AckResult,    // Accepted | Done | Rejected(Reason) | Expired
}
```

---

## 4. Encoding and versioning

| Boundary | Format | Reason |
|---|---|---|
| device <-> cloud | `postcard` (binary) | 3–5× smaller than JSON, `no_std`, zero-copy |
| cloud <-> cloud (NATS) | `postcard` | the same type, no reconversion |
| cloud <-> web/mobile | JSON (`serde_json`) | debuggability, tooling |

The same `#[derive(Serialize, Deserialize)]` type serves all three — only the
serde backend differs.

**Change rules (backward compatibility is mandatory):**

1. A field may be **added** only as `Option<T>` or with `#[serde(default)]`.
2. An enum variant is never removed or renumbered.
3. A breaking change = bump `Envelope.v`; the cloud supports N and N-1 for at
   least 12 months.
4. Devices in the field will not always update. The cloud has to understand
   every version that has ever left the factory.

Golden vector tests: the `proto/pkpu-proto/tests/golden/` directory holds the
recorded bytes of every frame version. A change that breaks decoding of an old
vector fails CI.

---

## 5. MQTT addressing

```
dev/{device_id}/hello         device -> cloud
dev/{device_id}/tel           device -> cloud
dev/{device_id}/evt           device -> cloud
dev/{device_id}/state         device -> cloud   (reported)
dev/{device_id}/ack           device -> cloud
dev/{device_id}/ota           device <-> cloud

dev/{device_id}/cmd           cloud -> device
dev/{device_id}/desired       cloud -> device
dev/{device_id}/time          cloud -> device
```

- Authorization on the broker: the identifier from the client certificate
  **must** equal the `{device_id}` in the topic. Without that, a single
  compromised device impersonates the whole fleet.
- QoS 1 for telemetry and commands, QoS 0 for `time`.
- LWT (Last Will) on `dev/{id}/state` with `DeviceState::Disconnected`.
- Retained: only `desired` — after waking up, the device receives the current
  target state without polling.

For `OT_THREAD` with native IP, CoAP + DTLS instead of MQTT is an alternative —
topics map 1:1 onto CoAP paths. The decision is open, see DECISIONS.md.

---

## 6. Device Shadow

```rust
pub struct Shadow {
    pub desired:  Map<PropId, Value>,
    pub reported: Map<PropId, Value>,
    pub version:  u64,          // increments on every desired change
    pub updated:  Timestamp,
}
```

Rules:
- The cloud writes **only** `desired`, the device writes **only** `reported`.
- Delta = `desired \ reported`; that is what is sent to the device, not the whole
  shadow.
- After `Hello` the device receives the full delta — this replaces session
  replay.
- Conflict (two users): last write wins, but the `version` in the API request
  lets the client detect the race (optimistic concurrency).

---

## 7. Schema generation

The `pkpu-schema` crate (build-time) generates, from the same types:

- **OpenAPI 3.1** for `pkpu-api` (via `utoipa`),
- **JSON Schema** for validating payloads in rules,
- **UniFFI definitions** for mobile,
- **a `.sql` file** with the enums for Postgres migrations.

Nothing is written by hand in two places.
