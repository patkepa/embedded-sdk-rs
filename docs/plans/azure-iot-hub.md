# Azure IoT Hub Integration Plan

## Status

- Status: Proposed
- Branch: `feat/cloud-iot-hub`
- Initial target: Seeed Studio XIAO ESP32C6
- Cloud service: Azure IoT Hub
- Device protocol: MQTT 3.1.1 over authenticated TLS
- Follow-up service: Azure IoT Hub Device Provisioning Service (DPS)
- Primary outcome: add a portable, bounded, recoverable Azure IoT device path
  without coupling cloud behavior to an MCU, network stack, MQTT backend, or
  product payload schema

## Executive recommendation

Implement Azure IoT Hub as a provider layer above a version-aware portable
MQTT boundary. Do not add Azure topic strings, credentials, or twin behavior
directly to the XIAO firmware or to the generic MQTT implementation.

The implementation should be divided into these independently testable layers:

1. `embedded-sdk-mqtt` evolves from an MQTT-5-only model into common MQTT
   contracts with explicit MQTT 3.1.1 and MQTT 5 session options.
2. A new backend adapter supplies allocation-free MQTT 3.1.1 over an existing
   `embedded-io-async` byte stream. The current `minimq` MQTT 5 adapter remains
   supported.
3. A security boundary supplies trusted time, random data, trust anchors,
   credential leases, SAS signing, and opaque private-key operations without
   exposing secrets through configuration or diagnostics.
4. `embedded-sdk-cloud-core` owns only provider-independent cloud lifecycle,
   capability, and error concepts that are already proven useful.
5. `embedded-sdk-cloud-azure-iot` owns Azure identities, MQTT username and
   topic construction, response parsing, request correlation, IoT Hub state
   machines, and Azure-specific events.
6. A dedicated Azure reference firmware composes Wi-Fi, DNS, TCP, TLS, MQTT,
   credentials, storage, queues, and application policy.
7. DPS is added as a separate transient provisioning state machine after
   direct IoT Hub connectivity is stable.

The first vertical slice should send one bounded telemetry message to a real
IoT Hub over verified TLS, recover from a forced network interruption, and
report distinct network, TLS, MQTT, authentication, and service failures. It
must not be described as production-ready until the security, persistence,
resource, interoperability, and hardware gates in this plan pass.

## Current repository constraints

The repository already has useful foundations:

- portable Wi-Fi state and an ESP32-C6 station implementation;
- DHCPv4, DNS, and TCP through `embassy-net`;
- a portable, allocation-free MQTT model;
- a `minimq` adapter over `embedded-io-async` streams;
- bounded MQTT buffers and reconnect behavior in the XIAO reference firmware;
- portable key/value storage contracts;
- service lifecycle and telemetry primitives.

The following gaps prevent a direct Azure integration today:

1. Azure IoT Hub device endpoints use MQTT 3.1.1. The existing adapter emits
   MQTT 5 and therefore cannot be used for IoT Hub unchanged.
2. The portable MQTT `Config` currently contains MQTT-5-specific session
   expiry and maximum-packet semantics without identifying a protocol version.
3. The current MQTT client identifier is limited to 64 bytes. Azure IoT Hub
   device identities and symmetric-key DPS registration identities can be up
   to 128 characters.
4. The reference MQTT transport is an explicit plaintext fixture. IoT Hub
   requires TLS and does not accept insecure MQTT on port 1883.
5. There is no portable security crate, trusted wall-clock policy, production
   trust store, client-certificate integration, or protected credential
   backend.
6. Portable storage exists, but the XIAO board does not yet advertise a
   reviewed persistent-storage partition suitable for credentials and a DPS
   assignment record.
7. There is no persistent outbound telemetry queue. MQTT QoS 1 must not be
   presented as power-loss-safe delivery.

These gaps are prerequisites and explicit support boundaries, not reasons to
move cloud behavior into platform-specific code.

## Azure protocol constraints

The design must encode and test the service's actual constraints:

- IoT Hub accepts MQTT 3.1.1 on port 8883 and MQTT 3.1.1 over WebSocket on
  port 443.
- All device traffic must use TLS. The classic
  `<hub>.azure-devices.net` endpoint is the production baseline and uses TLS
  1.2. TLS 1.3-capable device endpoints currently exist as a preview and must
  remain an opt-in experiment until generally available and validated.
- IoT Hub is not a general-purpose MQTT broker. Devices use fixed topic
  patterns for telemetry, cloud-to-device messages, twins, and direct methods.
- IoT Hub supports QoS 0 and QoS 1 but not QoS 2.
- Only one active MQTT connection is permitted per device identity. A new
  connection replaces the previous connection.
- A device can subscribe to at most five topics. The planned complete feature
  set uses four: cloud-to-device messages, twin responses, desired-property
  patches, and direct methods.
- Persistent sessions are important for queued cloud-to-device messages.
  IoT Hub connections should use `CleanSession=false` after the client has the
  required subscription behavior.
- Direct methods and twin operations use request identifiers in the topic and
  require response correlation.
- Desired-property change notifications are emitted only while the device is
  connected. A reconnect must perform a full twin read before treating desired
  state as synchronized.
- SAS tokens are scoped, signed, expiring credentials. They require trusted
  Unix time and must be replaced before expiration.
- X.509 device authentication uses mutual TLS. For CA-signed device
  certificates, the leaf certificate common name must match the device ID.
- Azure may rotate service leaf and intermediate certificates. The SDK must
  validate a replaceable set of root trust anchors rather than pinning a leaf
  certificate.

Device-side resource limits should normally be much smaller than Azure's
service limits. For example, the SDK must not reserve a 256 KiB MQTT packet
only because IoT Hub can accept a message of that size.

## Goals

The Azure IoT Hub work should provide:

