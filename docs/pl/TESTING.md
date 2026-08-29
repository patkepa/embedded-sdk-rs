# TESTING — strategia i poziomy testów

Dokument opisuje, **co** testujemy, **gdzie** to się wykonuje i **co blokuje
merge**. Uruchamianie tych testów opisuje [CI.md](CI.md); zasady rozwoju samego
SDK — [SDK.md](SDK.md).

---

## 1. Zasady

1. **Domyślnym miejscem testu jest host, nie płytka.** Sprzęt jest wolny,
   niedeterministyczny i jest go mało. Na sprzęt trafia wyłącznie to, czego
   fizycznie nie da się sprawdzić inaczej (sekcja 5).
2. **Czas do sygnału zwrotnego jest wymaganiem.** Testy jednostkowe firmware:
   sekundy. Pełny PR: minuty. HIL: nocą. Test, na który nikt nie czeka,
   przestaje być uruchamiany świadomie.
3. **Testy są deterministyczne albo ich nie ma.** Czas symulowany, nie `sleep`.
   Losowość z zasianego RNG. Test flaky trafia do kwarantanny na 24 h i jest
   naprawiany albo usuwany — nigdy „retry i jedziemy".
4. **Nie ścigamy procentu pokrycia.** Ścigamy listę scenariuszy obowiązkowych
   (sekcje 3–11). Pokrycie jest raportowane jako informacja, nie jako bramka.
5. **Testujemy nasz kod, nie cudzy.** Nie testujemy HAL-a producenta, stosu
   Thread w NCP ani systemu telefonu — testujemy nasze użycie ich i nasze
   zachowanie, gdy zawiodą.

---

## 2. Mapa testów

| Poziom | Co obejmuje | Gdzie się wykonuje | Kiedy |
|---|---|---|---|
| Kontrakt | `pkpu-proto`, zgodność wsteczna ramek, schematy | host | każdy PR |
| Jednostkowe firmware | `pkpu-device-core`, `pkpu-link`, logika `apps/` | host + mock platform | każdy PR |
| Przenośności | build macierzy targetów, rozmiar obrazu, lint `cfg` | host (cross-build) | każdy PR |
| Conformance platformy | zestaw, który musi przejść każda impl. `Platform`/`Link` | host (mock) + HIL | PR + nocne |
| Jednostkowe chmura | logika serwisów, reguły, reconcile, presence | host | każdy PR |
| Integracyjne chmura | Postgres/Timescale, NATS, MinIO na kontenerach | host (testcontainers) | każdy PR |
| Mobile core | maszyna provisioningu, cache, klient API | host + fake BLE/NFC | każdy PR |
| E2E wirtualne | pełny przepływ z symulatorem urządzenia | host (compose) | main + nocne |
| HIL | flash, radio, pobór prądu, OTA z bootloaderem | stanowisko z płytkami | nocne + przed release |
| Flotowe | staging tenant, kohorta canary, soak | staging / produkcja | przed każdym rolloutem |

---

## 3. Testy kontraktu (`pkpu-proto`)

Najtańsza i najwyżej zwrotna klasa testów w projekcie — jedna ramka zdekodowana
inaczej po dwóch stronach kosztuje więcej niż cała reszta pakietu.

- **Round-trip**: `encode -> decode == oryginał` dla każdego typu, property-based
  (`proptest`) na losowych wartościach, nie na trzech ręcznie dobranych.
- **Wektory złote**: katalog `proto/tests/vectors/` z bajtami ramek
  wygenerowanymi przez **poprzednie** wersje kontraktu. Nowy kod musi je
  dekodować. Plik wektorów wolno dopisywać, nigdy modyfikować — modyfikacja jest
  świadomym złamaniem zgodności i wymaga bumpa `v` w `Envelope`.
- **Zgodność enumów z bazą**: test generuje warianty z Rusta i porównuje
  z `CREATE TYPE` w migracjach ([DATA.md](DATA.md) sekcja 2). Rozjazd
  `ComType::Zigbee = 2` z enumem PG ma być błędem CI, nie incydentem
  produkcyjnym.
