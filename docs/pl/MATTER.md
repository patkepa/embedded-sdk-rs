# MATTER — zgodność z ekosystemem

Matter jest jedyną decyzją w tej dokumentacji, która zmienia **warstwę
aplikacyjną urządzenia**, a nie tylko transport. Dlatego ma własny dokument,
a nie akapit w [DEVICE.md](DEVICE.md).

**Decyzja bazowa (ADR-012):** Matter jest **powierzchnią kompatybilności**,
nie modelem wewnętrznym platformy. Domyślną drogą jest **most (bridge)** na
gatewayu; tryb natywny tylko dla produktów, w których ekosystem jest wartością
samą w sobie.

---

## 1. Co Matter daje, a czego nie daje

| Obszar | Matter załatwia | Zostaje po naszej stronie |
|---|---|---|
| Sterowanie lokalne z ekosystemów | tak — Apple, Google, Amazon, SmartThings bez integracji per ekosystem | — |
| Onboarding przez cudzą aplikację | tak — QR / kod parowania, jeden przepływ | nasz provisioning dla funkcji spoza modelu Matter |
| Bezpieczeństwo lokalne | tak — attestacja urządzenia, certyfikaty operacyjne, sesje CASE | tożsamość wobec **naszej** chmury |
| Interoperacyjność typów urządzeń | tak — dla typów z biblioteki Matter | wszystko, czego w bibliotece nie ma |
| **Telemetria historyczna** | **nie** — model jest stanowy, nie pomiarowy | całość: bufory, backfill, retencja, agregaty |
| **Zarządzanie flotą** | **nie** | rejestr, kampanie OTA, kohorty, presence, alerty |
| **Multi-tenancy, RBAC, audyt** | **nie** | całość ([CLOUD.md](CLOUD.md)) |
| Chmura producenta | nie zastępuje | nadal potrzebna |

Wniosek, który wyznacza całą resztę dokumentu: **Matter nie jest alternatywą dla
tej platformy — jest interfejsem do niej.** Produkt bez chmury producenta traci
telemetrię, historię, reguły i OTA falowe. Produkt bez Matter traci integrację
z ekosystemami. To są rozłączne wartości i dlatego oba byty muszą współistnieć.

---

## 2. Zderzenie modeli danych

| Nasz model ([PROTOCOL.md](PROTOCOL.md)) | Matter | Zgodność |
|---|---|---|
| `DeviceId` (UUIDv7, nasz) | Node ID (per fabric) + VID/PID | różne przestrzenie nazw — mapowanie, nie zastąpienie |
| `Shadow.desired` / `reported` | atrybuty klastrów + subskrypcje | koncepcyjnie bliskie, ale Matter nie ma „desired" — zapis atrybutu jest natychmiastowy |
| `Telemetry { channel, value }` | atrybut typowanego klastra (np. `MeasuredValue`) | nasze kanały są dowolne, klastry — nie |
| `Command` z `command_id`, TTL, dedup | komenda klastra, bez TTL i bez idempotencji z definicji | nasza semantyka jest bogatsza; przy moście trzeba ją domknąć |
| Zdarzenia | Events | zbieżne |
| Historia pomiarów | **brak** | wyłącznie po naszej stronie |

Praktyczna konsekwencja projektowa: **definiując kanał pomiarowy, sprawdzamy
najpierw, czy istnieje odpowiadający mu klaster Matter, i przyjmujemy jego
jednostkę oraz skalę.** Wtedy most jest mapowaniem tabelarycznym. Jeśli wymyślimy
własną skalę dla temperatury, most stanie się miejscem konwersji, zaokrągleń
i błędów — a taki kod zawsze trafia do produkcji najpóźniej i najgorzej
przetestowany.

---

## 3. Trzy modele integracji

### A. Most (bridge) — **domyślny**

Gateway wystawia nasze urządzenia jako Bridged Devices w jednym certyfikowanym
węźle-mostku. Urządzenia końcowe pozostają natywne (nasz protokół, nasze radio,
nasz provisioning).

- (+) **jedna certyfikacja zamiast certyfikacji każdego SKU**,
- (+) urządzenia bateryjne nie płacą za Matter ani flashem, ani prądem,
- (+) działa dla Zigbee, które do Matter inaczej nie wejdzie,
- (+) nasza architektura zostaje bez zmian,
- (−) wymaga gatewaya — czyli nie działa dla produktów `WIFI` direct-to-cloud,
- (−) urządzenie „widziane" przez ekosystem tylko wtedy, gdy most żyje,
- (−) ekosystemy traktują urządzenia mostkowane jako obywateli drugiej kategorii
  (ograniczone typy, brak części funkcji).

