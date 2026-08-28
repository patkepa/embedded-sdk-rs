# PKPU — Platforma IoT

DOKUMENTACJA PROJEKTOWA

Architektura referencyjna dla przyszłych projektów IoT. Cały kod produkcyjny
pisany w **Rust** — od firmware (`no_std`) przez chmurę (`tokio`) po rdzeń
aplikacji mobilnej (UniFFI).

## Spis dokumentów

| Dokument | Zakres |
|---|---|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | Widok całości, granice systemów, przepływy danych |
| [DEVICE.md](docs/DEVICE.md) | Stack urządzenia: HAL, radio, stany, provisioning, OTA |
| [PROTOCOL.md](docs/PROTOCOL.md) | Kontrakt wspólny: DeviceId, ramki, komendy, kodowanie |
| [CLOUD.md](docs/CLOUD.md) | Stack chmurowy: ingest, serwisy, autoryzacja, deployment |
| [DATA.md](docs/DATA.md) | Model danych, schemat bazy, retencja, zapytania |
| [MOBILE.md](docs/MOBILE.md) | Aplikacja mobilna i rdzeń współdzielony |
| [DECISIONS.md](docs/DECISIONS.md) | Log decyzji (ADR) + otwarte pytania |

## Trzy filary

1. **Device Side stack** — Embassy + `no_std`, jedna baza kodu na wiele MCU i wiele radii.
2. **Cloud stack** — Rust (axum/tokio), MQTT/NATS, PostgreSQL + TimescaleDB.
3. **Mobile stack** — rdzeń w Rust (`pkpu-core`) + cienki UI natywny.

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
otwarte z [DECISIONS.md](docs/DECISIONS.md).
