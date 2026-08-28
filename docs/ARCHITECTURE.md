# Architektura ogólna

## 1. Widok systemu

```
  +---------------------------- EDGE ----------------------------+
  |                                                              |
  |  [WIFI device] ------------ TLS/MQTT --------------+         |
  |                                                    |         |
  |  [OT_THREAD device] --+                            |         |
  |  [ZIGBEE device] -----+--- radio --- [GATEWAY]-----+         |
  |  [BLE device] --------+              (Rust, Linux) |         |
  |                                                    |         |
  |  [BLE device] --- GATT --- [MOBILE APP] -----------+         |
  +----------------------------------------------------+---------+
                                                       |
  +------------------------ CLOUD ----------------------v--------+
  |                                                              |
  |   +----------+    +--------------+    +------------------+   |
  |   |  broker  |--->|    ingest    |--->|  NATS JetStream  |   |
  |   |  (MQTT5) |<---|   (Rust)     |<---|   (event bus)    |   |
  |   +----------+    +------+-------+    +----+-------+-----+   |
  |                          |                 |       |         |
  |            +-------------v--+   +----------v--+  +-v------+  |
  |            | device-registry|   | rules/alert |  |  ota   |  |
  |            |   + shadow     |   |             |  |        |  |
  |            +-------+--------+   +-------------+  +---+----+  |
  |                    |                                 |       |
  |   +----------------v-----------------+        +------v-----+ |
  |   |  PostgreSQL + TimescaleDB        |        |  S3/MinIO  | |
  |   |  (registry, shadow, telemetria)  |        | (firmware) | |
  |   +----------------------------------+        +------------+ |
  |                    ^                                         |
  |            +-------+--------+                                |
  |            |   api (axum)   |<-- OIDC/JWT -- [WEB] [MOBILE]  |
  |            +----------------+                                |
  +--------------------------------------------------------------+
```

## 2. Granice odpowiedzialności

| Warstwa | Odpowiada za | NIE odpowiada za |
|---|---|---|
| **Device** | pomiar, lokalne sterowanie, buforowanie offline, bezpieczny boot | logikę biznesową wielu urządzeń, agregacje |
| **Gateway** | mostkowanie radia <-> IP, backhaul offline, lokalne reguły awaryjne | trwałe przechowywanie danych, autoryzację użytkowników |
| **Cloud** | tożsamość, shadow, reguły, historia, OTA, API | pomiar czasu rzeczywistego (<100 ms) |
| **Mobile** | provisioning, UI, sterowanie lokalne BLE | źródło prawdy o stanie |

**Reguła:** urządzenie musi działać poprawnie przy braku chmury przez czas
zdefiniowany w profilu produktu (domyślnie: 72 h buforowania telemetrii,
nieograniczone lokalne sterowanie).

## 3. Model wdrożeniowy: dwie ścieżki połączenia

**Ścieżka A — direct-to-cloud** (`COM_TYPE = WIFI`)
Urządzenie ma stos IP, łączy się bezpośrednio z brokerem po mTLS.
Prosta topologia, wyższy pobór mocy, wymaga poświadczeń Wi-Fi użytkownika.

**Ścieżka B — via gateway** (`COM_TYPE = OT_THREAD | ZIGBEE | BLE`)
Urządzenie mówi tylko radiem PAN. Gateway (Linux + Rust) tłumaczy na MQTT.
Niski pobór mocy, mesh, ale wymaga sprzedaży/instalacji gatewaya.

Warstwa aplikacyjna jest **identyczna w obu ścieżkach** — patrz
[PROTOCOL.md](PROTOCOL.md). Różnica jest zamknięta w traicie `Link`.

## 4. Stos technologiczny — decyzje domyślne

### Device

| Element | Wybór | Uzasadnienie |
|---|---|---|
| Async runtime | `embassy` | de facto standard, `no_std`, zero-alloc, dobre wsparcie nRF/ESP/STM32 |
| MCU (mesh/BLE) | nRF52840 / nRF5340 | najlepiej wspierany w Rust, multiprotokół |
| MCU (Wi-Fi) | ESP32-C6 / ESP32-S3 | `esp-hal` + `esp-wifi`, Wi-Fi 6 + 802.15.4 w C6 |
| MCU (przemysł/low-power, peryferia) | STM32 WB55 / U5 / L4 | `embassy-stm32`, szeroka dostępność, secure enclave w U5 |
| BLE host | `trouble` | czysty Rust, integracja z embassy |
| Thread | OpenThread jako RCP/NCP | brak dojrzałego stosu Thread w Rust — patrz ADR-004 |
| Zigbee | stos producenta jako NCP | brak stosu Zigbee w Rust — patrz ADR-004 |
| Storage | `sequential-storage` | log-structured KV na surowym flashu |
| Serializacja | `postcard` | kompaktowy, `no_std`, ten sam `serde` co w chmurze |

