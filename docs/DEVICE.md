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
- budżet energetyczny jest wymaganiem funkcjonalnym (patrz sekcja 8).

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
| pkpu-platform-{nrf,stm32,esp}   impl traitów pkpu-hal       |
+-----------------------------------------------------------+
| boards/<płytka>         BSP: pinout, zegary, partycje       |
+-----------------------------------------------------------+
| embassy + HAL producenta (embassy-nrf/-stm32 / esp-hal)     |
+-----------------------------------------------------------+
| bootloader (A/B, weryfikacja podpisu, rollback)             |
+-----------------------------------------------------------+
```

Reguła: `pkpu-device-core` **nie zna** technologii radiowej ani sprzętu.
Zna wyłącznie traity. Dzięki temu ten sam core kompiluje się do testów na hoście
z `Link` opartym o kanał in-memory — i, z tego samego powodu, na STM32, ESP32
i nRF bez zmiany linii kodu (patrz sekcja 4).

Poziomo (radio) i pionowo (krzem) rozdziela dwie **niezależne** osie zmienności:
`pkpu-link` zmienia się wraz ze stosem radiowym, `pkpu-platform-*` wraz z MCU.
Produkt wybiera po jednym z każdej osi.

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

## 4. Przenośność między platformami MCU

**Założenie bazowe:** firmware SDK ma jedną **wspólną część, przenośną między
rodzinami MCU**. Ten sam `pkpu-device-core` i ta sama logika produktowa
kompilują się bez zmian na STM32, ESP32 i nRF. Wybór platformy to wybór BSP
i cech kompilacji (`features`), nigdy rozgałęzienie kodu produktowego.

Konsekwencja praktyczna: nowy produkt na innym MCU (bo taki był dostępny,
tańszy albo ma potrzebne peryferium) nie jest nowym firmware — jest nową
płytką i nowym `product.toml`.

### 4.1 Co jest przenośne, a co nie

| Warstwa | Przenośna | Uwaga |
|---|---|---|
| `apps/<produkt>` | tak | zależy wyłącznie od traitów z `pkpu-hal` i `pkpu-link` |
| `pkpu-device-core` | tak, w 100% | zero `cfg(target_arch)`, zero zależności od HAL-i producenta |
| `pkpu-link` | API tak, impl per **stos radiowy** | `thread` przez Spinel działa na każdym MCU z co-procesorem |
| `pkpu-hal` | tak (same traity) | definicja kontraktu, bez implementacji |
| `pkpu-platform-{nrf,stm32,esp}` | **nie** — z definicji | tu mieszka cała wiedza o krzemie |
| `boards/<płytka>` | **nie** | pinout, zegary, mapa flasha, regulatory |
| bootloader | **nie** | inny mechanizm i format obrazu per rodzina (4.5) |

**Reguła twarda:** `#[cfg(target_arch)]`, `#[cfg(target_os)]` i cechy nazwane od
producenta są dozwolone **wyłącznie** w `pkpu-platform-*` i `boards/`.
Pojawienie się takiego `cfg` w core lub w `apps/` jest sygnałem, że w `pkpu-hal`
brakuje traitu — poprawką jest dodanie traitu, nie dodanie gałęzi.

### 4.2 Macierz targetów

| Rodzina | Przykładowy MCU | Target Rust | HAL | Toolchain |
|---|---|---|---|---|
| nRF52 | nRF52840 | `thumbv7em-none-eabihf` | `embassy-nrf` | stable |
| nRF53 | nRF5340 | `thumbv8m.main-none-eabihf` | `embassy-nrf` | stable |
| STM32 (M4) | STM32WB55, L4 | `thumbv7em-none-eabihf` | `embassy-stm32` | stable |
| STM32 (M33) | STM32U5, WBA | `thumbv8m.main-none-eabihf` | `embassy-stm32` | stable |
| ESP32 RISC-V | ESP32-C6, H2 | `riscv32imac-unknown-none-elf` | `esp-hal` | stable |
| ESP32 RISC-V | ESP32-C3 | `riscv32imc-unknown-none-elf` | `esp-hal` | stable |
| ESP32 Xtensa | ESP32-S3 | `xtensa-esp32s3-none-elf` | `esp-hal` | **fork esp-rs** (`espup`) |
| Host (testy) | — | natywny | mock/sim | stable |

Xtensa wymaga forka kompilatora — to jedyna pozycja w macierzy psująca „jeden
`rustup`, jedno CI". Dlatego domyślnie celujemy w warianty **RISC-V** ESP32
(C6/C3/H2); S3 dopiero gdy produkt wymaga jego mocy lub PSRAM.