- **Snapshot schematów**: wygenerowane JSON Schema i OpenAPI leżą w repo. PR,
  który je zmienia, pokazuje diff w review — zmiana API jest widoczna, a nie
  odkrywana przez klienta.
- **Fuzzing dekodera** (`cargo-fuzz`): dekoder `Envelope` przetwarza
  **niezaufane dane z radia**. Wymóg: żadne wejście nie powoduje paniki,
  nieskończonej pętli ani odczytu poza buforem. Korpus trzymany między
  przebiegami, każdy crash ląduje w repo jako test regresyjny.

---

## 4. Firmware — testy na hoście

Fundament: `pkpu-device-core` nie zna sprzętu ([DEVICE.md](DEVICE.md) sekcja 3),
więc kompiluje się na host z zestawem atrap.

| Atrapa | Zastępuje | Co daje |
|---|---|---|
| `pkpu-platform-mock` | impl `Platform` | flash w RAM z symulacją zużycia i błędów zapisu, RNG deterministyczny, sterowalny zegar |
| `MockLink` | radio | kanał in-memory z konfigurowalnym MTU, RTT, stratą pakietów, rozłączeniami |
| czas symulowany | `embassy-time` | godziny pracy urządzenia w milisekundach testu |

Scenariusze obowiązkowe:

- **Cykl życia telemetrii**: pomiar → bufor RAM → przepełnienie → flash → utrata
  łączności → powrót → wysyłka wsadowa z `backfill = true`. Brak duplikatów,
  brak luk w `seq`, brak zapisu do flasha dopóki mieści się RAM.
- **Przerwanie zasilania w każdym kroku OTA**: test parametryzowany punktem
  przerwania (pobieranie, weryfikacja, zapis slotu, przełączenie, pierwszy boot).
  Oczekiwane zawsze to samo: urządzenie bootuje się do **działającego** obrazu.
- **Rollback**: brak `mark_boot_ok()` przez N bootów → powrót do poprzedniego
  slotu, `DeviceIdentity` nienaruszone.
- **Idempotencja komend**: ta sama `command_id` dostarczona dwa razy → jedno
  wykonanie, dwa ACK. Komenda po TTL → odrzucona, nie wykonana z opóźnieniem.
- **Backoff**: 10 000 symulowanych urządzeń startujących jednocześnie po awarii
  chmury — rozkład ponowień musi być rozłożony w czasie. Thundering herd nie
  objawia się przy jednym urządzeniu, więc test musi być statystyczny.
- **Provisioning**: pełna sekwencja z [DEVICE.md](DEVICE.md) 6.1 wraz ze
  ścieżkami błędnymi — zły podpis chmury (creds **odrzucone**), timeout,
  urządzenie już claimowane, reset w środku sekwencji.
- **Factory reset**: kasuje creds i przypisanie, zachowuje tożsamość.
- **Degradacja łącza**: profil MTU 80 B / RTT 2 s (Zigbee) — core fragmentuje
  i zwalnia zamiast zapychać kolejkę; profil 1500 B (Wi-Fi) — nie marnuje okien.

Panika w firmware jest awarią, nie wyjątkiem: pakiet uruchamiany jest także
w trybie, w którym `panic` = fail testu, a publiczne API SDK nie używa
`unwrap()` w ścieżce runtime (patrz [SDK.md](SDK.md) sekcja 3).

---

## 5. Firmware — testy na sprzęcie (HIL)

Stanowisko: probe-rs + `defmt` po RTT, sterowany zasilacz (wymuszanie
brownoutu), analizator prądu (PPK2 / Otii), po jednej płytce z **każdej**
rodziny MCU z macierzy ([DEVICE.md](DEVICE.md) 4.2).