- a `no_std`, allocation-free provider implementation;
- MQTT 3.1.1 over a generic asynchronous byte stream;
- server-authenticated TLS with hostname verification and SNI;
- development SAS-token authentication and a production X.509 path;
- bounded device-to-cloud telemetry;
- cloud-to-device messages with an explicit acceptance policy;
- direct-method request and response handling;
- device twin retrieval, reported-property updates, and desired-property
  patches;
- reconnect, session restoration, credential refresh, and twin resynchronizing
  state machines;
- stable, redacted lifecycle and error telemetry;
- a dedicated XIAO ESP32-C6 reference firmware;
- host, interoperability, security-negative, and hardware-in-the-loop tests;
- an architecture reusable by a later AWS IoT provider without forcing Azure
  and AWS device-management semantics into one API.

## Non-goals for the first implementation

The first Azure work must not include or claim:

- a general-purpose Azure service-side management SDK;
- Azure portal, Azure CLI, ARM, or backend application functionality;
- MQTT over WebSocket;
- AMQP or HTTPS device transports;
- IoT Edge module or gateway behavior;
- Azure IoT Plug and Play model parsing;
- Device Update for IoT Hub;
- file upload support;
- TPM attestation;
- automatic device certificate renewal;
- durable telemetry across reset or power loss;
- OTA, rollback, or remote factory-reset implementation;
- an abstraction that makes Azure twins and AWS shadows appear identical;
- production secret storage in ordinary unprotected flash;
- silently accepting a TLS connection without hostname or certificate-time
  validation.

## Dependency architecture

Dependencies continue to point inward toward portable contracts:

```text
firmware/seeed/xiao-esp32c6-azure-iot
    |
    +-- product telemetry and command handlers
    +-- bounded Embassy channels and supervision policy
    +-- credential and persistent-record ownership
    |
    +-- embedded-sdk-cloud-azure-iot
    |       |
    |       +-- embedded-sdk-cloud-core
    |       +-- embedded-sdk-mqtt
    |
    +-- embedded-sdk-mqtt-<v311-backend>
    +-- selected TLS adapter
    +-- embedded-sdk-security
    +-- embedded-sdk-networking-embassy-net
    +-- embedded-sdk-storage
    +-- ESP32-C6 port and XIAO board package
```

The provider crate must not depend on Embassy, `embassy-net`, Espressif crates,
the selected TLS library, or a concrete MQTT implementation. It operates on
portable MQTT session capabilities and caller-owned buffers.

The MQTT protocol crate must not contain Azure topic strings or Azure identity
rules. The security crate must not contain an Azure IoT state machine. Product
schemas and business commands remain in firmware or product crates.

## Package plan

### `embedded-sdk-mqtt`

Evolve `crates/mqtt` while retaining its portable, allocation-free role.

Add these concepts:

- `ProtocolVersion::{V3_1_1, V5}`;
- `CommonConfig` for hostname, port, client ID, keepalive, and local packet
  capacity;
- `V311SessionConfig` for `clean_session`;
- `V5SessionConfig` for `clean_start`, session expiry, and MQTT 5 properties;
- a version-tagged `SessionConfig` or a builder that makes mixing version-only
  options impossible;
- backend capability reporting;
- a narrow asynchronous `MqttSession` contract once the second backend proves
  its exact required shape.

The session contract needs, at minimum:

- connect with borrowed username/password data;
- subscribe and expose the granted QoS;
- publish at QoS 0 and QoS 1;
- receive a topic and borrowed payload without allocation;
- expose session-present/fresh-session status;
- preserve or explicitly reject retained in-flight state on reconnect;
- disconnect cleanly;
- allow an inbound QoS 1 message to be acknowledged only after the owning
  service has accepted it, if the selected backend supports manual ACK;
- normalize configuration, capacity, transport, peer, disconnected,
  authentication, and protocol errors while retaining backend detail below
  the facade.

Do not emulate MQTT 5 session expiry on MQTT 3.1.1. The provider should state
the desired persistent-session behavior and the backend should encode the
correct version-specific wire behavior.

The current `embedded-sdk-mqtt-minimq` adapter remains the MQTT 5 adapter.
Azure support must not silently change its packet format.

### MQTT 3.1.1 backend selection

Use a time-boxed proof before adding a permanent dependency. Candidate
implementations must be evaluated against the following mandatory criteria:

- `no_std` and no required global allocator;
- builds on the workspace's pinned stable toolchain;
- asynchronous and cancellable operation;
- transport-independent operation, preferably using
  `embedded-io-async` 0.7 directly;
- MQTT 3.1.1 CONNECT, CONNACK, SUBSCRIBE, SUBACK, PUBLISH, PUBACK, PING, and
  DISCONNECT support;
- QoS 0 and QoS 1;
- persistent session and `session_present` support;
- bounded, caller-visible RX, TX, and in-flight storage;
- no secret-bearing packet logging in default features;
- control over inbound acknowledgment or a documented safe acceptance point;
- tolerance of IoT Hub closing the connection instead of returning rich MQTT
  errors;
- host tests against a strict MQTT 3.1.1 broker;
- successful authentication and telemetry publish to a real IoT Hub;
- acceptable maintenance status, license, unsafe-code boundary, and
  transitive dependency graph.

`mqtt-async-embedded` is a candidate because it currently advertises MQTT
3.1.1 and MQTT 5, `no_std`, no allocation, and Embassy-oriented operation. It
is not selected by this plan. Source review and the proof gates above are the
selection mechanism.

Extending or forking `minimq` to support MQTT 3.1.1 should be considered only
if maintained ecosystem options fail the proof. Reimplementing MQTT framing in
this repository remains out of scope.

### `embedded-sdk-security`

Create `crates/security` as a portable capability boundary, not a TLS stack.
It should define focused traits or values for:

- trusted Unix time with an explicit `untrusted` state;
- cryptographically secure random bytes;
- a replaceable root trust store or trust-anchor provider;
- opaque private-key signing suitable for software keys or a future secure
  element;
- device certificate chains;
- secret byte buffers with redacted `Debug` and zeroization on drop;
- credential leases with an absolute expiry and refresh margin;
- authentication failure and key-access error categories.

Prefer established ecosystem crypto traits where they express the requirement
without weakening it. Do not introduce an SDK-specific AES, hash, signature,
or random-number API merely to rename an existing standard trait.

Trusted-time policy requires its own ADR. At minimum, the design must address:

- certificate validity checks during TLS;
- SAS expiry calculation;
- devices without a battery-backed real-time clock;
- a persisted last-known-good lower bound that never moves backward silently;
- SNTP or another time source after IP becomes ready;
- behavior when neither persisted nor network time is trustworthy;
- tolerance for clock skew without accepting indefinitely valid credentials.

Production TLS and SAS authentication must fail closed while time is
untrusted. A development-only clock shortcut must be named explicitly and
must not be enabled by the production feature set.

### TLS adapter

Select a TLS implementation through a separate proof. The resulting stream
must implement the established asynchronous I/O traits consumed by the MQTT
backend.

The production baseline must prove:

- TLS 1.2 support for the classic IoT Hub endpoint;
- SNI with the exact configured hub hostname;
- hostname verification;
- full chain validation against an explicit trust set;
- certificate validity checks using trusted time;
- at least one Azure-supported ECDHE RSA or ECDHE ECDSA cipher suite;
- cryptographically strong client randomness from the platform;
- bounded record and handshake buffers;
- handshake cancellation and timeout;
- optional mutual TLS using an opaque private key;
- rejection of an unknown CA, wrong hostname, expired/not-yet-valid
  certificate, missing SNI, incompatible cipher, and tampered handshake;
- a trust-store update strategy that does not pin Azure leaf or intermediate
  certificates.

TLS 1.3 support may be tested against the preview
`<hub>.device.azure-devices.net` endpoint, but it is not a substitute for the
TLS 1.2 production baseline while that endpoint remains preview.

### `embedded-sdk-cloud-core`

Create `crates/cloud-core`. Keep it intentionally small until at least two
provider implementations demonstrate shared behavior.

Initial concepts may include:

```rust
pub enum CloudState {
    Disabled,
    WaitingForNetwork,
    WaitingForTime,
    Provisioning,
    ResolvingEndpoint,
    ConnectingTransport,
    AuthenticatingTransport,
    ConnectingProtocol,
    Synchronizing,
    Online,
    RefreshingCredentials,
    BackingOff,
    Failed,
}

pub enum CloudErrorKind {
    Configuration,
    Capacity,
    Network,
    Time,
    Tls,
    Authentication,
    Protocol,
    ServiceRejected,
    Throttled,
    Storage,
    Application,
}
```

It may also contain a compact snapshot, capability bits, and bounded generic
correlation identifiers. It should not initially contain `Twin`, `Shadow`,
`DirectMethod`, `Job`, or provider topic types.

Do not introduce one large `CloudClient` trait. Use provider-specific clients
and small application ports only when firmware needs to substitute one
provider implementation for another.

### `embedded-sdk-cloud-azure-iot`

Create `crates/cloud-azure-iot`. It remains `no_std`, allocation-free, and
independent of an executor or platform.

Suggested modules:

```text
src/
    lib.rs
    config.rs          bounded hub, device, module, and API-version values
    connection.rs      client ID, username, and authentication parameters
    telemetry.rs       D2C topic and property-bag encoding
    c2d.rs             C2D topic parsing and metadata decoding
    methods.rs         method request parsing and response topics
    twin.rs            twin topics, status parsing, and version tracking
    requests.rs        bounded request-ID allocation and correlation
    client.rs          provider state machine over MqttSession
    error.rs           Azure-specific errors and normalized mappings
    snapshot.rs        redacted health counters
```

The exact file split should follow implementation size. It is listed here to
make ownership clear, not to require empty modules.

The provider configuration should contain only non-secret identity and policy:

```rust
pub struct HubConfig {
    pub endpoint: HubEndpoint,
    pub device_id: DeviceId,
    pub module_id: Option<ModuleId>,
    pub keep_alive_seconds: u16,
    pub maximum_packet_size: u32,
    pub enabled_features: HubCapabilities,
}
```

Credentials are supplied at connection time through an authentication
provider. They must not be fields that derive `Debug`, persist accidentally,
or appear in lifecycle snapshots.

Conceptual application events:

```rust
pub enum HubEvent<'a> {
    CloudToDevice(CloudToDeviceMessage<'a>),
    DirectMethod(DirectMethodRequest<'a>),
    DesiredPropertiesPatch(DesiredPropertiesPatch<'a>),
    TwinResponse(TwinResponse<'a>),
}
```

Payloads are borrowed byte slices. Azure's JSON requirement for twin and
method payloads is validated at the provider boundary only to the extent
required to safely classify and correlate the operation. Product property
schemas are not interpreted by this crate.

## Azure IoT Hub connection parameters

For a device identity, the provider generates:

- MQTT host: configured IoT Hub fully qualified hostname;
- TCP port: 8883;
- MQTT protocol: 3.1.1;
- client ID: `{device-id}`;
- username:
  `{hub-hostname}/{device-id}/?api-version=2021-04-12`;
- password for SAS authentication: a short-lived device-scoped SAS token;
- password for X.509 authentication: absent;
- clean session: false for the normal IoT Hub session.

For a module identity, the provider later generates the documented device and
module variants. Module support should not complicate the first device-only
slice.

The API version is an SDK-owned compatibility constant. Arbitrary user input
must not select an untested Azure API version. Updating it requires golden
tests and live interoperability validation.