### B. Dual-stack — dla wybranych SKU

Urządzenie mówi Matter (lokalnie, do ekosystemów) **i** naszym protokołem
(do chmury: telemetria, OTA, flota).

- (+) pełnoprawne urządzenie Matter bez utraty telemetrii i zarządzania flotą,
- (−) dwa stosy w jednym obrazie: flash, RAM, dwa harmonogramy radiowe,
- (−) dwa przepływy onboardingu, które użytkownik myli — wymaga przemyślanego UX,
- (−) certyfikacja per SKU, recertyfikacja przy zmianach,
- (−) realny koszt energetyczny — dla `SLEEP` do policzenia, nie do założenia.

### C. Natywny Matter — tylko gdy ekosystem **jest** produktem

Urządzenie mówi wyłącznie Matter, chmura producenta (jeśli w ogóle) sięga przez
kontroler.

- (+) najprostszy produkt konsumencki, najniższy próg wejścia dla użytkownika,
- (−) tracimy telemetrię historyczną, kampanie falowe, presence i multi-tenancy —
  czyli wszystko, co ta platforma wnosi,
- (−) nasze SDK redukuje się do cienkiej warstwy nad cudzym stosem.

**Rekomendacja:** A jako domyślne, B dla produktów konsumenckich z realnym
popytem na ekosystem, C tylko dla pojedynczego SKU, gdzie platforma i tak nie
była potrzebna.

---

## 4. Wpływ na taksonomię

Matter **nie jest** `COM_TYPE`. To warstwa aplikacyjna nad IPv6 (Thread, Wi-Fi,
Ethernet), a BLE służy w niej wyłącznie do commissioningu. Dopisanie `MATTER`
do `ComType` byłoby błędem kategorii i rozjechałoby enum, którego wartości są
wieczne ([PROTOCOL.md](PROTOCOL.md) 2).

Zamiast tego — **osobny, ortogonalny wymiar**:

```rust
#[repr(u8)]
pub enum MatterMode {
    None   = 0,   // urządzenie spoza świata Matter
    Bridged = 1,  // wystawiane przez most na gatewayu
    Dual   = 2,   // własny stos Matter + nasz protokół
    Native = 3,   // wyłącznie Matter
}
```

W `product.toml`:

```toml
[matter]
mode        = "bridged"        # none | bridged | dual | native
device_type = "0x0302"         # typ z biblioteki Matter, jeśli dotyczy
vid         = "0xFFF1"         # testowy do czasu przydziału z CSA
pid         = "0x8001"
```

Klasyfikacja urządzenia staje się iloczynem pięciu wymiarów:
`SLEEP_TYPE × COM_TYPE × PROV_TYPE × MATTER_MODE × STATE`.

---

## 5. Dwie tożsamości na jednym urządzeniu

To jest miejsce, w którym Matter najmocniej dotyka naszego modelu bezpieczeństwa
([ARCHITECTURE.md](ARCHITECTURE.md) 7, ADR-007).

| | Nasza tożsamość | Tożsamość Matter |
|---|---|---|
| Klucz | ed25519 generowany na chipie | DAC — klucz + certyfikat urządzenia |
| Łańcuch | nasze CA fabryczne → operacyjne | DAC → PAI → PAA, wpis w DCL |
| Kto wystawia | my | my, ale w łańcuchu uznawanym przez CSA |
| Do czego | uwierzytelnienie wobec naszej chmury | attestacja przy commissioningu do fabric |
| Cykl życia | całe życie urządzenia | całe życie urządzenia, plus NOC per fabric (wymienne) |

Współistnienie jest możliwe i konieczne, ale **linia produkcyjna musi wgrać dwa
zestawy poświadczeń** — nasz i Matter (DAC + CD). To realny koszt stanowiska
provisioningu, wymagający własnego PAA/PAI i procedury wydawania DAC-ów, więc
decyzja o Matter zapada **przed** uruchomieniem produkcji, a nie po.

Wariant `Bridged` tę komplikację usuwa: DAC ma tylko most.

---

## 6. Commissioning i wielość administratorów

