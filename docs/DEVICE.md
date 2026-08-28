# 1. DEVICE — stack urządzenia

Dokument opisuje warstwę firmware. Klasyfikacja urządzenia to iloczyn czterech
wymiarów: **SLEEP_TYPE × COM_TYPE × PROV_TYPE × STATE**. Te same enumy żyją
w `pkpu-proto` i w bazie danych — patrz [PROTOCOL.md](PROTOCOL.md) i
[DATA.md](DATA.md).

---

## 1. Taksonomia

### SLEEP_TYPES

| Wariant | Zasilanie | Radio | Latencja komendy | Rola w mesh |
|---|---|---|---|---|
| `NONSLEEP` | sieć / stałe | RX zawsze włączony | < 1 s | router / FFD |
| `SLEEP` | bateria | RX cyklicznie (poll / interval) | do 1 cyklu wybudzenia | end device / SED |

Konsekwencje projektowe `SLEEP`:
- komendy **kolejkowane** po stronie parenta lub chmury, nie push,
- telemetria wysyłana paczkami, nie strumieniowo,
- brak ciągłego TLS — sesja odtwarzana z session ticket / PSK,
- budżet energetyczny jest wymaganiem funkcjonalnym (patrz sekcja 7).

### COM_TYPES

| Wariant | Stos | Ścieżka do chmury | Uwagi |
|---|---|---|---|
| `WIFI` | IP + TLS + MQTT5 | bezpośrednio | najwyższy pobór, najprostsza topologia |
| `OT_THREAD` | 802.15.4 + 6LoWPAN + IPv6 | przez Border Router | natywne IP w mesh, CoAP/DTLS |
| `ZIGBEE` | 802.15.4 + ZCL | przez koordynator | model klastrowy, nie-IP |
| `BLUETOOTH/BLE` | GATT | przez telefon lub gateway | również kanał provisioningu |

### PROV_TYPES

| Wariant | Nośnik | Kiedy |
|---|---|---|
| `BLE` | GATT service, ephemeral advertising | urządzenie z radiem BLE, onboarding z telefonu |
| `NFC` | NDEF na tagu / NTAG z I²C do MCU | urządzenie bez UI, tap-to-provision, także out-of-box |

### STATES

Stan raportowany do chmury (`device.state`):

| Stan | Znaczenie | Kto ustawia |
|---|---|---|
| `ONLINE` | sesja aktywna, urządzenie odpowiada | ingest przy connect / keepalive |
| `DISCONNECTED` | brak sesji poza oczekiwanym oknem | watchdog presence w chmurze |
| `SLEEPS` | brak sesji, ale **zgodnie z harmonogramem** | registry, na podstawie `SLEEP_TYPE` |

`SLEEPS` vs `DISCONNECTED` rozróżnia się przez okno tolerancji:
`last_seen + expected_interval × tolerance` (domyślnie ×3). Bez tego rozróżnienia
urządzenia bateryjne generują fałszywe alerty.

---

## 2. Tożsamość urządzenia

```rust
pub struct DeviceIdentity {
    pub device_id:  DeviceId,      // UUIDv7, 128-bit, nadany przy produkcji
    pub short_id:   u32,           // skrót do adresacji w PAN, nadany przez chmurę
    pub serial:     Serial,        // numer fabryczny, drukowany/QR/NFC
    pub model:      ModelId,       // typ produktu
    pub hw_rev:     HwRev,
    pub sleep_type: SleepType,
    pub com_type:   ComType,
    pub prov_type:  ProvType,
    pub pubkey:     [u8; 32],      // ed25519, klucz prywatny nigdy nie opuszcza chipa
}
```

- `device_id` jest **niezmienne** przez całe życie urządzenia (także po factory
  reset i po zmianie właściciela).
- `short_id` służy tylko oszczędności bajtów w ramkach radiowych; mapowanie
  `short_id -> device_id` trzyma gateway i chmura.
- Zapis: strefa OTP / chroniony region flash, chroniona przed masowaniem przy OTA.

---

## 3. Warstwy firmware

```
+-----------------------------------------------------------+
| apps/<produkt>          logika produktowa, konfiguracja     |
+-----------------------------------------------------------+
| pkpu-device-core        maszyna stanów, scheduler, shadow,  |
|                         bufor telemetrii, klient OTA        |
+---------------------------+-------------------------------+
| pkpu-link (trait Link)    | pkpu-hal (traity sprzętowe)     |
|  wifi | thread | zigbee   |  Sensor, Actuator, PowerRail,   |
|  | ble                    |  Rng, Clock, Storage            |
+---------------------------+-------------------------------+
| boards/<płytka>         BSP: pinout, zegary, partycje       |
+-----------------------------------------------------------+
| embassy + HAL producenta (embassy-nrf / esp-hal / ...)      |
+-----------------------------------------------------------+
| bootloader (A/B, weryfikacja podpisu, rollback)             |
+-----------------------------------------------------------+
```