### 4.3 Wspólny mianownik

Przenośność nie bierze się z dyscypliny, tylko z tego, że wszystkie trzy rodziny
mają wspólny zestaw abstrakcji:

| Warstwa wspólna | Rola | Co się zmienia per platforma |
|---|---|---|
| `embassy-executor` | model async, taski | nic |
| `embassy-time` | timery, `Timer::after` | tylko driver czasu w BSP |
| `embedded-hal` 1.0 / `-async` | SPI, I²C, GPIO | implementacja w HAL producenta |
| `embedded-io-async` | UART, TCP, kanał do RCP/NCP | implementacja w HAL producenta |
| `embedded-storage-async` | flash | geometria i sterownik |
| `critical-section` | sekcje krytyczne | impl dostarcza BSP |
| `defmt` + RTT | logi | nic (poza transportem) |

Cena tego wyboru: jesteśmy związani z ekosystemem embassy. MCU bez wsparcia
embassy/`embedded-hal` wypada z macierzy albo wymaga napisania HAL-a — to
kryterium wyboru krzemu, nie detal implementacyjny.

### 4.4 Kontrakt portu

Port na nową platformę = dostarczenie implementacji poniższego zestawu.
Nic ponadto; jeżeli potrzeba czegoś więcej, kontrakt jest niekompletny.

```rust
/// Wszystko, czego pkpu-device-core wymaga od krzemu.
/// Implementowane w pkpu-platform-*, składane w boards/.
pub trait Platform {
    type Rng:      CryptoRng;              // sprzętowy TRNG
    type Flash:    NorFlash;               // partycja storage (embedded-storage-async)
    type Clock:    Clock;                  // monotonic + RTC wall-clock + budzenie
    type Identity: IdentityStore;          // odczyt device_id/klucza, podpis (SE lub OTP)
    type Reset:    ResetController;        // reboot, przyczyna resetu, watchdog
    type Ota:      OtaSlots;               // adresy slotów A/B, mark_boot_ok, rollback
    type Power:    PowerControl;           // wejście w tryb niskiego poboru, rails
}
```

Checklista portu:

| Krok | Efekt |
|---|---|
| 1. `pkpu-platform-<x>`: implementacja traitów `Platform` | core się linkuje |
| 2. `memory.x` / partycje + integracja bootloadera | obraz się bootuje |
| 3. driver `embassy-time` (zwykle gotowy w HAL producenta) | timery działają |
| 4. `Link` dla radia dostępnego na tej platformie | urządzenie gada z chmurą |
| 5. job w macierzy CI + jedna płytka na stanowisku HIL | port nie gnije |

### 4.5 Gdzie przenośność realnie boli

Różnice, których **nie** da się schować za traitem — trzeba je zaprojektować
świadomie:

| Obszar | nRF | STM32 | ESP32 |
|---|---|---|---|
| Flash | wewnętrzny, jednorodne strony | wewnętrzny, sektory nierówne, dual-bank w U5/H7 | **zewnętrzny QSPI** + tabela partycji, zapis wymaga obsługi cache |
| Bootloader A/B | `embassy-boot-nrf` | `embassy-boot-stm32` | własny bootloader ESP / MCUboot — **inny format obrazu** |
| Krypto / klucz | CryptoCell, KMU | RNG + PKA (część rodzin) | RNG, peryferium HMAC/DS, eFuse + flash encryption |
| Wi-Fi | brak (co-procesor) | brak (co-procesor) | natywne (`esp-wifi`) |
| 802.15.4 / BLE | natywne | WB/WBA | C6/H2 |
| Deep sleep | jednostki µA | jednostki µA (Stop2/Standby) | wyraźnie wyżej — bywa dyskwalifikujące dla `SLEEP` |

Liczby poboru traktujemy jako rząd wielkości do potwierdzenia pomiarem
(patrz sekcja 8) — nie jako dane wejściowe do projektu.

Dwa wnioski wiążące dla reszty systemu:

1. **Manifest OTA musi nieść `platform`**, nie tylko `model` i `hw_rev` —
   obrazy nie są wymienne między rodzinami. Patrz sekcja 9.
2. **Budżet energetyczny nie jest przenośny.** Ten sam kod na innym MCU to inny
   czas życia baterii. Wybór platformy dla produktu `SLEEP` jest decyzją
   sprzętową, nie kosmetyczną.

### 4.6 Weryfikacja przenośności

Przenośność deklarowana i nietestowana rozpada się w pierwszym sprincie.
Dlatego jest wymuszana mechanicznie:

- każdy PR buduje `pkpu-device-core` i przykładowy `app` **na wszystkich**
  targetach z macierzy 4.2 — build failure na dowolnym z nich blokuje merge,
