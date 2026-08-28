# SDK — procedury tworzenia i rozwoju

Jak powstaje i jak się zmienia wspólna część firmware. Dokument jest o
**procesie i zasadach API**, nie o architekturze — tę opisuje
[DEVICE.md](DEVICE.md). Testy: [TESTING.md](TESTING.md). Wydania:
[CI.md](CI.md).

---

## 1. Co jest SDK, a co produktem

| Warstwa | Rola | Kto zmienia |
|---|---|---|
| `pkpu-proto` | kontrakt przez granice procesu | zmiana za zgodą właściciela crate'u (CODEOWNERS) |
| `pkpu-hal` | traity sprzętowe, kontrakt portu | zmiana = zmiana kontraktu dla wszystkich platform |
| `pkpu-device-core` | maszyna stanów, scheduler, OTA, storage | rdzeń SDK, przenośny |
| `pkpu-link` | abstrakcja radia + implementacje | rdzeń SDK, przenośny |
| `pkpu-platform-*`, `boards/` | implementacja pod krzem | właściciel platformy |
| `apps/<produkt>` | **produkt**, nie SDK | zespół produktowy |

Linia podziału: **SDK nie wie, jaki produkt powstaje; produkt nie wie, na czym
działa.** Jeżeli w SDK pojawia się nazwa produktu albo w `apps/` nazwa
producenta MCU — któraś strona przecieka i to jest defekt do naprawy, nie
szczegół.

---

## 2. Cykl życia zmiany w SDK

```
1. POTRZEBA          zgłoszona przez produkt ("apps/ nie da się tego napisać
                     bez cfg-a / bez duplikacji / bez sięgnięcia do HAL-a")
2. KWALIFIKACJA      czy to zmiana kontraktu?  -> tak: ADR (DECISIONS.md)
                     czy dotyka pkpu-proto?    -> tak: wersjonowanie wire (sekcja 5)
                     czy dotyka pkpu-hal?      -> tak: dotyka WSZYSTKICH platform
3. PROJEKT API       trait / typ / sygnatura + uzasadnienie w opisie PR
                     obowiązkowo: jak to wygląda z perspektywy apps/
4. IMPLEMENTACJA     core + pkpu-platform-mock  (host)
5. TESTY             scenariusz w TESTING.md 4 + rozszerzenie conformance,
                     jeśli zmiana dotyka kontraktu platformy
6. PORTY             implementacja w każdej platformie z macierzy
                     -> conformance na HIL musi przejść na KAŻDEJ
7. DOKUMENTACJA      rustdoc + przykład + wpis w CHANGELOG + aktualizacja docs/
8. WYDANIE           tag, semver wg sekcji 5
```

Krok 6 jest miejscem, w którym najczęściej pojawia się pokusa: „na razie
zaimplementuję tylko na nRF, reszta później". Skutek jest zawsze ten sam —
`pkpu-hal` przestaje być kontraktem, a staje się opisem jednej platformy.
Dlatego: **trait wchodzący do `pkpu-hal` musi mieć implementację na każdej
platformie w macierzy albo nie wchodzi.** Jeśli czegoś fizycznie nie ma na
którymś krzemie, wyraża się to typem (`Option`, osobny trait opcjonalny,
`type Unsupported`), a nie brakiem implementacji.

---

## 3. Zasady projektowania API

Reguły wynikające z tego, że kod jedzie na urządzenie, którego nie da się
odwiedzić, i musi być przenośny między trzema rodzinami MCU (ADR-011):

