# DECISIONS — log decyzji architektonicznych

Format: krótkie ADR. Status: `proposed` (do potwierdzenia) / `accepted` /
`superseded`. Wszystko poniżej ma status `proposed` — to propozycje do
przedyskutowania, nie ustalenia.

---

## ADR-001 — Rust w całym stosie

**Status:** proposed
**Decyzja:** firmware, chmura, gateway i rdzeń mobilny w Rust.
**Konsekwencje:**
- (+) jeden kontrakt typów przez wszystkie granice, brak rozjazdu schematów,
- (+) klasa błędów pamięciowych znika z firmware i z parserów sieciowych,
- (−) stosy radiowe (Thread, Zigbee) nie istnieją w Rust — konieczne FFI (ADR-004),
- (−) mniejsza pula ludzi na rynku niż C/Python/Go.

---

## ADR-002 — Monorepo, trzy workspace'y

**Status:** proposed
**Decyzja:** jedno repo, osobne workspace'y `proto/`, `firmware/`, `cloud/`.
**Alternatywy:** osobne repo per komponent (rozjazd wersji kontraktu),
jeden workspace (konflikty targetów, `no_std` vs `std` w jednym `Cargo.lock`).

---

## ADR-003 — `postcard` na łączu, JSON na API

**Status:** proposed
**Decyzja:** ten sam typ `serde` serializowany binarnie w kierunku urządzenia
i jako JSON w kierunku UI.
**Alternatywy:** CBOR (bardziej standardowy, ~20% większy), Protobuf (osobny IDL
i generowanie kodu — duplikacja źródła prawdy).

---

## ADR-004 — Thread i Zigbee jako radio co-processor (RCP/NCP)

**Status:** proposed
**Problem:** nie istnieją produkcyjne stosy Thread ani Zigbee w Rust.
**Decyzja:** aplikacja w Rust na hoście, certyfikowany stos producenta na
co-procesorze radiowym, komunikacja przez Spinel (Thread) / EZSP (Zigbee).
**Konsekwencje:**
- (+) certyfikacja Thread Group / Zigbee Alliance realnie osiągalna,
- (+) `unsafe`/C odizolowane za granicą sprzętową, nie w naszym procesie,
- (−) dwa układy = wyższy BOM i wyższy pobór mocy,
- (−) alternatywa jednoukładowa wymaga FFI do stosu C w tym samym MCU
  (tańsza, ale miesza C i Rust w jednym obrazie).
**Do rozstrzygnięcia:** czy dla urządzeń bateryjnych `SLEEP` akceptujemy
koszt energetyczny dwóch układów, czy idziemy w FFI single-chip.

---

## ADR-005 — MQTT 5 jako protokół urządzenie–chmura

**Status:** proposed
**Decyzja:** MQTT 5 + mTLS, broker osadzony w `pkpu-ingest`.
**Alternatywy:**
- CoAP + DTLS — naturalniejsze dla Thread i urządzeń `SLEEP`, mniejszy narzut,
  ale słabszy ekosystem narzędziowy,
- HTTP/3 + QUIC — dobre wznawianie sesji dla urządzeń bateryjnych, niedojrzałe
  na MCU.
**Otwarte:** czy dla `OT_THREAD` wystawiamy równolegle endpoint CoAP.

---

## ADR-006 — PostgreSQL + TimescaleDB jako jedyna baza

**Status:** proposed
**Decyzja:** rejestr, shadow, telemetria, audyt w jednej instancji.
**Próg rewizji:** > 500 tys. punktów/s zapisu **lub** > 10 TB danych po kompresji.
Wtedy: wydzielenie telemetrii do ClickHouse, rejestr zostaje w Postgresie.
**Alternatywy odrzucone na tym etapie:** InfluxDB (słabe joiny z metadanymi),
ClickHouse od razu (dwa systemy do utrzymania od pierwszego dnia).

---

## ADR-007 — Tożsamość urządzenia oparta o klucz generowany na chipie

**Status:** proposed
**Decyzja:** klucz prywatny ed25519 generowany na urządzeniu przy produkcji,
nigdy nie opuszcza chipa; dwa CA (fabryczne offline, operacyjne online).
**Konsekwencje:** linia produkcyjna musi mieć zabezpieczone stanowisko
provisioningu z dostępem do rejestru fabrycznego (mTLS, osobne poświadczenia).

---

## ADR-008 — Model wąski (long) telemetrii

**Status:** proposed
**Decyzja:** jeden wiersz = jeden pomiar jednego kanału; kanały numeryczne,
metadane w `device_type_channels`.
**Alternatywa:** wiersz szeroki per model (mniej wierszy, ale migracja przy
każdym nowym czujniku i rzadkie kolumny dla modeli o różnym zestawie sensorów).

---

## ADR-009 — Rozróżnienie `SLEEPS` vs `DISCONNECTED`

**Status:** proposed
**Decyzja:** watchdog presence liczy okno z `expected_interval × tolerancja`
na podstawie `sleep_type` z `device_types`.
**Uzasadnienie:** bez tego urządzenia bateryjne generują ciągłe fałszywe alarmy,
co w praktyce prowadzi do wyłączenia alertów przez operatora — czyli do utraty
całej wartości monitoringu.