## Authentication design

Model authentication as a connection-time capability:

```rust
pub enum HubAuthentication<'a, S, X> {
    Sas(&'a mut S),
    X509(&'a X),
}
```

Here, `S` implements `SasCredentialProvider` and `X` implements
`ClientCertificateIdentity`. The exact implementation may instead use focused
trait objects if they remain object-safe and allocation-free.

### SAS path

The SAS provider should:

- accept a device-scoped symmetric key through a secret-owning abstraction;
- obtain trusted Unix time;
- lower-case and percent-encode the resource URI as required by Azure;
- compute HMAC-SHA256 over the encoded resource URI and expiry;
- base64-encode and percent-encode the signature into caller-owned output;
- produce an absolute expiry and refresh deadline;
- zeroize intermediate decoded keys, signatures, and token buffers;
- never log a token, signature, key, connection string, or MQTT password;
- support primary/secondary key rotation without changing the public cloud API;
- schedule a controlled reconnect before token expiry.

Externally generated, short-lived SAS tokens may be used for the first
development fixture. They must enter through a redacted runtime or flashing
mechanism, must never be committed, and must not cause the compatibility matrix
to claim production authentication.

### X.509 path

The production path should:

- provide a leaf certificate and required intermediate chain;
- keep the private key opaque to the cloud and MQTT crates;
- support a software signer initially only if storage is protected and
  reviewed;
- allow a future secure-element implementation without changing Azure APIs;
- validate that identity provisioning makes the certificate common name match
  the registered device ID;
- support overlapping old/new credentials for rotation;
- distinguish service-root trust anchors from the device's client identity.

CA-signed X.509 is the recommended production identity. Symmetric keys remain
useful for development and constrained manufacturing flows, but must not be
shared across a fleet.

## IoT Hub service state machine

The dedicated cloud task owns the connection and runs this explicit state
machine:

```text
Disabled
  -> WaitingForNetwork
  -> WaitingForTrustedTime
  -> LoadingCredential
  -> ResolvingHub
  -> ConnectingTcp
  -> AuthenticatingTls
  -> ConnectingMqtt311
  -> Subscribing
  -> SynchronizingTwin
  -> Online
       |-> PublishingTelemetry
       |-> DispatchingCloudToDevice
       |-> DispatchingDirectMethod
       |-> ApplyingDesiredProperties
       |-> RefreshingCredential
  -> BackingOff
  -> WaitingForNetwork / WaitingForTrustedTime / LoadingCredential
```

State transitions must identify the failing layer. A DNS failure is not a TLS
failure, an MQTT CONNACK authentication failure is not a Wi-Fi failure, and an
Azure 429 response is not a transport failure.

Recovery policy:

- link or IP loss cancels DNS, TCP, TLS, and MQTT work and returns to
  `WaitingForNetwork`;
- loss of trusted time prevents new TLS or SAS sessions but does not rewrite
  network state;
- DNS, TCP, TLS, and transient MQTT errors use bounded exponential backoff with
  hardware-derived jitter;
- Azure `retry-after` takes precedence over local retry delay;
- an authentication rejection attempts the alternate credential slot or
  refresh path before normal transient retry;
- repeated authentication failure may request reprovisioning, but only after a
  bounded policy prevents a transient outage from erasing a valid assignment;
- successful online operation resets transient backoff after a configured
  stability interval rather than immediately on CONNACK;
- the task never restarts Wi-Fi, BLE, or unrelated application services solely
  because cloud connectivity failed.

## Telemetry flow

Product producers send bounded messages to the cloud task rather than owning
the MQTT connection:

```text
producer -> bounded channel -> Azure topic encoder -> MQTT publish -> PUBACK
```

Each queue entry should contain:

- payload bytes or a reference to an exclusively owned bounded buffer;
- QoS policy;
- content type and encoding where required;
- bounded application properties;
- expiration or staleness policy;
- a local correlation token for completion reporting.

Queue policy must be explicit per message class:

- replace or drop stale best-effort telemetry and increment a counter;
- reject enqueue when a reliable response cannot be retained;
- never block a safety-critical task indefinitely;
- report MQTT acknowledgment separately from application-level cloud
  processing;
- state clearly that RAM-only queue entries do not survive reset or power
  failure.

The telemetry topic encoder owns Azure property-bag percent encoding. It should
provide helpers for JSON content metadata without requiring that all telemetry
be JSON.

## Cloud-to-device message flow

Subscribe to:

```text
devices/{device-id}/messages/devicebound/#
```

The provider parses the URL-encoded property bag into bounded borrowed views.
It must reject malformed encodings without panicking or reading beyond the
packet.

IoT Hub MQTT does not support the AMQP-style reject operation. The MQTT backend
selection must therefore document exactly when PUBACK is sent. Preferred
behavior is:

1. receive the QoS 1 packet;
2. validate its topic and bounded size;
3. copy or transfer it into an application-owned bounded queue;
4. send PUBACK only after successful acceptance.

If the selected MQTT backend always acknowledges before application
acceptance, cloud-to-device messaging must remain an experimental capability
and its weaker loss behavior must be documented. Queue exhaustion must never
be reported as successful application delivery.

## Direct method flow

Subscribe to:

```text
$iothub/methods/POST/#
```

For each request:

1. parse and validate the method name and `$rid` from the topic;
2. reject a request that exceeds the configured payload or identifier bound;
3. reserve one bounded in-flight method slot;
4. dispatch a borrowed or copied request to the product handler;
5. require the handler to return an integer status and JSON or empty response;
6. publish the response to
   `$iothub/methods/res/{status}/?$rid={request-id}`;
7. release the in-flight slot only after publish completion or a terminal
   timeout.