| Zasada | Powód |
|---|---|
| `no_std`, `alloc` opcjonalny i wyłączony domyślnie w core | najmniejsza platforma w macierzy wyznacza budżet |
| Brak paniki w ścieżce runtime; błąd = `Result` z typem `defmt::Format` | panika na urządzeniu = reset, a reset w polu = utrata danych |
| `unwrap()`/`expect()` dozwolone wyłącznie w inicjalizacji BSP | tam awaria i tak oznacza niedziałający sprzęt |
| Traity minimalne; jedna odpowiedzialność na trait | każdy dodany element trzeba zaimplementować N razy |
| Generyki, nie `dyn`, w gorących ścieżkach | rozmiar obrazu i brak alokacji |
| Async-first (`embassy`), zero blokujących pętli | budżet energetyczny (DEVICE 8) |
| Brak alokacji i `format!` w ścieżce telemetrii | fragmentacja i skoki poboru |
| **Cechy (`features`) addytywne, nigdy wykluczające się** | cargo unifikuje cechy w grafie — dwie wykluczające się cechy to błąd linkowania u konsumenta, nie u autora |
| `#[non_exhaustive]` na publicznych enumach i strukturach konfiguracji | dodanie wariantu nie łamie konsumentów |
| Publiczne API z `#![deny(missing_docs)]` | API bez opisu i tak zostanie użyte — tylko źle |
| Wersje MSRV i toolchaina przypięte w repo | ESP32 Xtensa ma własny fork; rozjazd wersji psuje macierz |

Osobno: **nic w SDK nie mierzy czasu systemowego bezpośrednio.** Czas idzie
przez `Clock` z `pkpu-hal`, bo inaczej nie da się testować na hoście z czasem
symulowanym ([TESTING.md](TESTING.md) 4) — a to jest fundament całej strategii
testowej.

---

## 4. Trzy procedury

### 4.1 Dodanie platformy (nowa rodzina MCU)

Kontrakt i checklista: [DEVICE.md](DEVICE.md) 4.4. Kryteria akceptacji:

1. `pkpu-platform-<x>` implementuje pełny `Platform` — bez `todo!()`,
2. zestaw conformance przechodzi **na sprzęcie**, nie tylko na mocku,
3. płytka stoi na stanowisku HIL i job jest w nocnym pipelinie,
4. build target dodany do macierzy PR,
5. pomiar prądu ustala próg energetyczny dla tej platformy,
6. wpis w [DEVICE.md](DEVICE.md) 4.2 i 4.5 (znane ograniczenia — uczciwie).

Bez punktów 2–3 to nie jest wsparcie platformy, tylko deklaracja. Platforma bez
płytki na HIL jest usuwana z macierzy przy pierwszej awarii, której nikt nie
umie odtworzyć.

### 4.2 Dodanie technologii radiowej

1. Implementacja `Link` + uczciwy `LinkProfile` (MTU, RTT, push-capable, koszt
   energetyczny) — core dostraja się do tych liczb, więc kłamstwo w profilu
   objawia się jako „dziwne" zachowanie telemetrii,
2. `conformance::link` przechodzi, w tym zgodność deklarowanego profilu z pomiarem,
3. scenariusz degradacji łącza w testach hostowych ([TESTING.md](TESTING.md) 4),
4. ścieżka provisioningu sieci PAN opisana w [DEVICE.md](DEVICE.md) 6.3,
5. mapowanie na ścieżkę do chmury: bezpośrednio czy przez gateway
   ([ARCHITECTURE.md](ARCHITECTURE.md) 3).

### 4.3 Dodanie produktu

1. `apps/<produkt>/product.toml` — `model`, `platform`, `board`, taksonomia,
   budżety ([DEVICE.md](DEVICE.md) 11),
2. kanały pomiarowe zdefiniowane w `pkpu-proto` i wygenerowany wpis
   `device_types` + `device_type_channels` ([DATA.md](DATA.md)),
3. BSP w `boards/` jeśli płytka jest nowa,
4. E2E na symulatorze przed pierwszym egzemplarzem sprzętu,
5. test energetyczny z progiem z `product.toml`,
6. pipeline wydania firmware dla tego modelu ([CI.md](CI.md) 7).

Produkt **nie** dodaje kodu do `pkpu-device-core`. Jeżeli musi — to znaczy, że
brakuje punktu rozszerzenia w SDK i właściwą zmianą jest cykl z sekcji 2.

---

## 5. Wersjonowanie i zgodność

Trzy niezależne osie, celowo rozdzielone:

| Oś | Wersjonowanie | Kto łamie, ten płaci |
|---|---|---|
| Crate'y SDK | semver, `cargo-semver-checks` w CI | konsument = nasze `apps/` — koszt kontrolowany |
| **Protokół na łączu** | pole `v` w `Envelope`, osobno od semver | konsument = urządzenia w polu — koszt niekontrolowany |
| Schemat bazy | migracje expand/contract ([CI.md](CI.md) 6) | konsument = działająca chmura |