Reguła: `pkpu-device-core` **nie zna** technologii radiowej ani sprzętu.
Zna wyłącznie traity. Dzięki temu ten sam core kompiluje się do testów na hoście
z `Link` opartym o kanał in-memory.

### Trait `Link`

```rust
pub trait Link {
    type Error;

    /// Nawiąż łączność (join sieci, DHCP, handshake, attach do parenta).
    async fn connect(&mut self, creds: &NetworkCreds) -> Result<(), Self::Error>;

    /// Wyślij ramkę aplikacyjną. Implementacja decyduje o fragmentacji.
    async fn send(&mut self, frame: &Frame<'_>) -> Result<(), Self::Error>;

    /// Odbierz ramkę. Dla SLEEP: poll parenta; dla NONSLEEP: nasłuch.
    async fn recv<'a>(&mut self, buf: &'a mut [u8]) -> Result<Frame<'a>, Self::Error>;

    /// Charakterystyka łącza — core dostraja rozmiar paczek i częstotliwość.
    fn profile(&self) -> LinkProfile;   // mtu, rtt_typ, czy push-capable, koszt energ.
}
```

`LinkProfile` jest tym, co pozwala jednemu core'owi zachowywać się rozsądnie na
łączu 1500 B/Wi-Fi i na łączu ~80 B/Zigbee.

---

## 4. Maszyna stanów urządzenia

```
        power-on
           |
           v
     [SELF_TEST] --fail--> [FAULT] --(watchdog)--> reset
           |
           v
    creds w flash?  --nie--> [PROVISIONING] --ok--> zapis creds
           | tak                  ^                      |
           v                      |                      |
      [CONNECTING] --timeout/backoff--                    |
           |  ok                  ^                      |
           v                      |                      |
      [OPERATIONAL] <-------------+----------------------+
        |   |   |
        |   |   +--> [OTA]        (pobieranie, weryfikacja, reboot)
        |   +------> [LOW_POWER]  (tylko SLEEP_TYPE=SLEEP)
        +----------> [FACTORY_RESET] --> czyszczenie creds, zachowanie identity
```

- Backoff w `CONNECTING`: exponential z jitterem, cap 15 min. Bez jittera flota
  po awarii chmury tworzy thundering herd.
- `FAULT` nie oznacza pętli resetów: po N nieudanych bootach bootloader robi
  rollback do poprzedniego slotu.
- Reset do ustawień fabrycznych **kasuje** poświadczenia sieci i przypisanie do
  właściciela, **zachowuje** `DeviceIdentity`.

---

## 5. Provisioning

### 5.1 Sekwencja BLE

```
1. Urządzenie bez creds -> advertising `PKPU-PROV`,
   payload: model, serial (skrócony), nonce.
2. Aplikacja mobilna -> connect, GATT service 0xPKPU:
     char PROV_INFO   (read)   identity + capabilities
     char PROV_CHAL   (read)   challenge z urządzenia
     char PROV_RESP   (write)  odpowiedź chmury (podpis) + creds
     char PROV_STATE  (notify) postęp / błąd
3. Aplikacja przekazuje challenge do chmury.
   Chmura sprawdza device_id w rejestrze fabrycznym i podpisuje token claimowania.
4. Aplikacja zapisuje do PROV_RESP: {claim_token, network_creds, mqtt_endpoint}.
   Urządzenie weryfikuje podpis chmury -> dopiero wtedy przyjmuje creds.
5. Urządzenie -> CONNECTING -> pierwszy raport -> chmura zamyka claim.
```

Kluczowe: **urządzenie weryfikuje chmurę, nie tylko chmura urządzenie.**
Bez kroku 4 telefon w zasięgu może wstrzyknąć dowolne creds.

### 5.2 Sekwencja NFC

- Tag NTAG z interfejsem I²C do MCU (nie sam pasywny tag) — pozwala na
  dwukierunkową wymianę bez zasilania radia głównego.
- NDEF zawiera: `device_id`, `serial`, `model`, URL deep-link do aplikacji.
- Telefon czyta tag -> otwiera aplikację -> dalej ścieżka jak w BLE od kroku 3,
  z tym że creds trafiają do MCU przez pamięć tagu.