The application owns command authorization and safety policy. The Azure crate
only proves that the request arrived on an authenticated device session and
correlates the response.

Method handler deadlines, concurrency, queue depth, and overload response must
be compile-time-visible resource choices. A default of one in-flight method is
appropriate for the first embedded slice.

## Device twin flow

The full feature uses two subscriptions:

```text
$iothub/twin/res/#
$iothub/twin/PATCH/properties/desired/#
```

The provider owns:

- bounded request-ID allocation;
- pending GET and reported-PATCH correlation;
- response status and `$version` parsing;
- desired-property patch version tracking;
- detection of a skipped, stale, duplicate, or malformed version;
- a `synchronized` flag that becomes true only after a complete twin GET is
  successfully handed to the application;
- a forced full GET after every connection whose desired-state continuity
  cannot be proven.

The product owns:

- JSON schema and semantic validation;
- deciding which desired properties are authorized;
- applying changes transactionally;
- mapping application state into reported-property JSON;
- persisting locally applied configuration where required.

A desired patch must not be marked applied merely because it was received.
Reported properties should describe the result after application validation
and activation.

## DPS provisioning follow-up

Add DPS only after direct Hub connectivity is proven. Prefer a separate
`embedded-sdk-cloud-azure-dps` crate if its implementation and feature set
would otherwise force unused provisioning code into every Hub-only build.

DPS reuses MQTT 3.1.1, TLS, credential, topic, and bounded parsing
foundations, but has a separate state machine:

```text
Unprovisioned
  -> WaitingForNetwork
  -> WaitingForTrustedTime
  -> ConnectingToGlobalEndpoint
  -> Authenticating
  -> SubscribingForResponses
  -> Registering
  -> WaitingRetryAfter
  -> PollingOperation
  -> Assigned
  -> PersistingAssignment
  -> DisconnectingDps
  -> StartingHubSession
```

DPS-specific rules include:

- endpoint `global.azure-devices-provisioning.net` by default;
- MQTT 3.1.1 over TLS 1.2;
- `CleanSession=true`, because DPS does not support persistent sessions;
- client ID equal to the registration ID;
- username containing ID scope, registration ID, and the tested DPS API
  version;
- SAS policy name `registration` for symmetric-key attestation;
- subscription `$dps/registrations/res/#`;
- registration request and operation-status polling topics;
- strict use of server `retry-after`;
- X.509 and symmetric-key attestation in the MQTT path;
- no TPM-over-MQTT support claim.

The assignment record should contain at least:

- record schema version;
- assigned hub hostname;
- assigned device ID;
- provisioning ID scope or enrollment identity reference;
- credential generation/slot identifier, never the raw credential;
- successful assignment timestamp if trusted time exists;
- integrity/authentication metadata supplied by the protected backend.

Persistence must use stable `Key` identifiers from `embedded-sdk-storage`.
The owning component reserves a namespace before publishing the format. A
future record schema migration must be tested across resets and interrupted
writes.

## Reference firmware

Create a dedicated package:

```text
firmware/seeed/xiao-esp32c6-azure-iot/
```

Do not add Azure policy to the existing general XIAO MQTT fixture. The
dedicated firmware should reuse the board and ESP32-C6 port while owning:

- network stack resources and random seed;
- the network runner and Wi-Fi supervisor;
- trusted-time acquisition and policy;
- TLS record and handshake buffers;
- MQTT RX, TX, replay, and in-flight buffers;
- the Azure client state machine;
- telemetry and inbound application channels;
- credential acquisition and refresh;
- persistent assignment access;
- development configuration inputs;
- task timeouts, retry limits, and watchdog health;
- redacted diagnostic output.

The first firmware proof sends a versioned heartbeat payload and does not
accept remote state-changing commands. Subsequent slices add one harmless
direct method, one reported property, and one desired property before exposing
general product hooks.

Development input names should distinguish public configuration from secrets:

- `AZURE_IOT_HUB_HOSTNAME`;
- `AZURE_IOT_DEVICE_ID`;
- `AZURE_IOT_AUTH_MODE`;
- a runtime/injected credential reference rather than a logged connection
  string;
- a separately named development-only short-lived SAS-token input;
- optional DPS ID scope and registration ID only after the DPS phase.

Partial configuration must fail closed while leaving Wi-Fi, BLE, heartbeat,
and local device functionality operational.

## Resource budget

Every buffer is caller-owned or compile-time bounded. Record measured values
for the reference target rather than copying Azure's maximum service sizes.

| Resource | Initial policy | Evidence required |
| --- | --- | --- |
| MQTT RX packet | Derived from largest enabled inbound feature | Golden maximum-size packet and one-byte overflow test |
| MQTT TX/replay | Telemetry plus largest method/twin response and QoS metadata | Reconnect replay test |
| TCP RX/TX | Selected from measured throughput and TLS behavior | HIL throughput and loss test |
| TLS records | Smallest value supported by the TLS backend and Azure | Successful handshake and sustained MQTT traffic |
| TLS handshake workspace | Statically or explicitly allocated | Peak RAM measurement |
| Telemetry queue | Product-selected depth and payload bound | Full-queue policy test |
| C2D queue | At least one complete accepted message | ACK/queue-exhaustion test |
| Direct methods | Initially one in flight | Timeout and overload test |
| Twin requests | Small fixed pending table | Request-ID wrap and exhaustion test |
| Topic scratch | Longest exact encoded Azure topic | Boundary and percent-encoding tests |
| SAS scratch | Longest device-scoped token | Zeroization and overflow tests |
| Trust anchors | Multiple roots with update room | Rotation fixture |
| Task allocation | One cloud owner task plus existing runners | Embassy spawn and stack measurement |

Track:

