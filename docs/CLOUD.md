# 2. CLOUD — stack chmurowy

Wszystkie serwisy w Rust, `tokio` + `tracing`, jeden workspace `cloud/`.
Każdy serwis to osobna binarka, ale współdzielą crate `pkpu-proto` i
`pkpu-cloud-common` (konfiguracja, telemetria, pule połączeń, błędy).

---

## 1. Serwisy

| Serwis | Odpowiedzialność | Skalowanie |
|---|---|---|
| `pkpu-ingest` | broker MQTT + terminacja mTLS + walidacja ramek + publikacja na NATS | poziome, sticky per device |
| `pkpu-registry` | rejestr urządzeń, device shadow, presence, reconcile komend | poziome, stateless |
| `pkpu-provisioning` | rejestr fabryczny, attestacja, wydawanie certyfikatów, claiming | niskie QPS, 2 instancje |
| `pkpu-ota` | artefakty, manifesty, kampanie, rollout falowy | niskie QPS |
| `pkpu-rules` | ewaluacja reguł, alerty, webhooki, integracje | poziome per partycja tenanta |
| `pkpu-api` | REST + WebSocket dla web i mobile, autoryzacja OIDC | poziome, stateless |
| `pkpu-gateway` | binarka na edge (Linux), most PAN <-> MQTT | 1 per instalacja |

Świadomie **nie** zaczynamy od mikroserwisów rozdrobnionych — `registry`,
`provisioning` i `ota` mogą początkowo być jedną binarką z trzema modułami.
Podział jest zaprojektowany, wdrożony wtedy, gdy zaboli.

---

## 2. `pkpu-ingest` — brama danych

Broker MQTT 5 osadzony jako biblioteka (`rumqttd`) w procesie ingest,
nie osobny broker. Zysk: walidacja i autoryzacja dzieją się bez skoku sieciowego,
i mamy pełną kontrolę nad hookami connect/publish.

```
TLS accept
  -> ekstrakcja CN/SAN z certyfikatu klienta = device_id
  -> lookup w registry (cache Valkey, TTL 60 s): czy aktywne, czy nie odwołane
  -> ACL: topic MUSI zaczynać się od dev/{device_id}/
  -> dekodowanie postcard -> Envelope
  -> walidacja: v, seq (luki -> metryka), ts (dryf -> korekta)
  -> publikacja na NATS JetStream
  -> aktualizacja presence w Valkey (SETEX device:{id}:seen)
```

Backpressure: gdy JetStream nie nadąża, ingest przestaje ACK-ować QoS 1.
Urządzenia buforują lokalnie (patrz DEVICE.md sekcja 7). **Nigdy nie gubimy
danych po cichu** — albo ACK, albo urządzenie retransmituje.

### Tematy NATS

```
tel.{tenant}.{model}.{device_id}      telemetria
evt.{tenant}.{model}.{device_id}      zdarzenia
state.{tenant}.{device_id}            reported state
ack.{tenant}.{device_id}              potwierdzenia komend
cmd.{tenant}.{device_id}              komendy (cloud -> ingest -> device)
presence.{tenant}.{device_id}         zmiany ONLINE/DISCONNECTED/SLEEPS
```

Strumienie JetStream z retencją 7 dni — pozwala odtworzyć sink po awarii bazy
bez utraty danych.

---

## 3. `pkpu-registry` — rejestr i shadow

- Źródło prawdy o tym, jakie urządzenia istnieją, do kogo należą, w jakim są stanie.
- Utrzymuje `Shadow` (patrz PROTOCOL.md sekcja 6) w Postgresie, cache w Valkey.
- **Watchdog presence**: zadanie cykliczne, które promuje urządzenia z `ONLINE`
  do `DISCONNECTED` lub `SLEEPS` na podstawie `last_seen`, `sleep_type`
  i `expected_interval`:

```rust
let deadline = last_seen + expected_interval * TOLERANCE;   // TOLERANCE = 3
let state = if now < deadline           { Online }
            else if sleep_type == Sleep { Sleeps }
            else                        { Disconnected };
```

- **Reconcile komend**: komenda ma stan `pending -> sent -> acked | expired |
  rejected`. Dla urządzeń `SLEEP` komenda czeka w kolejce do najbliższego `Hello`.

---

## 4. `pkpu-provisioning`

Trzy rejestry, świadomie rozdzielone:

| Rejestr | Zawiera | Kto zapisuje |
|---|---|---|
| **fabryczny** | `device_id`, `serial`, `pubkey`, `model`, `hw_rev`, data produkcji | linia produkcyjna przez `pkpu-cli` (mTLS, osobne CA) |
| **operacyjny** | przypisanie do `tenant` / `site` / właściciela | proces claimowania |
| **odwołań** | urządzenia zablokowane (kradzież, RMA, kompromitacja klucza) | operator |

Przepływ claimowania — patrz DEVICE.md sekcja 6.1. Po stronie chmury:

```
POST /provisioning/challenge  { device_id, nonce_device }
   -> sprawdź rejestr fabryczny i rejestr odwołań
   -> sprawdź, czy urządzenie nie jest już claimed przez innego tenanta
   -> zwróć { claim_token, sig_cloud }   (ed25519, ważny 5 min)

POST /provisioning/complete   { device_id, proof }   // podpis urządzenia
   -> weryfikacja podpisu kluczem publicznym z rejestru fabrycznego
   -> wydanie certyfikatu klienta (CA operacyjne, ważność 2 lata, auto-renew)
   -> wpis do rejestru operacyjnego + audit_log
```