- Wariant „out-of-box": tag zapisany fabrycznie, tylko do identyfikacji;
  transfer creds i tak przez BLE.

### 5.3 Provisioning sieci PAN

| COM_TYPE | Co trafia do urządzenia |
|---|---|
| `WIFI` | SSID + PSK (lub WPA3 SAE), endpoint MQTT, CA chmury |
| `OT_THREAD` | Thread Operational Dataset (network key, PAN ID, channel) |
| `ZIGBEE` | tryb parowania (Install Code preferowany nad well-known key) |
| `BLE` | brak — łączność jest samym GATT |

---

## 6. Trwały stan i buforowanie

- `sequential-storage` na dedykowanej partycji flash, klucze:
  `identity`, `net_creds`, `shadow_reported`, `ota_state`, `tel_buffer`.
- Bufor telemetrii: ring buffer, wpisy `postcard`, ze znacznikiem czasu.
  Przy utracie łączności zapis do flash; przy odzyskaniu — wysyłka wsadowa
  z flagą `backfill = true`, aby chmura nie interpretowała ich jako live.
- Budżet zapisów: flash NOR ~100k cykli. Nie zapisujemy telemetrii do flash
  dopóki bufor RAM się mieści — dopiero przy przepełnieniu.
- Zegar: RTC + synchronizacja czasu przy każdym połączeniu. Urządzenie bez
  zsynchronizowanego czasu wysyła `uptime_ms` zamiast timestampu, a chmura
  rekonstruuje czas — nigdy nie wysyłamy zmyślonego wall-clocku.

---

## 7. Budżet energetyczny (SLEEP_TYPE = SLEEP)

Wymaganie zapisywane w profilu produktu, weryfikowane pomiarem:

```
target: CR2032 220 mAh -> 24 miesiące
budżet dzienny: 220 mAh / 730 dni = ~0.30 mAh/dzień = ~12.5 uA średnio
podział: sleep 4 uA | pomiar 2 uA | radio TX/RX 5 uA | rezerwa 1.5 uA
```

Konsekwencje dla firmware:
- brak busy-wait, wszystko na `embassy` timerach i przerwaniach,
- radio włączane tylko na okno TX + oczekiwaną odpowiedź,
- agregacja: N pomiarów -> 1 transmisja,
- transmisja zdarzeniowa (zmiana > próg) zamiast stałego interwału,
  z heartbeatem raz na `max_silence`.

---

## 8. OTA i bootloader

- Układ flash: `bootloader | slot A | slot B | storage | identity(OTP)`.
- Manifest podpisany ed25519, zawiera: `model`, `hw_rev_min/max`, `version`,
  `size`, `sha256`, `min_battery_pct`, `requires_reboot`.
- Weryfikacja podpisu **w bootloaderze**, klucz publiczny w regionie chronionym.
- Rollback: licznik prób bootu; aplikacja musi wywołać `mark_boot_ok()` po
  udanym połączeniu z chmurą, inaczej powrót do poprzedniego slotu.
- Urządzenia `SLEEP`: OTA tylko przy `battery > min_battery_pct` i w oknie
  serwisowym; transfer wznawialny (blokowy, z offsetem).
- Delta OTA — opcjonalne, dopiero gdy rozmiar obrazu to realny problem.

---

## 9. Testowalność

| Poziom | Jak |
|---|---|
| Jednostkowe | `pkpu-device-core` kompilowane na host, `Link` in-memory, czas symulowany |
| Integracyjne | QEMU / `embassy` sim + fake broker MQTT |
| HIL | probe-rs + `defmt` po RTT, płytka na stanowisku, pomiar prądu (Otii/PPK2) |
| Flotowe | staging tenant, kohorta „canary" 1% przed każdym rolloutem |

`defmt` zamiast `log` — logi kompaktowe, dekodowane po stronie hosta,
nie zjadają flasha ani czasu.

---

## 10. Profil produktu

Każdy produkt (`apps/<nazwa>/product.toml`) deklaruje:

```toml
model      = "PKPU-TH-01"
hw_rev     = "B"
sleep_type = "SLEEP"
com_type   = "OT_THREAD"
prov_type  = ["BLE", "NFC"]

[telemetry]
interval_s       = 600
max_silence_s    = 3600
buffer_entries   = 4096

[power]
battery         = "CR2032"
target_months   = 24
avg_current_ua  = 12.5

[ota]
min_battery_pct = 30
window          = "02:00-05:00"
```

Ten plik jest źródłem prawdy dla firmware **i** dla rekordu w rejestrze chmury —
generowany z niego jest wpis `device_type`. Patrz [DATA.md](DATA.md).
