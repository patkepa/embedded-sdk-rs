# MQTT

## Implemented scope

The workspace contains two MQTT packages:

- `embedded-sdk-mqtt` provides bounded portable configuration, validated
  topic names and filters, QoS 0/1, lifecycle snapshots, normalized error
  categories, and deterministic reconnect backoff.
- `embedded-sdk-mqtt-minimq` provides MQTT 5 over a caller-supplied
  `embedded-io-async` byte stream using `minimq` 0.13 and caller-owned packet
  buffers.

The adapter supports fresh and resumed sessions, QoS 0/1 publish, subscribe,
unsubscribe, receive, cooperative keepalive polling, graceful disconnect, and
explicit capacity failures. Blocking protocol waits are cancellation-safe;
the composing application must still apply transport deadlines and external
wall-clock timeouts.

QoS 2, topic aliases, request/reply services, multiple product subscriptions,
provider topic layouts, and a persistent outbound queue are not exposed by
this first slice.

## Security status

Secure MQTT is not yet supported. The reference firmware contains only a
plaintext path for an isolated local fixture. It requires the explicit
`MQTT_PLAINTEXT_FIXTURE=1` build input, rejects all MQTT credential inputs, and
must not carry reusable credentials or sensitive payloads.

The adapter accepts credentials only when composition declares an encrypted
transport. Production firmware must not make that declaration until the
stream verifies the broker hostname and a provisioned trust anchor. TLS clock,
rotation, negative-certificate, and credential-provisioning work remains a
support gate.

## Reference firmware fixture

Run a controlled MQTT 5 broker reachable from the XIAO network, then build or
flash with all four explicit inputs:

```sh
WIFI_SSID='network' WIFI_PASSWORD='passphrase' \
MQTT_HOST='broker.test' MQTT_PORT='1883' \
MQTT_CLIENT_ID='xiao-c6-fixture' MQTT_PLAINTEXT_FIXTURE='1' \
  cargo xtask run-xiao-esp32c6
```

Partial values, empty or invalid host/client values, port zero, any value other
than `1` for the fixture switch, or `MQTT_USERNAME`/`MQTT_PASSWORD` disable
MQTT with a bounded configuration error. Wi-Fi, BLE, networking, and heartbeat
continue independently.

The firmware uses these fixture-only topics:

```text
embedded-sdk/test/{client-id}/telemetry
embedded-sdk/test/{client-id}/commands
```

Telemetry payloads currently use the bounded golden value below. This is test
data, not a product schema.

```json
{"version":1,"kind":"heartbeat"}
```

The task subscribes after a fresh session, preserves broker subscriptions on a
resumed session, publishes queued telemetry at QoS 1, receives commands, and
drives keepalive. It logs lifecycle and counters only—not the configured host,
client identifier, topics, payloads, addresses, or credentials.

## Resource contract

The reference firmware fixes these compile-time bounds:

| Resource | Bound |
| --- | ---: |
| MQTT inbound packet buffer | 512 bytes |
| MQTT TX encode/replay arena | 1,024 bytes |
| MQTT TCP RX buffer | 1,024 bytes |
| MQTT TCP TX buffer | 1,024 bytes |
| Outbound application queue | 4 entries |
| Entry payload ownership | borrowed static bytes |
| Broker hostname | 253 bytes |
| Client identifier | 64 bytes |
| Topic/filter | 256 bytes |
| Firmware subscriptions | 1 |
| Exposed publish QoS | 0 and 1 |
| Network stack sockets | 4 |

The maximum accepted inbound packet is the 512-byte RX buffer, including MQTT
headers, topic, properties, and payload. The 1,024-byte TX arena contains
outbound encoding plus retained QoS/subscription replay, so actual concurrent
in-flight capacity depends on encoded packet sizes and fails explicitly when
the arena is full. The firmware has no heap-backed MQTT queue.

Best-effort producer overflow drops the newly produced fixture telemetry and
increments a saturating lifecycle counter. No command or reply producer is
implemented, so no command acceptance guarantee is implied.

Flash, task high-water, and live heap deltas require HIL measurement and are
not claimed from compile output.

## Recovery behavior

- DNS, TCP, or MQTT failure does not restart Wi-Fi or BLE.
- MQTT uses bounded exponential backoff with caller-supplied hardware jitter.
- IPv4 loss cancels the live MQTT loop and returns to network-readiness wait.
- A broker reconnect resumes the MQTT session when the broker retained it;
  otherwise firmware restores its one subscription.
- In-flight QoS state survives transport reconnect only in live RAM.
- Heartbeat and BLE tasks never wait on the MQTT queue.

## Verification boundary

Host tests cover configuration boundaries, facade isolation, error mapping,
credential/plaintext rejection, reconnect behavior, lifecycle counters, and a
fragmented async byte stream carrying CONNECT, QoS 1 publish, and inbound
publish traffic. CI cross-compiles the XIAO firmware with the generic
`embassy-net::TcpSocket` transport.

Broker interoperability, TLS verification, resource measurements, repeated
hardware recovery, and BLE coexistence under MQTT load still require the
controlled integration/HIL fixtures described in the local implementation
plan. Until those pass, MQTT is a compile-tested plaintext fixture rather than
a supported production capability.