| Test | Dlaczego wyłącznie na sprzęcie |
|---|---|
| Cykl życia flasha | realna geometria sektorów, czasy kasowania, zanik zasilania w trakcie zapisu |
| OTA end-to-end | realny bootloader, realna weryfikacja podpisu, realne przełączenie slotów |
| Wymuszony rollback | brownout w oknie bootu — nie da się wiarygodnie zasymulować |
| Pobór prądu | sekcja 7 |
| Radio | RSSI/LQI, join do sieci, zachowanie na krawędzi zasięgu, rekonekcja po utracie parenta |
| Watchdog i przyczyna resetu | peryferium, nie logika |
| Dryf RTC i czas budzenia | zależne od zegara, temperatury i egzemplarza |

Testy HIL są pisane jako testy (`embedded-test` / `defmt-test`) uruchamiane
z CI, nie jako procedura dla operatora — inaczej nie przeżyją pierwszego
miesiąca.

---

## 6. Przenośność i conformance

Dwa różne pytania, dwa różne mechanizmy.

**Czy się buduje** — macierz targetów z [DEVICE.md](DEVICE.md) 4.2 na każdym PR,
lint zakazujący `cfg(target_arch|target_os)` poza `pkpu-platform-*` i `boards/`,
raport rozmiaru obrazu z progiem regresji.

**Czy zachowuje się tak samo** — **zestaw conformance**: jeden zbiór testów
parametryzowany implementacją, uruchamiany na mocku (PR) i na każdej realnej
platformie (HIL). To kontrakt `Platform` wyrażony wykonywalnie:

```
conformance::flash     zapis/odczyt/kasowanie, granice sektorów, stan po resecie
conformance::clock     monotoniczność, budzenie w tolerancji, wall-clock po synchronizacji
conformance::rng       brak powtórzeń, testy statystyczne na próbce
conformance::identity  odczyt device_id, podpis weryfikowalny kluczem publicznym
conformance::ota       zapis slotu, mark_boot_ok, rollback, odrzucenie obcego `platform`
conformance::reset     przyczyna resetu rozpoznana dla każdego źródła
conformance::power     wejście/wyjście z trybu niskiego poboru, retencja RAM
conformance::link      connect/send/recv, zgodność deklarowanego LinkProfile z pomiarem
```

Port na nową platformę jest **ukończony, gdy przechodzi conformance na
sprzęcie** — nie gdy się kompiluje. To jedyna definicja, która nie rozjeżdża się
z rzeczywistością, i to ona zamienia przenośność z deklaracji w cechę.

---

## 7. Testy energetyczne jako testy regresyjne

Budżet z `product.toml` ([DEVICE.md](DEVICE.md) sekcja 11) jest asercją, nie
komentarzem:

```
scenariusz: cykl pomiarowy × N, powtórzony, z realnym radiem
mierzone:   ładunek [uC] na cykl, prąd w sleep, czas okna radiowego
asercja:    prąd średni <= avg_current_ua z product.toml + 10% marginesu
raport:     trend per commit; wzrost > 5% wymaga uzasadnienia w PR
```

Bez tego regresja energetyczna ujawnia się w polu po roku, jako „baterie padają
szybciej, niż obiecaliśmy" — czyli wtedy, gdy nie da się jej naprawić
aktualizacją. Budżet nie jest przenośny między platformami (ADR-011), więc
próg jest definiowany per `platform` + `board`.

---

## 8. Chmura

- **Jednostkowe**: ewaluacja reguł, reconcile shadow, watchdog presence (czas
  wstrzykiwany, nie systemowy), kohortowanie i bramki OTA.
- **Integracyjne** (`testcontainers`): realny Postgres+Timescale, NATS, MinIO.
  `sqlx` weryfikuje zapytania przy kompilacji, ale hypertable, ciągłe agregaty
  i polityki retencji trzeba sprawdzić na żywej bazie.
- **Izolacja tenantów** — osobna, obowiązkowa klasa testów: dla każdego
  endpointu i każdej tabeli próba dostępu tokenem obcego tenanta musi zwrócić
  pustkę albo 403. RLS jest siatką bezpieczeństwa, test jest dowodem, że jest
  napięta.
- **Migracje**: `up` na kopii schematu produkcyjnego oraz test zgodności
  wstecznej — poprzednia wersja binarki musi działać na nowym schemacie, bo
  wdrożenie jest rolling ([CI.md](CI.md) sekcja 6).