- firmware flash delta;
- static RAM delta;
- peak heap, with a goal of no new cloud-layer heap requirement;
- task stack/high-water mark where measurable;
- handshake duration;
- reconnect duration;
- sustained publish rate;
- energy impact of keepalive and reconnect;
- queue drops and maximum queue residence time.

The XIAO firmware currently reserves four network sockets for DHCP, DNS, a
probe, and MQTT. The Azure firmware must recalculate this count for overlapping
SNTP, DNS, MQTT, and any connectivity probe rather than assuming four remains
sufficient.

## Error model and observability

Stable diagnostics must preserve failure domains:

- Wi-Fi association;
- IP configuration;
- DNS;
- trusted time;
- TCP;
- TLS negotiation and verification;
- local credential generation or key access;
- MQTT protocol and broker response;
- Azure service status;
- throttling and retry-after;
- application queue or handler failure;
- persistent assignment failure.

An Azure snapshot should include bounded counters such as:

- fresh connections and resumed sessions;
- TLS failures;
- authentication failures;
- MQTT disconnects;
- telemetry accepted, acknowledged, failed, and dropped;
- inbound messages accepted and rejected for capacity;
- direct methods received, responded, and timed out;
- twin GET/PATCH success and failure;
- desired-property resynchronizations;
- throttled responses;
- credential refreshes;
- DPS attempts and assignments after DPS is implemented.

Snapshots and logs must never contain:

- shared keys;
- SAS tokens or signatures;
- MQTT passwords;
- private keys;
- complete connection strings;
- certificate private material;
- arbitrary inbound payloads unless a separately reviewed debug fixture opts
  in.

Use stable numeric telemetry event codes only after reserving component ranges.
Backend debug values can remain available below the portable facade if they
are redacted and do not become compatibility contracts.

## Testing strategy

### Unit tests

Run on the host without networking:

- every bounded identifier at empty, maximum, and maximum-plus-one lengths;
- Azure device ID character rules;
- MQTT username generation for device and later module identities;
- every publish and subscription topic;
- property-bag percent encoding and decoding;
- malformed percent encodings and UTF-8;
- request-ID allocation, wrap, collision, and pending-table exhaustion;
- direct-method request and response topic parsing;
- twin status, request ID, and version parsing;
- duplicate, stale, and skipped desired-property versions;
- SAS resource URI, HMAC, base64, encoding, and expiry golden vectors;
- secret redaction and zeroization where observable safely;
- state transition and error normalization tables;
- backoff saturation and service retry-after precedence.

### Property and fuzz tests

Add fuzz targets for:

- inbound Azure topic classification;
- URL/property-bag decoding;
- twin response query parsing;
- direct-method method-name and request-ID parsing;
- DPS response topics and JSON envelope parsing;
- bounded encoders proving no partial-success truncation.

Assertions include no panic, no out-of-bounds access, no secret disclosure, and
round-trip behavior for canonical encodings.

### MQTT interoperability tests

Against an ephemeral strict MQTT 3.1.1 broker:

- clean and persistent sessions;
- QoS 0 and QoS 1 publish;
- delayed PUBACK and reconnect replay;
- subscription restoration;
- server disconnect at every protocol phase;
- malformed CONNACK/SUBACK/PUBLISH packets;
- inbound packet larger than the configured capacity;
- credential replacement between reconnects;
- cancellation and timeouts without corrupting reusable state.

### TLS tests

Use controlled certificates to prove acceptance of the valid fixture and
rejection of:

- an unknown root;
- a wrong hostname;
- an expired certificate;
- a not-yet-valid certificate;
- missing or wrong SNI;
- an unsupported cipher suite;
- a truncated or tampered handshake;
- an invalid client certificate or signature;
- untrusted local time.

### Live Azure tests

Live tests are opt-in and use an isolated test hub and device identity. They
must never run with production credentials in ordinary CI.

Validate:

- SAS and X.509 connection paths independently;
- telemetry with content type, encoding, and application properties;
- forced replacement by a second connection using the same device identity;
- persistent-session behavior across reconnect;
- cloud-to-device delivery and acknowledgment behavior;
- direct method success, application error, and timeout;
- twin GET, reported update, desired patch, and reconnect resynchronization;
- throttling and retry-after handling where safely reproducible;
- wrong key, expired SAS, wrong certificate, and disabled device identity;
- classic TLS 1.2 endpoint; optionally, the preview TLS 1.3 device endpoint as
  a non-production compatibility test.

### Hardware-in-the-loop tests

On XIAO ESP32C6:

- cold boot through Wi-Fi, time, TLS, MQTT, and telemetry;
- access-point loss and restoration;
- DHCP lease loss;
- DNS failure and recovery;
- broker connection timeout;
- TLS verification failures;
- SAS expiry/refresh reconnect;
- repeated reset at provisioning-record writes after DPS exists;
- sustained telemetry with BLE and heartbeat active;
- full outbound and inbound queues;
- method handler timeout;
- twin resynchronization after offline desired-property changes;
- measured RAM, flash, task, and watchdog behavior.

## Implementation phases and acceptance criteria

### Phase 0: Record architecture decisions

Deliver:

- an ADR for the cloud/provider boundary;
- an ADR revision or new ADR for MQTT 3.1.1 plus MQTT 5 coexistence;
- an ADR for trusted time, trust anchors, credentials, and private-key
  ownership;
- documented support tiers and security terminology.

Accept when dependency direction, ownership, non-goals, and production gates
are agreed before public APIs are stabilized.

### Phase 1: MQTT 3.1.1 and TLS proofs

Deliver:

- version-aware portable MQTT configuration;
- a selected MQTT 3.1.1 backend adapter;
- a verified TLS 1.2 stream on host and ESP32-C6;
- local broker interoperability tests;
- TLS-negative tests;
- measured resource report.

Accept when a generic MQTT 3.1.1 QoS 1 session works over verified TLS without
Azure code and without a global allocator requirement.

