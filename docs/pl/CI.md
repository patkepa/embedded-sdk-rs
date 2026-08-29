# CI/CD — pipeline'y i wydania

Co, kiedy i czym się uruchamia; jak powstaje artefakt, który trafia na
urządzenie i na serwer. Zakres testów opisuje [TESTING.md](TESTING.md),
zasady rozwoju SDK — [SDK.md](SDK.md).

---

## 1. Zasady

1. **Nic nie jest budowane ręcznie.** Artefakt, który trafia do produkcji lub na
   urządzenie, pochodzi wyłącznie z pipeline'u. Build z laptopa nie ma prawa
   dostać podpisu.
2. **Pipeline jest w repo i wersjonowany razem z kodem.** Zmiana pipeline'u
   przechodzi review jak kod.
3. **Szybki sygnał przed pełnym.** PR dostaje wynik w ~10 min; ciężkie rzeczy
   (HIL, soak, fuzzing) jadą nocą, a nie w pętli developera.
4. **Odtwarzalność.** `--locked`, przypięte wersje toolchaina (`rust-toolchain.toml`),
   akcje przypięte po SHA, obrazy po digeście. Build tego samego taga za rok ma
   dać ten sam bajt — inaczej nie da się zbadać incydentu z pola.
5. **Sekrety krótkożyjące.** OIDC do KMS/rejestru zamiast długowiecznych tokenów.
   Klucz podpisujący firmware nie opuszcza HSM — CI wysyła hash, dostaje podpis.
6. **Zielone `main` zawsze.** Merge queue: PR jest testowany na wyniku scalenia,
   nie na swojej starej bazie.

---

## 2. Topologia runnerów

| Runner | Do czego | Uwagi |
|---|---|---|
| linux x86_64 (chmurowy) | build, testy hostowe, cross-build, kontenery | trzon; skalowalny, wymienny |
| linux aarch64 | obrazy chmury na ARM, build `pkpu-gateway` | opcjonalny, gdy target edge to aarch64 |
| **HIL (self-hosted)** | testy na płytkach, pomiar prądu | maszyna z probe-rs, płytkami i PPK2; jedyny runner „ze stanem" |
| macOS | XCFramework i smoke test bindingów iOS | tylko pipeline mobilny |

Runner HIL jest zasobem współdzielonym i wąskim gardłem — kolejkuje zadania,
nie zrównolegla ich. Dlatego HIL nie jest w ścieżce PR (patrz sekcja 4).

---

## 3. Podział na pipeline'y

Monorepo z trzema workspace'ami ([ARCHITECTURE.md](ARCHITECTURE.md) 5) →
pipeline'y wyzwalane **ścieżkami**, żeby zmiana w chmurze nie budowała
sześciu targetów embedded:

| Pipeline | Wyzwalany zmianą w |
|---|---|
| `proto` | `proto/**` — plus **wymusza** uruchomienie `firmware`, `cloud` i `mobile` |
| `firmware` | `firmware/**`, `proto/**` |
| `cloud` | `cloud/**`, `proto/**`, migracje |
| `mobile` | `mobile/**`, `proto/**` |
| `e2e` | dowolna z powyższych, po scaleniu do `main` |

Zmiana w `proto/` z definicji dotyka wszystkich — to jest cena „jednego
kontraktu" i ma być widoczna w czasie CI, a nie odkrywana po wdrożeniu.

---

## 4. Pipeline PR

Kolejność ustawiona tak, by najtańsze bramki odpadały pierwsze.

```
[lint]        fmt --check | clippy -D warnings | typos | cargo-machete
                  |
[kontrakt]    testy proto | wektory złote | zgodność enumów z SQL
              cargo-semver-checks (publiczne API SDK) | snapshot OpenAPI/JSON Schema
                  |
        +---------+-----------------------------+------------------+
        |                                       |                  |
[firmware]                               [cloud]              [mobile]
  test hostowy (core + mock platform)      test jednostkowy     test core na hoście
  conformance na mocku                     testcontainers:      build bindingów
  build macierzy targetów (4.2 DEVICE)       PG+Timescale,      (smoke iOS/Android
  raport rozmiaru obrazu + próg               NATS, MinIO        tylko na main)
  test energetyczny (symulowany profil)     testy izolacji RLS
                                            test migracji up + wstecz
        +---------+-----------------------------+------------------+
                  |
[bezpieczeństwo]  cargo-deny (licencje, CVE, źródła) | audit zależności
                  |
[merge queue]     rebuild na wyniku scalenia -> merge
```

