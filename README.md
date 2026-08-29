# PKPU — Platforma IoT

DOKUMENTACJA PROJEKTOWA

Architektura referencyjna dla przyszłych projektów IoT. Cały kod produkcyjny
pisany w **Rust** — od firmware (`no_std`) przez chmurę (`tokio`) po rdzeń
aplikacji mobilnej (UniFFI).

Dokumentacja jest dwujęzyczna: [`docs/pl/`](docs/pl/) (wersja źródłowa) oraz
[`docs/eng/`](docs/eng/) (English version).

## Spis dokumentów

| Dokument | PL | EN | Zakres |
|---|---|---|---|
| ARCHITECTURE | [pl](docs/pl/ARCHITECTURE.md) | [en](docs/eng/ARCHITECTURE.md) | Widok całości, granice systemów, przepływy danych |
| DEVICE | [pl](docs/pl/DEVICE.md) | [en](docs/eng/DEVICE.md) | Stack urządzenia: HAL, radio, stany, provisioning, OTA |
| PROTOCOL | [pl](docs/pl/PROTOCOL.md) | [en](docs/eng/PROTOCOL.md) | Kontrakt wspólny: DeviceId, ramki, komendy, kodowanie |
| CLOUD | [pl](docs/pl/CLOUD.md) | [en](docs/eng/CLOUD.md) | Stack chmurowy: ingest, serwisy, autoryzacja, deployment |
| DATA | [pl](docs/pl/DATA.md) | [en](docs/eng/DATA.md) | Model danych, schemat bazy, retencja, zapytania |
| MOBILE | [pl](docs/pl/MOBILE.md) | [en](docs/eng/MOBILE.md) | Aplikacja mobilna i rdzeń współdzielony |
| MATTER | [pl](docs/pl/MATTER.md) | [en](docs/eng/MATTER.md) | Zgodność z ekosystemem: most, tożsamość, certyfikacja |
| TESTING | [pl](docs/pl/TESTING.md) | [en](docs/eng/TESTING.md) | Strategia i poziomy testów, conformance, bramki |
| CI | [pl](docs/pl/CI.md) | [en](docs/eng/CI.md) | Pipeline'y, wydania, podpisywanie firmware, zgodność |
| SDK | [pl](docs/pl/SDK.md) | [en](docs/eng/SDK.md) | Procedury rozwoju SDK: API, wersjonowanie, dodawanie platform |
| DECISIONS | [pl](docs/pl/DECISIONS.md) | [en](docs/eng/DECISIONS.md) | Log decyzji (ADR) + otwarte pytania |

## Trzy filary

1. **Device Side stack** — Embassy + `no_std`, przenośny rdzeń SDK: ten sam kod
   firmware na **nRF, STM32 i ESP32**, jedna baza kodu na wiele MCU i wiele radii.
2. **Cloud stack** — Rust (axum/tokio), MQTT/NATS, PostgreSQL + TimescaleDB.
3. **Mobile stack** — rdzeń w Rust (`pkpu-core`) + cienki UI natywny.

Krzem i radio to dwie niezależne osie: `pkpu-platform-*` zmienia się wraz z MCU,
`pkpu-link` wraz ze stosem radiowym. Rdzeń nie zna ani jednego, ani drugiego —
szczegóły w [DEVICE.md](docs/pl/DEVICE.md) sekcja 4 i [ADR-011](docs/pl/DECISIONS.md).

## Zasada nadrzędna: jeden kontrakt

Crate `pkpu-proto` jest **jedynym** źródłem prawdy dla typów przesyłanych między
warstwami. Jest `no_std + alloc`-kompatybilny, więc ten sam kod kompiluje się
na Cortex-M, na serwerze i do biblioteki mobilnej. Zmiana formatu ramki w jednym
miejscu łamie kompilację wszystkich konsumentów — to jest zamierzone.

```
                    ┌────────────────┐
                    │  pkpu-proto    │  (no_std, serde, postcard)
                    └───┬────┬───┬───┘
              ┌─────────┘    │   └─────────┐
        ┌─────▼─────┐  ┌─────▼─────┐  ┌────▼──────┐
        │ firmware  │  │   cloud   │  │  mobile   │
        └───────────┘  └───────────┘  └───────────┘
```

## Status

Faza dokumentacyjna. Brak kodu — najpierw domykamy architekturę i decyzje
otwarte z [DECISIONS.md](docs/pl/DECISIONS.md).
