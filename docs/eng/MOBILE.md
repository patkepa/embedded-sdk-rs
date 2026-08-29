# 3. MOBILE — the app and the shared core

The mobile app is not an add-on: for most IoT products it is the **only**
provisioning tool and the main user interface.

---

## 1. The split

```
+-------------------------+   +-------------------------+
|  iOS shell (SwiftUI)    |   | Android shell (Compose) |
|  - views                |   |  - views                |
|  - CoreBluetooth        |   |  - BluetoothLeScanner   |
|  - CoreNFC              |   |  - NfcAdapter           |
+-----------+-------------+   +-------------+-----------+
            |     UniFFI bindings (generated)          |
            +--------------------+---------------------+
                                 |
                    +------------v-------------+
                    |       pkpu-core          |  Rust
                    |  - API client (reqwest)  |
                    |  - WebSocket / live      |
                    |  - provisioning machine  |
                    |  - pkpu-proto (same types)|
                    |  - offline cache (redb)  |
                    |  - auth / OIDC / keychain|
                    +--------------------------+
```

**The dividing rule:** everything that is logic goes into `pkpu-core`.
What stays in the native layer is only the UI and the system APIs Rust has no
access to (BLE, NFC, notifications, keychain).

The gain: the provisioning sequence — the most error-prone part of the app — is
written **once**, tested on the host and identical on both platforms.
On top of that, the same `pkpu-core` powers the desktop CLI and the E2E tests.

---

## 2. BLE — inversion of control

Rust does not drive the BLE radio directly. Instead, `pkpu-core` defines a trait
that the native layer implements:

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

The provisioning state machine in Rust calls these methods; iOS and Android
supply thin adapters. Unit tests substitute a fake and walk through the whole
sequence without hardware.

`NfcTransport` for reading NDEF works the same way.

---

## 3. The provisioning flow (from the user's point of view)

```
1. [Scan]        an NFC tap  or  a list of nearby BLE devices
2. [Select]      confirmation of the model and the serial number
3. [Network]     Wi-Fi: pick an SSID + password
                 Thread/Zigbee: automatically from the user's gateway
4. [Claim]       core -> cloud: challenge/response (see CLOUD.md 4)
5. [Write]       core -> device: creds + a signed claim_token
6. [Wait]        a WS subscription for the device's first report
7. [Done]        name it, assign it to a site/room
```

UX requirements that follow from the architecture:

- Step 6 has a timeout and a **specific** error message at each stage
  (not "something went wrong"): wrong PSK, out of range, the device is already
  claimed by another user, the device is revoked.
- Provisioning has to be resumable: a device in the "has creds, has no claim"
  state must be detectable and repairable without a factory reset.
- For `SLEEP_TYPE = SLEEP` step 6 may take up to one wake-up cycle — the UI has
  to communicate that instead of counting down 30 seconds and reporting failure.

---

## 4. Offline and local mode

- Cache in `redb`: the device list, latest states, configuration — the app opens
  with data, not with a spinner.
- **Local control over BLE** when there is no internet (for devices with BLE):
  the same `Command` frame from `pkpu-proto`, just over a different transport.
  The result is synchronized to the shadow once connectivity returns.
- Conflict resolution: the cloud shadow wins, but a local command with a newer
  timestamp is replayed as `desired` after coming back online.

---

## 5. Authorization

- OIDC Authorization Code + PKCE, in the system browser (not a webview).
- The refresh token lives in the Keychain / Android Keystore, accessed from Rust
  through the `SecureStore` callback trait (the same pattern as BLE).
- Logging out clears the cache and invalidates the token on the IdP side.

---

## 6. Notifications

- APNs / FCM, sent from `pkpu-rules`.
- The mobile device token is registered through the API, tied to `user_id` and
  `tenant_id`, with a TTL and cleanup of dead tokens.
- Minimal payload (the event id); the content is fetched from the API —
  measurement data does not travel through intermediaries.

---

## 7. Alternative: Flutter / a single UI

If the resources for two native apps are too tight, one option is Flutter with
`pkpu-core` through `flutter_rust_bridge`. It keeps the same split (logic in
Rust) and costs in the maturity of the BLE/NFC integration.
The decision is open — see [DECISIONS.md](DECISIONS.md).