Matter wnosi przepływ onboardingu, którego nie kontrolujemy: użytkownik może
sparować urządzenie z poziomu obcej aplikacji, nigdy nie otwierając naszej.

Konsekwencje do zaprojektowania:

- **Urządzenie może być w fabric ekosystemu i nie być zaclaimowane u nas.**
  Rejestr musi dopuszczać taki stan, zamiast traktować go jako błąd.
- **Multi-admin jest wbudowany w Matter** — użytkownik ma prawo dołożyć kolejny
  ekosystem. Nasza chmura nie jest jedynym administratorem i nie może tego
  zakładać.
- **Odebranie urządzenia (decommission) z obcej aplikacji** musi być wykryte,
  a nie objawiać się jako cicha utrata funkcji.
- Kod parowania Matter (QR / NFC) i nasz kod provisioningu ([DEVICE.md](DEVICE.md)
  6) muszą być **jednym** nadrukiem na obudowie. Dwa różne QR na jednym
  urządzeniu to gwarantowany błąd użytkownika i lawina zgłoszeń serwisowych.

---

## 7. Urządzenia bateryjne: ICD a nasz `SLEEP`

Matter ma własną odpowiedź na urządzenia śpiące — **ICD** (Intermittently
Connected Devices), z wariantami krótko- i długo-interwałowymi. Odwzorowanie:

| Nasze | Matter | Uwaga |
|---|---|---|
| `SLEEP_TYPE = NONSLEEP` | urządzenie zawsze osiągalne | zbieżne |
| `SLEEP_TYPE = SLEEP` | ICD (SIT / LIT) | parametry ICD muszą wynikać z `expected_interval` |
| stan `SLEEPS` | okno osiągalności ICD | ta sama idea: brak odpowiedzi ≠ awaria |
| kolejkowanie komend u parenta | mechanika ICD | zbieżne |

Dobra wiadomość: Matter rozwiązuje ten problem tak samo jak my, więc `SLEEPS`
z ADR-009 nie jest konfliktowe. Zła: parametry ICD są **negocjowane z fabric**
i mogą się rozjechać z naszym budżetem energetycznym. Jeśli ekosystem wymusi
częstsze okna niż zakłada `product.toml`, deklarowany czas życia baterii
przestaje obowiązywać — i trzeba to wykryć testem, nie reklamacją.

---

## 8. Dwie ścieżki OTA, jedno źródło prawdy

W trybach `Dual` i `Native` istnieje druga droga aktualizacji: klaster OTA
Requestor / Provider, z własnym formatem nagłówka obrazu.

Reguły, żeby to nie eksplodowało:

1. **Jeden artefakt, jedno źródło wersji.** Obraz powstaje raz w naszym pipelinie
   ([CI.md](CI.md) 7) i jest opakowywany w format Matter — nigdy nie budujemy
   dwóch obrazów tej samej wersji.
2. **`SoftwareVersion` Matter jest wyprowadzany z naszej wersji** deterministycznie
   (Matter wymaga liczby monotonicznej, my używamy semver — mapowanie musi być
   funkcją, nie tabelką w Excelu).
3. **Kampanie falowe zostają nasze.** Matter OTA nie ma kohort, bramek ani
   rollbacku flotowego. Ekosystemowa ścieżka jest kanałem zapasowym, nie
   podstawowym.
4. **Rollback A/B pozostaje w naszym bootloaderze** — niezależnie od tego, którą
   drogą przyszedł obraz.

---

## 9. Stos i platformy

Twardy fakt: **nie ma produkcyjnie certyfikowanego stosu Matter w Rust.**
Referencyjny SDK (`connectedhomeip`) jest w C++, wsparcie krzemu idzie przez
SDK producentów (nRF Connect SDK, `esp-matter`, ST). Rustowy `rs-matter` istnieje
i jest wart obserwowania, ale to nie jest baza pod certyfikację produktu — dziś.

To jest dokładnie ta sama sytuacja co Thread i Zigbee (ADR-004), więc odpowiedź
jest ta sama i spójna:

| Tryb | Realizacja |
|---|---|
| `Bridged` | stos Matter **tylko na gatewayu** (Linux, C++ SDK jako proces obok naszej binarki) — urządzenia czyste |
| `Dual` / `Native` | stos producenta w obrazie MCU + nasza logika, granica przez FFI, albo Matter na co-procesorze |