**Dwa CA**: fabryczne (offline, klucz w HSM) i operacyjne (online, rotowalne).
Kompromitacja CA operacyjnego nie unieważnia tożsamości sprzętu.

---

## 5. `pkpu-ota`

- Artefakty w S3/MinIO, adresowane hashem: `fw/{model}/{version}/{sha256}.bin`.
- Manifest podpisywany w CI kluczem z HSM — **nigdy** ręcznie na laptopie.
- Kampania:

```rust
struct Campaign {
    id:          CampaignId,
    cohort:      CohortFilter,   // model, hw_rev, from_version, site, tag
    target:      FirmwareVersion,
    waves:       Vec<Wave>,      // [1%, 10%, 50%, 100%]
    gate:        Gate,           // max_failure_pct, min_soak_minutes
    window:      Option<TimeWindow>,
    state:       CampaignState,  // draft|running|paused|done|rolled_back
}
```

- Bramka między falami: jeśli w kohorcie odsetek urządzeń, które nie zgłosiły
  `mark_boot_ok` w `soak` przekracza próg — kampania automatycznie na `paused`.
- Rollback floty: nowa kampania z poprzednią wersją, nie „cofanie" starej.

---

## 6. `pkpu-rules`

Reguły per tenant, przechowywane jako dane (nie kod):

```
WHEN  tel.channel(TEMP) > 30  FOR 5m
AND   device.site = "hala-1"
THEN  notify(email, webhook) , command(device, "set_output", {relay: 1})
```

- Silnik: konsument NATS z oknem czasowym w pamięci + stan w Valkey.
- Wersja 1: reguły progowe i czasowe wystarczą na 90% przypadków.
  DSL/skryptowanie (rhai, WASM) dopiero gdy realnie potrzebne.
- Każde odpalenie reguły -> wpis do `events` + ewentualna komenda przez registry
  (nigdy bezpośrednio na broker — komendy zawsze przez shadow, żeby były w audycie).

---

## 7. `pkpu-api`

- `axum`, OpenAPI generowane z typów (`utoipa`), wersjonowanie ścieżką `/v1/`.
- Autoryzacja: OIDC Bearer JWT. Claim `tenant_id` + role.
- WebSocket `/v1/stream` — subskrypcja live telemetrii i zmian stanu,
  filtr po `site` / `device_id`, backed by NATS.
- Rate limiting `tower-governor` per token.
- Row Level Security w Postgresie: połączenie ustawia `SET LOCAL app.tenant_id`,
  polityki RLS filtrują. Błąd w kodzie API nie może wyciec danych innego tenanta.

Zarys endpointów:

```
GET    /v1/devices?site=&state=&model=
GET    /v1/devices/{id}
PATCH  /v1/devices/{id}/desired        // zapis shadow.desired
POST   /v1/devices/{id}/commands
GET    /v1/devices/{id}/telemetry?ch=&from=&to=&agg=
GET    /v1/sites  /v1/sites/{id}/devices
POST   /v1/provisioning/challenge  |  /complete
GET    /v1/ota/campaigns  |  POST /v1/ota/campaigns
GET    /v1/events?severity=&from=
WS     /v1/stream
```

---

## 8. `pkpu-gateway` (edge)

Binarka na Linux (RPi CM4 / dowolny aarch64), rola:

1. Utrzymuje stos PAN: OpenThread Border Router (Thread) lub koordynator Zigbee
   — oba jako procesy/biblioteki C, sterowane z Rusta przez Spinel/EZSP.
2. Mapuje `short_id` <-> `device_id`, dokleja tożsamość do ramek.
3. Trzyma jedną sesję mTLS do chmury dla całej podsieci.
4. **Store-and-forward**: przy braku internetu buforuje na dysku (sled/redb),
   dosyła z `backfill = true`.
5. Lokalne reguły awaryjne — minimalny podzbiór `pkpu-rules`, żeby instalacja
   działała bez chmury.
6. Sam jest urządzeniem w rejestrze: ma `device_id`, stan, OTA.

---

## 9. Deployment

- **Docelowo**: Kubernetes (k3s wystarczy), Helm/Kustomize, obrazy distroless.
- **Na start**: docker-compose na jednym VPS — Postgres+Timescale, NATS, Valkey,
  MinIO, wszystkie binarki Rust. Migracja do k8s bez zmian w kodzie.
- Migracje bazy: `sqlx migrate`, uruchamiane jako job przed deployem.
- Konfiguracja: zmienne środowiskowe + plik TOML, walidowane przy starcie
  (`figment` + `serde`), fail-fast przy złej konfiguracji.
- CI: `cargo clippy -- -D warnings`, `cargo test`, `cargo deny check`,
  build obrazów, testy integracyjne na `testcontainers`.

---

## 10. Obserwowalność i SLO

| Metryka | Cel |
|---|---|
| ingest p99 latency (od MQTT publish do NATS ack) | < 50 ms |
| dostarczenie komendy do `NONSLEEP` | < 1 s p95 |
| dostępność API | 99.9 % |
| utrata telemetrii | 0 (buforowanie + retransmisja) |
| czas odtworzenia bazy z JetStream | < 30 min |

- `tracing` + OpenTelemetry, `trace_id` propagowany od requestu API do komendy
  i z powrotem do `CommandAck`.
- Metryki floty jako first-class dashboard: rozkład stanów
  (`ONLINE`/`DISCONNECTED`/`SLEEPS`), rozkład wersji firmware, RSSI/LQI,
  poziom baterii, wskaźnik niepowodzeń OTA.
- Alerty na anomalie flotowe (nagły wzrost `DISCONNECTED` w jednym `site`
  = awaria gatewaya lub internetu, nie urządzeń).