---

## ADR-010 — Kolejność budowy: Wi-Fi najpierw

**Status:** proposed
**Decyzja:** pierwszy pełny przepływ end-to-end na ESP32-C6 przez Wi-Fi,
dopiero potem Thread/Zigbee i gateway.
**Uzasadnienie:** abstrakcja `Link` zaprojektowana na podstawie jednej
działającej implementacji i drugiej realnie zaimplementowanej — nie na podstawie
przewidywań o czterech technologiach naraz.

---

## ADR-011 — Przenośny rdzeń firmware (jeden SDK na nRF, STM32 i ESP32)

**Status:** proposed
**Decyzja:** `pkpu-device-core`, `pkpu-hal`, `pkpu-link` i kod produktowy
w `apps/` są przenośne między rodzinami MCU. Cała wiedza o krzemie mieszka
w `pkpu-platform-{nrf,stm32,esp}` i `boards/`. Wspólnym mianownikiem jest
`embassy` + `embedded-hal` 1.0; wybór platformy to wybór BSP i cech kompilacji.
**Uzasadnienie:**
- dostępność krzemu bywa decyzją narzuconą (cena, lead time, peryferium,
  wymóg klienta) — nie może kosztować przepisania firmware,
- Wi-Fi praktycznie prowadzi do ESP32, a 802.15.4/BLE o niskim poborze do nRF —
  bez wspólnego rdzenia i tak mielibyśmy dwie bazy kodu w jednym produkcie,
- ten sam zestaw traitów daje darmowo build hostowy i testy jednostkowe core'a.

**Konsekwencje:**
- (+) jedna maszyna stanów, jeden klient OTA, jeden bufor telemetrii — jeden
  zestaw błędów do naprawienia zamiast trzech,
- (+) port na nową rodzinę to skończona checklista (DEVICE.md sekcja 4.4),
  nie projekt od nowa,
- (−) związanie z ekosystemem embassy: MCU bez jego wsparcia wypada z macierzy,
- (−) koszt stały CI: build całej macierzy targetów przy każdym PR,
- (−) najniższy wspólny mianownik — peryferia specyficzne dla platformy
  (CryptoCell, PKA, DS) są dostępne tylko przez trait albo wcale,
- (−) Xtensa (ESP32-S3) wymaga forka kompilatora; dlatego domyślne są warianty
  RISC-V (C6/C3/H2).

**Nieprzenośne z premedytacją:** format obrazu i bootloader (stąd `platform`
w manifeście OTA), mapa flasha, budżet energetyczny.

**Do rozstrzygnięcia:** czy STM32 wchodzi do macierzy od początku, czy dopiero
gdy pojawi się produkt tego wymagający. Utrzymywanie portu bez płytki na HIL
to deklaracja przenośności, nie przenośność.

---

# Pytania otwarte

Do rozstrzygnięcia zanim ruszy kod. Uszeregowane wpływem na architekturę.

### 1. Skala i model biznesowy
- Rząd wielkości floty docelowo: setki, tysiące, setki tysięcy urządzeń?
- Produkt własny czy platforma dla wielu klientów (czy multi-tenancy jest
  realnie potrzebne od początku)?
- B2B (instalacje, gateway w cenie) czy B2C (Wi-Fi, bez gatewaya)?

### 2. Chmura: self-hosted czy zarządzana
- Własny VPS/kolokacja vs AWS/GCP/Hetzner? Wpływa na wybór między
  self-hosted NATS/Postgres a usługami zarządzanymi.
- Czy są wymagania co do lokalizacji danych (RODO, dane w PL/EU)?

### 3. Sprzęt
- Czy jest już wybrana rodzina MCU, czy projektujemy od zera?
- Które rodziny wchodzą do macierzy przenośności **od początku** (ADR-011)?
  Każda dodana platforma to job w CI i płytka na stanowisku HIL — koszt stały,
  nie jednorazowy.
- Czy któryś produkt wymaga ESP32-S3 (Xtensa, fork toolchaina), czy wystarczą
  warianty RISC-V?
- Czy któryś produkt jest bateryjny na tyle długo (2+ lata), że budżet
  energetyczny wymusza architekturę single-chip zamiast RCP (ADR-004)?
- Czy przewidujemy secure element, czy tożsamość w flashu MCU?

### 4. Certyfikacja i zgodność
- Czy planujemy certyfikację Thread / Zigbee / Matter? Matter zmieniłby
  znacząco warstwę aplikacyjną (model klastrów zamiast własnego shadow).
- CE/RED, cyberbezpieczeństwo (EN 18031 / Cyber Resilience Act) — CRA wymaga
  m.in. secure boot, aktualizacji i SBOM. Warto zaprojektować od razu.

### 5. Zakres pierwszego produktu
- Jaki jest pierwszy realny przypadek użycia? Bez niego etap 1 roadmapy
  nie ma konkretnego celu.

### 6. Zespół
- Ile osób, jakie doświadczenie w Rust i w embedded? Determinuje, czy
  RCP (prościej, drożej w BOM) czy single-chip FFI (taniej, trudniej).