Budżet czasu: **10 min** dla ścieżki jednego workspace'u, 20 min gdy zmiana
dotyka `proto/`. Przekroczenie budżetu jest traktowane jak regresja — dzieli się
job albo poprawia cache (`sccache`, cache rejestru i `target/`).

Czego **nie ma** w PR i dlaczego: HIL (jeden runner, kolejka), fuzzing (czas),
soak (czas), testy obciążeniowe (koszt). Zamiast tego — sekcja 5.

---

## 5. `main` i pipeline nocny

Po scaleniu:

- E2E na symulatorze ([TESTING.md](TESTING.md) 9): provisioning → telemetria →
  komenda → OTA → rollback, na compose z pełnym stosem chmury,
- build obrazów kontenerowych i publikacja pod tagiem = git SHA,
- automatyczny deploy na **staging** + smoke E2E przeciw stagingowi.

Nocą (`main`, harmonogram):

| Zadanie | Czas | Efekt |
|---|---|---|
| HIL: conformance + OTA + rollback na każdej rodzinie MCU | ~40 min | raport per platforma |
| Pomiar prądu na płytkach referencyjnych | ~30 min | trend energetyczny per commit |
| Fuzzing dekodera `Envelope` | 30 min, korpus trwały | crash → automatyczny issue + test regresyjny |
| Soak wirtualnej floty (10k urządzeń, 8 h) | nocne | wycieki pamięci, dryf `seq`, stabilność presence |
| Testy obciążeniowe ingest | ~20 min | p99 vs SLO z [CLOUD.md](CLOUD.md) 10 |
| `cargo deny` + audit na świeżej bazie CVE | ~2 min | nowe podatności w zależnościach |

Czerwone zadanie nocne blokuje **release**, nie merge. Rozróżnienie jest
celowe: nocne testy łapią klasy błędów, których nie da się przypisać do
pojedynczego PR-a, ale nie mogą zatrzymywać codziennej pracy.

---

## 6. Wydanie chmury

```
tag cloud-vX.Y.Z
  -> build obrazów distroless (--locked), digest zapisany w release notes
  -> SBOM (CycloneDX) + attestacja provenance artefaktu
  -> deploy staging -> E2E -> soak 1 h
  -> zatwierdzenie człowieka -> deploy produkcja (rolling)
  -> migracje jako osobny job PRZED deployem binarek
```

Reguły twarde:

- **Migracje tylko wstecz-kompatybilne** (expand → migrate → contract). W czasie
  rolloutu stara i nowa binarka pracują na tym samym schemacie jednocześnie;
  `DROP COLUMN` jest osobnym wydaniem, po wygaszeniu starej wersji.
- **Rollback = poprzedni tag**, nie „odwracanie" migracji. Migracja odwracalna
  jest wyjątkiem, nie regułą — dlatego reguła powyżej.
- Konfiguracja walidowana przy starcie (fail-fast), więc zła konfiguracja
  zatrzymuje jeden pod, a nie flotę.

---

## 7. Wydanie firmware

Najbardziej wrażliwy pipeline w systemie: jego wyjście jedzie na sprzęt, którego
nie da się odwiedzić.

```
tag fw-<model>-vX.Y.Z
  -> build --locked dla platform zadeklarowanych w product.toml
  -> HIL: conformance + OTA + wymuszony rollback + pomiar prądu na docelowej płytce
  -> manifest: {model, platform, hw_rev_min/max, version, size, sha256,
                min_battery_pct, requires_reboot, sbom_digest}
  -> podpis ed25519 kluczem z HSM (środowisko chronione, wymaga zatwierdzenia)
  -> upload artefaktu: fw/{model}/{version}/{sha256}.bin
  -> wpis do firmware_versions (DATA.md 7)
  -> [KONIEC]  wydanie != rollout
```

- **Wydanie nie jest rolloutem.** Powstanie podpisanego artefaktu nie wysyła go
  na żadne urządzenie. Kampania OTA jest osobną, świadomą decyzją operatora
  z bramkami falowymi ([CLOUD.md](CLOUD.md) 5).
- **Podpis w chronionym środowisku** z ręcznym zatwierdzeniem. Kompromitacja
  konta developera nie może skutkować podpisanym firmware.
