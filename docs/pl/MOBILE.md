# 3. MOBILE — aplikacja i rdzeń współdzielony

Aplikacja mobilna nie jest dodatkiem: dla większości produktów IoT jest
**jedynym** narzędziem provisioningu i głównym interfejsem użytkownika.

---

## 1. Podział

```
+-------------------------+   +-------------------------+
|  iOS shell (SwiftUI)    |   | Android shell (Compose) |
|  - widoki               |   |  - widoki               |
|  - CoreBluetooth        |   |  - BluetoothLeScanner   |
|  - CoreNFC              |   |  - NfcAdapter           |
+-----------+-------------+   +-------------+-----------+
            |     UniFFI bindings (generowane)         |
            +--------------------+---------------------+
                                 |
                    +------------v-------------+
                    |       pkpu-core          |  Rust
                    |  - klient API (reqwest)  |
                    |  - WebSocket / live      |
                    |  - maszyna provisioningu |
                    |  - pkpu-proto (te typy)  |
                    |  - cache offline (redb)  |
                    |  - auth / OIDC / keychain|
                    +--------------------------+
```

**Zasada podziału:** wszystko, co jest logiką, idzie do `pkpu-core`.
W warstwie natywnej zostaje wyłącznie UI i API systemowe, których Rust nie ma
dostępu (BLE, NFC, powiadomienia, keychain).

Zysk: sekwencja provisioningu — najbardziej podatna na błędy część aplikacji —
jest napisana **raz**, testowana na hoście, i identyczna na obu platformach.
Dodatkowo ten sam `pkpu-core` obsługuje CLI desktopowe i testy E2E.

---

## 2. BLE — inwersja sterowania

Rust nie steruje radiem BLE bezpośrednio. Zamiast tego `pkpu-core` definiuje
trait, który implementuje warstwa natywna:

```rust
#[uniffi::export(callback_interface)]
pub trait BleTransport: Send + Sync {
    fn scan(&self, service_uuid: String) -> Vec<BleDevice>;
    fn connect(&self, id: String) -> Result<(), BleError>;
    fn read(&self, char_uuid: String) -> Result<Vec<u8>, BleError>;
    fn write(&self, char_uuid: String, data: Vec<u8>) -> Result<(), BleError>;
    fn subscribe(&self, char_uuid: String, sink: Arc<dyn BleSink>);
    fn disconnect(&self);
}
```

Maszyna stanów provisioningu w Rust wywołuje te metody; iOS i Android
dostarczają cienkie adaptery. Testy jednostkowe podstawiają fake'a i przechodzą
całą sekwencję bez sprzętu.

Analogicznie `NfcTransport` dla odczytu NDEF.

---

## 3. Przepływ provisioningu (widok użytkownika)

```
1. [Skanuj]      NFC tap  albo  lista urządzeń BLE w pobliżu
2. [Wybierz]     potwierdzenie modelu i numeru seryjnego
3. [Sieć]        Wi-Fi: wybór SSID + hasło
                 Thread/Zigbee: automatycznie z gatewaya użytkownika
4. [Claim]       core -> chmura: challenge/response (patrz CLOUD.md 4)
5. [Zapis]       core -> urządzenie: creds + podpisany claim_token
6. [Czekaj]      subskrypcja WS na pierwszy raport urządzenia
7. [Gotowe]      nazwa, przypisanie do site/pomieszczenia
```

Wymagania UX wynikające z architektury:

- Krok 6 ma timeout i **konkretny** komunikat błędu na każdym etapie
  (nie „coś poszło nie tak"): zły PSK, brak zasięgu, urządzenie już claimowane
  przez innego użytkownika, urządzenie odwołane.
- Provisioning musi być wznawialny: urządzenie w stanie „ma creds, nie ma claimu"
  jest wykrywalne i naprawialne bez factory resetu.
- Dla `SLEEP_TYPE = SLEEP` krok 6 może trwać do jednego cyklu wybudzenia —
  UI musi to komunikować, a nie odliczać 30 sekund i zgłaszać porażkę.

---

## 4. Tryb offline i lokalny

- Cache w `redb`: lista urządzeń, ostatnie stany, konfiguracja — aplikacja
  otwiera się z danymi, nie ze spinnerem.
- **Sterowanie lokalne przez BLE** przy braku internetu (dla urządzeń z BLE):
  ta sama ramka `Command` z `pkpu-proto`, tylko innym transportem.
  Wynik synchronizowany do shadow, gdy wróci łączność.
- Rozstrzyganie konfliktów: shadow w chmurze wygrywa, ale lokalna komenda
  z nowszym timestampem jest odtwarzana jako `desired` po powrocie online.

---

## 5. Autoryzacja

- OIDC Authorization Code + PKCE, przeglądarka systemowa (nie webview).
- Refresh token w Keychain / Android Keystore, dostęp z Rusta przez callback
  trait `SecureStore` (ten sam wzorzec co BLE).
- Wylogowanie czyści cache i unieważnia token po stronie IdP.

---

## 6. Notyfikacje

- APNs / FCM, wysyłane z `pkpu-rules`.
- Token urządzenia mobilnego rejestrowany przez API, powiązany z `user_id`
  i `tenant_id`, TTL i czyszczenie martwych tokenów.
- Payload minimalny (id zdarzenia); treść dociągana z API — dane pomiarowe
  nie idą przez pośredników.

---

## 7. Alternatywa: Flutter / jeden UI

Jeżeli zasoby na dwie natywne aplikacje są zbyt małe, wariantem jest Flutter
z `pkpu-core` przez `flutter_rust_bridge`. Zachowuje ten sam podział
(logika w Rust), kosztuje na dojrzałości integracji BLE/NFC.
Decyzja otwarta — patrz [DECISIONS.md](DECISIONS.md).