**Przenośny rdzeń SDK:** `pkpu-device-core` i logika produktowa kompilują się
bez zmian na **wszystkie trzy** rodziny (nRF, ESP32, STM32). Krzem wybieramy per
produkt — dostępnością, ceną i peryferiami, nie kosztem przepisania firmware.
Różnice są zamknięte w `pkpu-platform-*` i `boards/`; kontrakt portu, macierz
targetów i znane ograniczenia — [DEVICE.md](DEVICE.md) sekcja 4, [ADR-011](DECISIONS.md).

### Cloud

| Element | Wybór | Uzasadnienie |
|---|---|---|
| HTTP/WS API | `axum` + `tower` | ekosystem tokio, middleware |
| Broker MQTT | `rumqttd` (jako biblioteka) | Rust, osadzalny w naszym procesie ingest |
| Event bus | NATS JetStream | prostszy operacyjnie niż Kafka, wystarczy do ~100k msg/s |
| DB | PostgreSQL 16 + TimescaleDB | jedna baza na wszystko, hypertable na telemetrię |
| Dostęp do DB | `sqlx` (compile-time checked) | zapytania weryfikowane przy `cargo build` |
| Cache/presence | Valkey (Redis) | TTL presence, rate-limit, dedup komend |
| Obiekty | S3 / MinIO | artefakty firmware, eksporty |
| Obserwowalność | `tracing` + OTLP -> Grafana/Tempo/Loki | jedno API logów i traceów |

### Mobile

| Element | Wybór |
|---|---|
| Rdzeń | `pkpu-core` (Rust) eksportowany przez **UniFFI** |
| UI | Kotlin/Compose + Swift/SwiftUI (cienkie) |
| BLE | natywne API platformy, wywoływane przez callbacki z rdzenia |

## 5. Layout repozytoriów

Monorepo z **trzema workspace'ami** (różne targety kompilacji nie dzielą
sensownie jednego `Cargo.lock`):

```
pkpu/
├── proto/                    # workspace 1 — kontrakty (no_std)
│   ├── pkpu-proto/           #   typy wiadomości, DeviceId, enumy z DEVICE.md
│   ├── pkpu-crypto/          #   podpisy ed25519, KDF, format manifestu OTA
│   └── pkpu-schema/          #   generator: JSON Schema + OpenAPI z typów Rust
│
├── firmware/                 # workspace 2 — no_std, target thumbv7em / riscv32
│   ├── pkpu-device-core/     #   PRZENOŚNE: maszyna stanów, scheduler, OTA, storage
│   ├── pkpu-link/            #   trait Link + impl per stos radiowy: wifi, thread, zigbee, ble
│   ├── pkpu-hal/             #   traity sprzętowe (Sensor, Actuator, PowerRail, Platform)
│   ├── platform/             #   NIEPRZENOŚNE: impl traitów per krzem
│   │   ├── pkpu-platform-nrf/
│   │   ├── pkpu-platform-stm32/
│   │   └── pkpu-platform-esp/
│   ├── boards/               #   BSP per płytka (pinout, clock, partycje flash)
│   └── apps/                 #   binarki produktowe (np. apps/sensor-th/)
│
├── cloud/                    # workspace 3 — std, tokio, target x86_64 / aarch64
│   ├── pkpu-ingest/          #   broker + walidacja + publikacja na bus
│   ├── pkpu-registry/        #   rejestr urządzeń + device shadow
│   ├── pkpu-provisioning/    #   attestacja, wydawanie certyfikatów, claiming
│   ├── pkpu-ota/             #   kampanie, rollout, manifesty
│   ├── pkpu-rules/           #   reguły, alerty, webhooki
│   ├── pkpu-api/             #   REST/WS dla web i mobile
│   ├── pkpu-gateway/         #   binarka na Linux edge (RPi / CM4)
│   └── pkpu-cli/             #   narzędzia operacyjne i fabryczne
│
├── mobile/                   # pkpu-core (UniFFI) + shells iOS/Android
└── docs/                     # ta dokumentacja
```

`proto/` jest wciągane do `firmware/` i `cloud/` przez `path = "../proto/..."`,
nie przez rejestr. Jeden commit zmienia kontrakt i obie strony naraz.