- `platform` w manifeście jest weryfikowane przy releasie i ponownie przez
  bootloader — rdzeń jest przenośny, obraz nie ([DEVICE.md](DEVICE.md) 4.5).
- Artefakty, manifesty i SBOM-y przechowujemy przez cały cykl życia produktu
  plus zapas — bez tego nie da się zbadać incydentu ani spełnić obowiązku
  raportowania (sekcja 9).

---

## 8. Wydanie rdzenia mobilnego

```
tag core-vX.Y.Z -> build pkpu-core -> UniFFI -> XCFramework (macOS runner) + AAR
                -> publikacja do rejestru artefaktów, wersjonowanie semver
                -> shelle iOS/Android konsumują wersję, nie ścieżkę lokalną
```

Shell nigdy nie buduje rdzenia ze źródeł na maszynie developera aplikacji —
inaczej „u mnie działa" wraca w najgorszym możliwym miejscu, czyli
w provisioningu u klienta.

---

## 9. Bezpieczeństwo i zgodność pipeline'u

| Wymóg | Realizacja |
|---|---|
| Brak długowiecznych sekretów | OIDC do chmury/KMS/rejestru; sekrety statyczne tylko tam, gdzie nie ma alternatywy |
| Klucz podpisujący | HSM/KMS, operacja podpisu zdalna, klucz nie opuszcza modułu |
| Integralność zależności | `Cargo.lock` w repo, `cargo deny` (licencje, CVE, źródła), akcje przypięte po SHA, zakaz `curl \| sh` w krokach |
| Pochodzenie artefaktu | attestacja provenance + SBOM (CycloneDX) do każdego wydania firmware i chmury |
| Rozdział ról | podpis firmware i deploy produkcyjny za bramką zatwierdzenia; autor zmiany nie zatwierdza własnego wydania |
| Retencja dowodów | artefakty, manifesty, SBOM, logi pipeline'u przez cykl życia produktu |

Kontekst regulacyjny (CRA / EN 18031, [DECISIONS.md](DECISIONS.md), pytania
otwarte 4): obowiązek bezpiecznej aktualizacji, SBOM-u i raportowania podatności
oznacza, że powyższe nie jest higieną „na później" — jest wymaganiem produktu.
Dobudowanie tego po fakcie jest droższe niż zrobienie od razu.

---

## 10. Konwencje repo

- **Conventional commits** → generowany CHANGELOG per workspace.
- **Tagi z prefiksem**: `proto-v*`, `fw-<model>-v*`, `cloud-v*`, `core-v*`.
  Jeden monorepo, cztery niezależne cykle wydawnicze.
- **Branch protection** na `main`: wymagane review, zielone bramki PR,
  merge queue, liniowa historia.
- **CODEOWNERS**: `proto/` ma właściciela — zmiana kontraktu wymaga jego zgody,
  bo łamie kompilację wszystkich konsumentów z założenia
  ([ARCHITECTURE.md](ARCHITECTURE.md), zasada jednego kontraktu).
- Zmiana w `proto/` bez wpisu w CHANGELOG i bez wektora zgodności nie przechodzi
  review — to nie jest formalność, to jedyny ślad, po którym za dwa lata odtworzy
  się, dlaczego urządzenie z firmware 1.3 mówi inaczej niż z 1.9.

---

## 11. Kolejność wdrażania samego CI

Nie budujemy całości od pierwszego dnia — kolejność wynika z etapów
z [ARCHITECTURE.md](ARCHITECTURE.md) 9:

| Etap projektu | Co dokładamy do CI |
|---|---|
| 0 (kontrakt) | lint, testy proto, wektory złote, `cargo deny` |
| 1 (telemetria E2E) | testcontainers dla chmury, build firmware na jednym targecie, obrazy + staging |
| 2 (komendy) | E2E na symulatorze, merge queue |
| 3 (provisioning) | pipeline mobilny, smoke bindingów |
| 4 (OTA) | **pipeline wydania firmware z podpisem HSM** + HIL (OTA/rollback) |
| 5 (druga platforma) | pełna macierz targetów, conformance na HIL, testy energetyczne |
| 6 (produkt) | soak, obciążeniowe, game day, retencja dowodów |

Pipeline wydania firmware musi istnieć **zanim** pierwsze urządzenie wyjdzie
poza biurko. Podpisywanie „tymczasowo ręcznie" jest tym rodzajem tymczasowości,
która zostaje na lata i której nie da się później odróżnić od kompromitacji.