### Phase 2: Azure codec and telemetry

Deliver:

- bounded Azure configuration and identity types;
- client ID, username, telemetry topic, and property encoders;
- SAS development credential path;
- provider lifecycle and error snapshots;
- dedicated Azure reference firmware;
- live IoT Hub telemetry test.

Accept when the XIAO publishes telemetry to IoT Hub over verified TLS,
recovers after link interruption, and emits no secrets in logs.

### Phase 3: Inbound device operations

Deliver:

- C2D subscription, parsing, acceptance, and acknowledgment policy;
- direct-method request/response flow;
- bounded application channels;
- overload and timeout behavior;
- live and HIL tests.

Accept when queue exhaustion and application failure cannot be mistaken for
successful command processing.

### Phase 4: Device twins

Deliver:

- twin GET and response correlation;
- reported-property updates;
- desired-property patch delivery and version tracking;
- full resynchronization after reconnect;
- product-owned schema example and golden vectors.

Accept when offline desired-state changes are observed after reconnect and a
desired patch is reported applied only after application activation.

### Phase 5: Production identity foundation

Deliver:

- CA-signed X.509 mutual TLS;
- opaque signer integration;
- protected credential storage for the reference board or a clearly supported
  external secure element;
- trusted-time production policy;
- overlapping trust-anchor and device-credential rotation;
- security-negative and recovery tests.

Accept when the compatibility matrix can state authenticated Azure IoT Hub
connectivity without development-only credential or clock exceptions.

### Phase 6: DPS

Deliver:

- DPS MQTT codec and registration state machine;
- symmetric-key and/or X.509 attestation according to the selected production
  identity;
- atomic assignment persistence and schema migration tests;
- reprovisioning policy;
- live Azure and reset-injection tests.

Accept when a factory-unassigned device can obtain, persist, and reuse an IoT
Hub assignment without embedding the final hub endpoint in product firmware.

### Phase 7: Durable telemetry and second provider

Deliver separately:

- a persistent outbox with explicit wear, capacity, ordering, and power-loss
  semantics;
- AWS IoT Core integration using the same MQTT, TLS, security, lifecycle, and
  storage boundaries;
- a review of which Azure/AWS behavior has genuinely become common enough to
  move into `cloud-core`.

Accept only after fault-injection tests distinguish queued, MQTT-acknowledged,
and cloud-processed delivery states.

## File-level change map

| File or directory | Planned change |
| --- | --- |
| `docs/adr/` | Add cloud boundary, MQTT-version, and security/time decisions before stabilizing APIs. |
| `docs/plans/azure-iot-hub.md` | Maintain this phased implementation plan and record resolved open decisions. |
| `docs/connectivity/azure-iot-hub.md` | Add user-facing configuration and support documentation only when the first vertical slice works. |
| `docs/compatibility/platforms.md` | Record experimental and production support separately. |
| `Cargo.toml` | Add portable cloud/security crates and the selected MQTT/TLS adapters with reviewed features. |
| `Cargo.lock` | Pin and audit MQTT, TLS, crypto, certificate, encoding, and JSON dependencies. |
| `crates/mqtt/` | Add version-aware session configuration and the proven session boundary. |
| `crates/mqtt-minimq/` | Retain MQTT 5 behavior; adapt only to common contracts without changing its wire version. |
| `crates/mqtt-<v311-backend>/` | Add the selected MQTT 3.1.1 adapter after the proof. |
| `crates/security/` | Add trusted-time, secret, trust-anchor, credential-lease, and opaque-signer contracts. |
| `crates/cloud-core/` | Add minimal provider-independent state, capabilities, errors, and snapshots. |
| `crates/cloud-azure-iot/` | Add Azure Hub identity, codec, service state machine, and provider events. |
| `crates/cloud-azure-dps/` | Add later only when the DPS slice has owned tests and functionality. |
| `crates/embedded-sdk/` | Export portable cloud core and feature-gated provider APIs, never concrete backend types. |
| `firmware/seeed/xiao-esp32c6-azure-iot/` | Compose the dedicated reference application and all product policy. |
| `ports/espressif/esp32c6/` | Add only platform capabilities such as RNG, flash, or secure-element access; no Azure logic. |
| `boards/seeed/xiao-esp32c6/` | Own reviewed partitions and physical security capabilities. |
| `tests/host/` | Add facade, validation, golden vector, and state-machine tests. |
| `tests/integration/` | Add local MQTT 3.1.1 and TLS fixtures. |
| `tests/interoperability/` | Add opt-in live Azure scenarios without committed secrets. |
| `tests/hil/` | Add ESP32-C6 cloud, recovery, coexistence, and resource scenarios. |
| `tests/fuzz/` | Add topic, property, response, and bounded-encoding fuzz targets. |
| `tools/xtask/` | Add redacted local-fixture and opt-in Azure test orchestration only after credential handling is safe. |

## Feature and facade policy

Provider integrations should be opt-in so applications do not link cloud code
or crypto they do not use. A likely facade shape is:

```rust
pub mod cloud {
    pub use embedded_sdk_cloud_core as core;

    #[cfg(feature = "azure-iot")]
    pub use embedded_sdk_cloud_azure_iot as azure_iot;
}
```

Concrete MQTT and TLS backends remain firmware dependencies and are not
re-exported by `embedded-sdk`. Features add functionality; selecting Azure
must not silently replace MQTT 5 or change an existing application's protocol
version.

Firmware packages are built independently to avoid Cargo feature unification
between mutually inappropriate target or backend configurations.

## Support gates

Use explicit support labels:

- **codec complete**: topic/identity generation and parsing pass unit and fuzz
  tests;
- **host interoperable**: MQTT/TLS and Azure operations pass controlled host
  tests;