Wewnątrz `firmware/` obowiązuje ta sama zasada w pionie: wszystko powyżej
`platform/` jest wspólne dla nRF, STM32 i ESP32. Zależność idzie tylko w dół —
`pkpu-device-core` nie zna nazwy żadnego producenta.

## 6. Przepływy krytyczne

### 6.1 Telemetria (device -> cloud)

```
sensor -> device-core (buforuje w RAM/flash)
       -> Link::send(Frame::Telemetry)
       -> [gateway?] -> broker MQTT   topic: dev/{device_id}/tel
       -> ingest: weryfikacja tożsamości + dekodowanie postcard
       -> NATS: tel.{tenant}.{device_type}.{device_id}
       -> sink: TimescaleDB (batch COPY) | rules: ewaluacja | ws: push do UI
```

### 6.2 Komenda (cloud -> device)

```
API -> registry: zapis desired state w shadow (wersjonowany)
    -> NATS: cmd.{device_id}
    -> ingest -> broker   topic: dev/{device_id}/cmd
    -> [SLEEP? kolejkuj do następnego wybudzenia / poll]
    -> device wykonuje -> publikuje reported state
    -> registry: reconcile desired vs reported, zamknięcie komendy
```

Komendy są **idempotentne** (`command_id` + dedup w Valkey) i mają TTL.
Dla `SLEEP_TYPE = SLEEP` domyślny TTL = 4 × interwał raportowania.

### 6.3 Provisioning

Patrz [DEVICE.md](DEVICE.md) — pełna sekwencja BLE i NFC.

### 6.4 OTA

```
CI buduje firmware -> podpisuje ed25519 -> upload artefaktu do S3
  -> pkpu-ota: kampania (kohorta = filtr po model / hw_rev / fw_version / site)
  -> rollout falowy: 1% -> 10% -> 50% -> 100%, bramka na metryce błędów
  -> device: pobiera do slotu B, weryfikuje podpis + hash, boot, potwierdza
  -> brak potwierdzenia w N boot -> bootloader rollback do slotu A
```

## 7. Bezpieczeństwo — model bazowy

- **Tożsamość urządzenia**: klucz prywatny generowany **na urządzeniu** przy
  produkcji, nigdy nie opuszcza chipa. Klucz publiczny + `device_id` trafiają do
  rejestru fabrycznego. Preferowany secure element (ATECC608 / nRF CryptoCell).
- **Transport**: mTLS do brokera (Wi-Fi / gateway), DTLS lub OSCORE albo
  link-layer security w PAN (Thread: MLE + AES-CCM; Zigbee: TC link key).
- **Autoryzacja użytkownika**: OIDC (Keycloak self-hosted lub zewnętrzny IdP),
  JWT z `tenant_id`, RBAC na poziomie `site`.
- **Multi-tenancy**: `tenant_id` w każdej tabeli, wymuszony przez Row Level
  Security w PostgreSQL — nie tylko przez warstwę aplikacji.
- **Secrets**: nic w repo. SOPS/age dla konfiguracji, Vault opcjonalnie.
- **Audyt**: każda zmiana desired state, każda kampania OTA, każde claimowanie
  urządzenia -> tabela `audit_log` (append-only).

## 8. Kolejność budowy

| Etap | Zakres | Efekt |
|---|---|---|
| 0 | `pkpu-proto` + schemat DB + ADR | zamrożony kontrakt |
| 1 | `pkpu-ingest` + Timescale + jedno urządzenie Wi-Fi (ESP32-C6) | telemetria end-to-end |
| 2 | `pkpu-registry` + shadow + komendy | sterowanie dwukierunkowe |
| 3 | Provisioning BLE + `pkpu-core` mobilny | onboarding użytkownika |
| 4 | OTA + bootloader A/B | serwisowalność w polu |
| 5 | Thread/Zigbee + `pkpu-gateway` + drugi krzem (nRF52840) | urządzenia bateryjne (SLEEP), weryfikacja przenośności rdzenia |
| 6 | Reguły, alerty, dashboard, multi-tenant | produkt |

Etapy 1–4 na jednym typie urządzenia. Dopiero potem druga technologia radiowa —
inaczej abstrakcja `Link` powstanie na podstawie zgadywania, nie doświadczenia.
To samo dotyczy przenośności: `pkpu-hal` projektujemy tak, by port był możliwy
(sekcja „kontrakt portu"), ale drugą rodzinę MCU realnie uruchamiamy w etapie 5.
Trzy platformy naraz od pierwszego dnia dają abstrakcję opartą na wyobrażeniach.
