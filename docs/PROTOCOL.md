# PROTOCOL — kontrakt wspólny (`pkpu-proto`)

Jedno źródło prawdy dla wszystkiego, co przekracza granicę procesu.
Crate `no_std + alloc`, zależności: `serde`, `postcard`, `heapless`.
Kompiluje się na Cortex-M, na serwerze i do biblioteki mobilnej.

---

## 1. Identyfikatory

```rust
/// UUIDv7 — 128 bit, sortowalny po czasie utworzenia.
/// Forma tekstowa: Crockford base32, 26 znaków, bez myślników.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId([u8; 16]);

/// Skrót do adresacji w PAN i w topicach o ograniczonej długości.
pub struct ShortId(u32);

/// Identyfikator najemcy — obecny w każdej wiadomości sterującej.
pub struct TenantId(Uuid);
```

Dlaczego UUIDv7, a nie sekwencja z bazy: urządzenia dostają `device_id` **przy
produkcji**, offline, bez dostępu do bazy. Sortowalność po czasie daje przy okazji
dobrą lokalność w indeksach Postgresa.

---

## 2. Enumy z DEVICE.md

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

/// Rodzina krzemu. Rdzeń firmware jest przenośny, obraz binarny nie —
/// to jedyne miejsce, w którym platforma przecieka do kontraktu.
/// Używane wyłącznie do doboru artefaktu OTA i do inwentarza floty,
/// NIGDY jako warunek w logice aplikacyjnej.
#[repr(u8)]
pub enum Platform { Nrf52 = 0, Nrf53 = 1, Stm32 = 2, Esp32Riscv = 3, Esp32Xtensa = 4 }
```

`#[repr(u8)]` z jawnymi wartościami: te liczby trafiają do bazy i na radio.
**Nigdy nie zmieniamy istniejącej wartości — tylko dopisujemy nowe na końcu.**

---

## 3. Ramka aplikacyjna

```rust
pub struct Envelope<'a> {
    pub v:        u8,            // wersja protokołu
    pub device:   DeviceId,
    pub seq:      u32,           // monotoniczny licznik, do wykrywania luk
    pub ts:       Timestamp,     // Unix ms  |  Uptime ms jeśli brak synchro
    pub body:     Frame<'a>,
}

pub enum Frame<'a> {
    Hello(Hello),                    // przy nawiązaniu sesji
    Telemetry(Telemetry<'a>),        // pomiary
    Event(Event),                    // zdarzenia dyskretne (alarm, przycisk)
    StateReport(StateReport<'a>),    // reported shadow
    CommandAck(CommandAck),          // potwierdzenie/odrzucenie komendy
    OtaStatus(OtaStatus),
    Command(Command<'a>),            // cloud -> device
    StateDesired(StateDesired<'a>),  // cloud -> device
    OtaOffer(OtaOffer),              // cloud -> device
    Time(TimeSync),                  // cloud -> device
}
```

### Telemetria

```rust
pub struct Telemetry<'a> {
    pub backfill: bool,              // dane z bufora offline
    pub samples:  &'a [Sample],
}

pub struct Sample {
    pub ch:  ChannelId,   // u16 — kanał pomiarowy, definiowany per model
    pub ts:  i64,         // offset ms względem Envelope.ts
    pub val: Value,
}

pub enum Value { I32(i32), F32(f32), Bool(bool), U8(u8) }
```

Kanały (`ChannelId`) są **numeryczne, nie tekstowe** — nazwa i jednostka żyją
w rejestrze `device_type_channel` w bazie. Ramka radiowa nie wozi stringów.

### Komenda

```rust
pub struct Command<'a> {
    pub id:      CommandId,   // UUIDv7 — klucz idempotencji
    pub expires: i64,         // Unix ms; po tym czasie device odrzuca
    pub op:      &'a str,     // np. "set_output", "reboot", "factory_reset"
    pub args:    &'a [u8],    // postcard, schemat zależny od `op`
}

pub struct CommandAck {
    pub id:     CommandId,
    pub result: AckResult,    // Accepted | Done | Rejected(Reason) | Expired
}
```

---

## 4. Kodowanie i wersjonowanie

| Granica | Format | Powód |
|---|---|---|
| device <-> cloud | `postcard` (binarne) | 3–5× mniejsze niż JSON, `no_std`, zero-copy |
| cloud <-> cloud (NATS) | `postcard` | ten sam typ, bez rekonwersji |
| cloud <-> web/mobile | JSON (`serde_json`) | debugowalność, narzędzia |

Ten sam typ `#[derive(Serialize, Deserialize)]` obsługuje wszystkie trzy —
różni się tylko backend serde.

**Zasady zmian (kompatybilność wsteczna jest obowiązkowa):**

1. Pole można **dodać** tylko jako `Option<T>` lub z `#[serde(default)]`.
2. Wariantu enuma nigdy nie usuwamy ani nie przenumerowujemy.
3. Zmiana łamiąca = `Envelope.v` w górę; chmura obsługuje N i N-1 przez min. 12 miesięcy.
4. Urządzenia w polu nie zawsze się zaktualizują. Chmura musi rozumieć każdą
   wersję, która kiedykolwiek wyszła z fabryki.

Testy złotych wektorów: katalog `proto/pkpu-proto/tests/golden/` z zapisanymi
bajtami każdej wersji ramki. Zmiana, która psuje dekodowanie starego wektora,
wywala CI.

---

## 5. Adresacja MQTT

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

- Autoryzacja na brokerze: identyfikator z certyfikatu klienta **musi** równać się
  `{device_id}` w topicu. Bez tego jedno skompromitowane urządzenie podszywa się
  pod całą flotę.
- QoS 1 dla telemetrii i komend, QoS 0 dla `time`.
- LWT (Last Will) na `dev/{id}/state` z `DeviceState::Disconnected`.
- Retained: tylko `desired` — urządzenie po wybudzeniu dostaje aktualny stan
  docelowy bez odpytywania.

Dla `OT_THREAD` z natywnym IP alternatywą jest CoAP + DTLS zamiast MQTT —
mapowanie topiców 1:1 na ścieżki CoAP. Decyzja otwarta, patrz DECISIONS.md.

---

## 6. Device Shadow

```rust
pub struct Shadow {
    pub desired:  Map<PropId, Value>,
    pub reported: Map<PropId, Value>,
    pub version:  u64,          // rośnie przy każdej zmianie desired
    pub updated:  Timestamp,
}
```

Reguły:
- Chmura pisze **tylko** `desired`, urządzenie pisze **tylko** `reported`.
- Delta = `desired \ reported`; to ona jest wysyłana do urządzenia, nie cały shadow.
- Urządzenie po `Hello` dostaje pełną deltę — to zastępuje odtwarzanie sesji.
- Konflikt (dwóch użytkowników): wygrywa ostatni zapis, ale `version`
  w żądaniu API pozwala klientowi wykryć wyścig (optimistic concurrency).

---

## 7. Generowanie schematów

Crate `pkpu-schema` (build-time) generuje z tych samych typów:

- **OpenAPI 3.1** dla `pkpu-api` (przez `utoipa`),
- **JSON Schema** dla walidacji payloadów w regułach,
- **definicje UniFFI** dla mobile,
- **plik `.sql`** z enumami dla migracji Postgresa.

Nic nie jest pisane ręcznie w dwóch miejscach.