- `cargo test` core'a na hoście (`Link` in-memory, czas symulowany) — to jest
  główna siatka bezpieczeństwa, patrz sekcja 10,
- lint CI: `cfg(target_arch|target_os)` poza `pkpu-platform-*` i `boards/`
  to błąd,
- raport rozmiaru obrazu per target, próg regresji — przenośność nie może
  oznaczać puchnięcia flasha na najmniejszej platformie,
- HIL nocny: co najmniej jedna płytka z **każdej** rodziny w macierzy.

Minimalny sensowny zakres na start: dwie rodziny (ESP32-C6 dla Wi-Fi,
nRF52840 dla 802.15.4/BLE). Trzecia platforma dodana jako trzecia, a nie
przewidziana z góry, jest właściwym testem tego, czy abstrakcja jest realna —
tak samo jak przy `Link` (patrz ADR-010, ADR-011).

---

## 5. Maszyna stanów urządzenia

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

## 6. Provisioning

### 6.1 Sekwencja BLE

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

### 6.2 Sekwencja NFC

- Tag NTAG z interfejsem I²C do MCU (nie sam pasywny tag) — pozwala na
  dwukierunkową wymianę bez zasilania radia głównego.
- NDEF zawiera: `device_id`, `serial`, `model`, URL deep-link do aplikacji.
- Telefon czyta tag -> otwiera aplikację -> dalej ścieżka jak w BLE od kroku 3,
  z tym że creds trafiają do MCU przez pamięć tagu.
- Wariant „out-of-box": tag zapisany fabrycznie, tylko do identyfikacji;
  transfer creds i tak przez BLE.

### 6.3 Provisioning sieci PAN

| COM_TYPE | Co trafia do urządzenia |
|---|---|
| `WIFI` | SSID + PSK (lub WPA3 SAE), endpoint MQTT, CA chmury |
| `OT_THREAD` | Thread Operational Dataset (network key, PAN ID, channel) |
| `ZIGBEE` | tryb parowania (Install Code preferowany nad well-known key) |
| `BLE` | brak — łączność jest samym GATT |

---

## 7. Trwały stan i buforowanie

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

## 8. Budżet energetyczny (SLEEP_TYPE = SLEEP)

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

## 9. OTA i bootloader

- Układ flash: `bootloader | slot A | slot B | storage | identity(OTP)`.
- Manifest podpisany ed25519, zawiera: `model`, `platform`, `hw_rev_min/max`,
  `version`, `size`, `sha256`, `min_battery_pct`, `requires_reboot`.
  `platform` jest obowiązkowe i weryfikowane przed zapisem do slotu — rdzeń jest
  przenośny, obraz binarny nie jest (patrz sekcja 4.5).
- Weryfikacja podpisu **w bootloaderze**, klucz publiczny w regionie chronionym.
- Rollback: licznik prób bootu; aplikacja musi wywołać `mark_boot_ok()` po
  udanym połączeniu z chmurą, inaczej powrót do poprzedniego slotu.
- Urządzenia `SLEEP`: OTA tylko przy `battery > min_battery_pct` i w oknie
  serwisowym; transfer wznawialny (blokowy, z offsetem).
- Delta OTA — opcjonalne, dopiero gdy rozmiar obrazu to realny problem.

---

## 10. Testowalność

| Poziom | Jak |
|---|---|
| Jednostkowe | `pkpu-device-core` kompilowane na host, `Link` in-memory, czas symulowany |
| Przenośności | build całej macierzy targetów (sekcja 4.2) na każdym PR, raport rozmiaru obrazu |
| Integracyjne | QEMU / `embassy` sim + fake broker MQTT |
| HIL | probe-rs + `defmt` po RTT, płytka **z każdej rodziny MCU** na stanowisku, pomiar prądu (Otii/PPK2) |
| Flotowe | staging tenant, kohorta „canary" 1% przed każdym rolloutem |

`defmt` zamiast `log` — logi kompaktowe, dekodowane po stronie hosta,
nie zjadają flasha ani czasu.

Pełna strategia — scenariusze obowiązkowe, zestaw conformance dla platform,
testy energetyczne jako testy regresyjne i bramki blokujące merge — w
[TESTING.md](TESTING.md).

---

## 11. Profil produktu

Każdy produkt (`apps/<nazwa>/product.toml`) deklaruje:

```toml
model      = "PKPU-TH-01"
hw_rev     = "B"
platform   = "nrf52840"        # wybór krzemu; rdzeń SDK identyczny dla stm32/esp32
board      = "th01-rev-b"
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