- **experimental hardware**: the XIAO connects to a real test hub with
  development credential handling;
- **production candidate**: protected identity, trusted time, certificate
  validation, rotation, resource, negative, and recovery gates pass;
- **supported**: documentation, compatibility matrix, CI/HIL coverage, and a
  release maintenance owner exist.

Do not use a single `AZURE_IOT` capability bit to imply every operation and
security mode. Record telemetry, C2D, direct methods, twins, DPS, SAS, X.509,
and durable outbox capabilities separately.

## Open decisions

Resolve these with focused proofs rather than assumptions:

1. Which maintained MQTT 3.1.1 crate best satisfies manual ACK, persistent
   session, cancellation, and resource requirements?
2. Which TLS 1.2 implementation composes cleanly with `embassy-net` and the
   selected MQTT backend on ESP32-C6?
3. Can the TLS backend use an opaque external signer for X.509 client auth, or
   does it require raw private-key bytes?
4. What trusted-time bootstrap and persisted lower-bound policy is acceptable
   for the first supported board?
5. Which root trust anchors ship initially, how are multiple roots selected,
   and how can an emergency root migration be delivered before OTA exists?
6. Does the MQTT backend allow delaying inbound QoS 1 PUBACK until the
   application queue accepts the message?
7. What packet and payload bounds fit the XIAO while supporting the selected
   telemetry, method, and twin examples?
8. Should the first production identity be software X.509, a specific secure
   element, or symmetric key with protected storage?
9. Which persistent namespace and record migration format will own DPS
   assignment and last-known-good time?
10. Should live Azure tests be manual, scheduled, or run through a dedicated
    protected CI environment?

## Principal risks

- **Protocol mismatch:** accidentally sending MQTT 5 to an MQTT-3.1.1-only
  endpoint. Mitigate with type-level version selection and wire golden tests.
- **TLS incompatibility:** selecting a TLS-1.3-only or unsupported-cipher
  implementation for the classic endpoint. Mitigate with the TLS 1.2 proof
  before provider work.
- **Credential exposure:** compile-time environment values appearing in the
  binary, logs, or build artifacts. Mitigate with runtime injection, redacted
  types, artifact scanning, and production protected storage.
- **Untrusted time:** accepting invalid certificates or creating unusable SAS
  tokens. Mitigate with an explicit time state and fail-closed policy.
- **False delivery guarantees:** equating QoS 1 or PUBACK with durable or
  application-level delivery. Mitigate with separate counters and a later
  persistent-outbox design.
- **Queue-induced message loss:** acknowledging inbound data before the
  application owns it. Mitigate through backend selection and acceptance tests.
- **RAM growth:** TLS, MQTT replay, JSON, and concurrent queues exceeding the
  target. Mitigate by deriving bounds, measuring each phase, and keeping
  payload schemas outside the provider.
- **Over-generalization:** freezing an API around assumed AWS/Azure
  similarities. Mitigate by keeping provider APIs explicit and `cloud-core`
  small until the second provider exists.
- **Certificate rotation dead end:** pinning transient certificates or having
  no room for a second root/device identity. Mitigate with multi-slot trust and
  credential designs from the first production phase.
- **Cloud dependency in generic firmware:** making local device behavior fail
  when Azure is unavailable. Mitigate with a dedicated supervised cloud task
  and separate reference firmware.

## Definition of done for initial Azure IoT Hub support

The initial integration is complete only when all of the following are true:

- MQTT 3.1.1 is selected explicitly and MQTT 5 behavior remains intact;
- the cloud provider and protocol crates build as `no_std` without a required
  global allocator;
- the XIAO resolves the hub, establishes authenticated TLS, connects with the
  expected Azure identity fields, and publishes bounded QoS 1 telemetry;
- hostname, trust-chain, certificate-time, and SNI verification are enabled;
- an invalid CA, wrong hostname, expired credential, and wrong device identity
  all fail closed and report the correct failure domain;
- Wi-Fi loss cancels the active cloud connection and reconnects without
  restarting unrelated services;
- no secret appears in formatted values, logs, test output, or committed files;
- packet, TLS, task, RAM, flash, and queue budgets are measured and documented;
- host, local interoperability, live Azure, and XIAO HIL tests cover the
  claimed capability;
- the compatibility matrix says exactly which authentication method and Azure
  operations are experimental or supported;
- limitations explicitly state that QoS 1 is not a power-loss-safe outbox.

Direct methods, twins, DPS, production X.509, and durable telemetry each add
their own definition-of-done gates rather than inheriting support from the
telemetry slice.

## References

- [Communicate with Azure IoT Hub using MQTT](https://learn.microsoft.com/en-us/azure/iot-hub/iot-mqtt-connect-to-iot-hub)
- [Azure IoT Hub TLS support](https://learn.microsoft.com/en-us/azure/iot-hub/iot-hub-tls-support)
- [Control access with shared access signatures](https://learn.microsoft.com/en-us/azure/iot-hub/authenticate-authorize-sas)
- [Authenticate identities with X.509 certificates](https://learn.microsoft.com/en-us/azure/iot-hub/authenticate-authorize-x509)
- [Understand Azure IoT Hub device twins](https://learn.microsoft.com/en-us/azure/iot-hub/iot-hub-devguide-device-twins)
- [Understand Azure IoT Hub direct methods](https://learn.microsoft.com/en-us/azure/iot-hub/iot-hub-devguide-direct-methods)
- [Communicate with Azure DPS using MQTT](https://learn.microsoft.com/en-us/azure/iot-dps/iot-mqtt-connect-to-iot-dps)
- [Azure DPS terminology and attestation mechanisms](https://learn.microsoft.com/en-us/azure/iot-dps/concepts-service)
- [Azure SDK for Embedded C](https://github.com/Azure/azure-sdk-for-c)