Wpływ na ADR-001 („Rust w całym stosie"): Matter jest **trzecim** wyłomem po
Thread i Zigbee. Trzy wyłomy w jednym miejscu to sygnał, że reguła brzmi
w rzeczywistości: *Rust wszędzie tam, gdzie nie stoi certyfikowany stos
radiowy/aplikacyjny osoby trzeciej*. Lepiej zapisać ją uczciwie w tej formie,
niż udawać, że wyjątki są incydentalne.

Wpływ na przenośność (ADR-011): most nie dotyka rdzenia SDK w ogóle. Tryb `Dual`
dotyka mocno — dostępność i dojrzałość SDK Matter staje się **kolejnym
kryterium doboru krzemu**, obok wsparcia embassy.

---

## 10. Certyfikacja — realny koszt

| Element | Co oznacza |
|---|---|
| Członkostwo CSA | opłata roczna; poziom decyduje o dostępie do specyfikacji i VID |
| VID / PID | identyfikator producenta z CSA; do czasu przydziału pracujemy na testowych |
| PAA / PAI | własny łańcuch attestacji + wpis do DCL, albo skorzystanie z łańcucha dostawcy krzemu |
| Certyfikacja produktu | testy w autoryzowanym laboratorium, per typ urządzenia |
| Recertyfikacja | przy istotnych zmianach i przy przejściu na nowe wersje specyfikacji |
| Utrzymanie | specyfikacja Matter wydaje kolejne wersje regularnie — zgodność jest procesem, nie jednorazowym zdarzeniem |

Dlatego most jest domyślny: **jeden certyfikowany artefakt zamiast N**, przy
zachowaniu widoczności całej gamy produktów w ekosystemach.

---

## 11. Testy i CI

Uzupełnienia do [TESTING.md](TESTING.md) i [CI.md](CI.md), jeśli Matter wchodzi:

- **Zestaw conformance mostka**: dla każdego wspieranego typu urządzenia mapowanie
  nasz kanał ↔ atrybut klastra jest testowane w obie strony, z jednostkami
  i skalą włącznie. To jest miejsce, gdzie błąd konwersji objawia się jako
  „termometr pokazuje 21°C w naszej aplikacji i 69,8 w Apple Home".
- **Test wielu administratorów**: parowanie do dwóch fabric jednocześnie,
  usunięcie z jednego, zachowanie drugiego.
- **Test rozbieżności stanu**: zmiana z ekosystemu vs zmiana z naszej chmury
  w tej samej chwili — rozstrzyganie musi być zdefiniowane, nie przypadkowe.
- **Test ICD kontra budżet energetyczny**: pomiar prądu przy parametrach
  narzuconych przez fabric, nie przy naszych domyślnych.
- **Test Harness CSA** w pipelinie nocnym — certyfikacja nie może być pierwszym
  momentem, w którym uruchamiamy oficjalne testy.
- **Artefakty DAC** obsługiwane jak klucze produkcyjne: HSM, brak dostępu
  z laptopa, audyt wydań ([CI.md](CI.md) 9).

---

## 12. Co robimy teraz, a co odkładamy

Zgodność z Matter nie wymaga dziś ani jednej linii kodu Matter — wymaga
**nieodcinania sobie drogi**:

| Teraz (koszt bliski zeru) | Odłożone |
|---|---|
| Kanały pomiarowe definiowane pod jednostki i skale klastrów Matter | implementacja mostka |
| `MatterMode` w taksonomii i `product.toml` | stos Matter w firmware |
| Rejestr dopuszcza urządzenie sparowane w obcym fabric, nie zaclaimowane u nas | multi-admin w UI |
| Jedno pole na obudowie na kod parowania | nadruk kodów Matter |
| Decyzja o VID/PAA **przed** uruchomieniem produkcji | certyfikacja |
| `SoftwareVersion` wyprowadzalny z naszej wersji | Matter OTA |

Etapowanie względem [ARCHITECTURE.md](ARCHITECTURE.md) 9: most wchodzi razem
z `pkpu-gateway` (etap 5), bo dopiero wtedy istnieje węzeł, który może go
udźwignąć. Tryb `Dual` — nie wcześniej niż po etapie 6 i tylko z konkretnym
produktem w ręku.

Rzecz, której **nie** wolno odłożyć: jeżeli Matter ma się kiedykolwiek pojawić,
to decyzje o VID, PAA i o tym, co wgrywa linia produkcyjna, zapadają przed
pierwszą serią. Urządzenia bez DAC-a nie da się dorobić zdalnie.