Reguły dla protokołu — najostrzejsze, bo urządzenie w polu żyje latami
i **nie da się go zaktualizować w tej samej chwili co chmury**:

- pola się **dodaje**, nigdy nie usuwa ani nie zmienia znaczenia,
- wartości enumów są dopisywane na końcu; istniejąca wartość jest wieczna
  ([PROTOCOL.md](PROTOCOL.md) 2),
- **chmura obsługuje wszystkie wersje wire, które są w polu** — dopóki ostatnie
  urządzenie z daną wersją nie zostanie wycofane z rejestru, nie usuwamy jej
  obsługi,
- **firmware nigdy nie wymaga nowszej chmury**: nowe urządzenie musi działać ze
  starszą wersją backendu przez okres rolloutu,
- zmiana łamiąca wymaga bumpa `v`, wektorów zgodności i ADR-a. To nie jest
  zakazane — jest kosztowne i ma być widoczne.

Deprecacja API SDK: oznaczenie `#[deprecated]` z podaniem zamiennika →
minimum jeden pełny cykl wydawniczy współistnienia → usunięcie przy bumpie
major. Dla protokołu okno liczy się w latach życia sprzętu, nie w wydaniach.

---

## 6. Definition of Done dla zmiany w SDK

Lista sprawdzana w review — brak pozycji jest powodem do niescalenia:

- [ ] testy hostowe pokrywają nowy scenariusz (nie tylko happy path),
- [ ] conformance rozszerzony, jeśli zmienia się kontrakt platformy,
- [ ] implementacja na **wszystkich** platformach z macierzy,
- [ ] wpływ na rozmiar obrazu w raporcie CI, uzasadniony jeśli > próg,
- [ ] wpływ na pobór prądu zmierzony, jeśli zmiana dotyka radia, sleepu lub flasha,
- [ ] rustdoc na publicznym API + przykład użycia z perspektywy `apps/`,
- [ ] zgodność wire nienaruszona albo bump `v` + wektory + ADR,
- [ ] CHANGELOG,
- [ ] zaktualizowany odpowiedni dokument w `docs/` — dokumentacja rozjeżdżająca
      się z kodem jest gorsza niż jej brak, bo jest cytowana.

---

## 7. Role i przeglądy

| Obszar | Właściciel decyzji | Co wymaga zgody |
|---|---|---|
| `pkpu-proto` | właściciel kontraktu | każda zmiana pól, enumów, wersji wire |
| `pkpu-hal` | właściciel SDK | dodanie/zmiana traitu — dotyka wszystkich platform |
| `pkpu-platform-*` | właściciel danej platformy | zmiany lokalne bez zgody właściciela SDK |
| `apps/` | zespół produktowy | swobodnie w granicach API SDK |
| Wydanie firmware, podpis | osoba z uprawnieniem release | zatwierdzenie w chronionym środowisku ([CI.md](CI.md) 9) |

Przy zespole jedno- lub dwuosobowym role zbiegają się w jednej osobie — wtedy
mechanizmem nie jest review, tylko **ADR**: decyzja zapisana wtedy, gdy jest
podejmowana. Za pół roku nikt nie odtworzy, dlaczego trait wygląda tak, a nie
inaczej, a odtworzyć trzeba będzie przy pierwszym porcie.

---

## 8. Czego SDK świadomie nie robi

- **Nie ukrywa różnic, których nie da się ukryć**: budżetu energetycznego,
  formatu obrazu, mapy flasha ([DEVICE.md](DEVICE.md) 4.5). Udawanie, że są
  przenośne, jest gorsze niż jawne ich wystawienie.
- **Nie owija HAL-a producenta w całości.** Trait powstaje wtedy, gdy potrzebuje
  go core — nie „na zapas", bo przewidujemy przyszłe użycie.
- **Nie zawiera logiki produktowej.** Próg alarmu, kalibracja czujnika, nazwa
  kanału — to `apps/`.
- **Nie wspiera platform bez płytki na HIL.** Wsparcie, którego nikt nie mierzy,
  jest wsparciem tylko na papierze.