- **Obciążeniowe**: `pkpu-ingest` z wirtualną flotą (sekcja 9) w profilu
  docelowym i szczytowym. Profil szczytowy to jednoczesna rekonekcja całej floty
  po awarii sieci — scenariusz realny, nie teoretyczny.
- **Chaos**: zabicie NATS, bazy i MinIO w trakcie ruchu. Wymóg: brak cichej
  utraty danych — albo ACK, albo urządzenie retransmituje.
- **Odtworzenie**: restore bazy z JetStream w oknie SLO (< 30 min,
  [CLOUD.md](CLOUD.md) 10) — testowane, nie zakładane.

---

## 9. Symulator urządzenia i wirtualna flota

`pkpu-simulator` — binarka hostowa uruchamiająca **prawdziwy**
`pkpu-device-core` na `pkpu-platform-mock`, z `Link` podpiętym do realnego MQTT.
To nie jest atrapa protokołu: to ten sam kod, który jedzie na urządzeniu.

Zastosowania:
- E2E bez sprzętu: provisioning → telemetria → komenda → OTA → rollback,
- testy skali: N tysięcy instancji z profilami `SLEEP` i `NONSLEEP`,
- odtwarzanie incydentów: symulator karmiony ruchem zarejestrowanym w polu,
- rozwój chmury bez pojedynczej płytki w pętli.

Czego symulator **nie** zdejmuje: czasu, prądu i flasha. Zielony symulator przy
czerwonym HIL oznacza błąd w warstwie platformy, nie w logice — i odwrotnie,
zielony HIL przy czerwonym symulatorze oznacza, że test HIL jest za płytki.

---

## 10. Mobile

- `pkpu-core` na hoście z fake `BleTransport`/`NfcTransport` — cała maszyna
  provisioningu, w tym ścieżki błędne z [MOBILE.md](MOBILE.md) sekcja 3.
- Smoke test bindingów UniFFI na obu platformach: czy artefakt się ładuje i czy
  typy przechodzą granicę FFI. To jedyne, co musi jechać na macOS i Androida.
- Testy UI: minimalne, na krytycznej ścieżce onboardingu. Reszta to koszt bez
  pokrycia ryzyka.

---

## 11. Testy flotowe

Ostatnia linia — łapie to, czego laboratorium nie złapie: różnorodność sieci,
temperatur, wersji firmware i zachowań użytkowników.

| Mechanizm | Zasada |
|---|---|
| Staging tenant | pełna kopia stosu, kilkanaście realnych urządzeń, reszta ruchu symulowana |
| Kohorta canary | 1% floty, min. 24 h soak przed rozszerzeniem fali ([CLOUD.md](CLOUD.md) 5) |
| Urządzenia referencyjne | kilka sztuk w produkcji pod stałym monitoringiem prądu i logów |
| Bramka metryczna | wzrost `DISCONNECTED` albo braku `mark_boot_ok` w kohorcie → automatyczna pauza kampanii |
| Game day | ćwiczenie z zegarem: awaria brokera, awaria bazy, rollback floty |

---

## 12. Bramki

| Etap | Musi przejść |
|---|---|
| PR | fmt, clippy `-D warnings`, kontrakt, jednostkowe (firmware/chmura/mobile), build macierzy targetów, conformance na mocku, integracyjne chmury, brak regresji rozmiaru |
| Merge do `main` | powyższe + E2E na symulatorze |
| Nocne | HIL na wszystkich rodzinach, fuzzing, soak wirtualnej floty, testy obciążeniowe |
| Release firmware | HIL zielony na docelowej platformie + test energetyczny + OTA z wymuszonym rollbackiem na sprzęcie |
| Rollout | kohorta canary + bramka metryczna między falami |

Reguła: **bramki nie obchodzi się przez wyłączenie testu.** Jeżeli test jest zły,
poprawia się go w osobnym PR z uzasadnieniem — a nie przy okazji zmiany, którą
akurat blokuje.
