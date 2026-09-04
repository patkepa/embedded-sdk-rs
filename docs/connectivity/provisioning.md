# XIAO ESP32C6 Provisioning

## Implemented boot path

The reference firmware opens the board-owned provisioning partition before it
selects Wi-Fi, network-probe, or development MQTT settings. Recovery follows
this precedence:

1. a complete pending configuration being verified;
2. a complete confirmed persistent configuration;
3. build-time development settings only when the
   `development-config-fallback` feature is enabled;
4. otherwise unprovisioned scan-only mode.

Recovery validates the atomic state record and every referenced slot before
product decoding. Corruption, missing slots, generation disagreement, storage
failure, and incompatible product data stop persistent configuration from
being applied. They do not silently erase records or fall back to compiled
credentials.

The default reference build enables `development-config-fallback` solely for
the current migration period. A production-style build disables it explicitly:

```sh
cargo build -p xiao-esp32c6-firmware \
  --release \
  --no-default-features \
  --target riscv32imac-unknown-none-elf
```

## Pending verification

A pending candidate is loaded from its complete slot, decoded using the XIAO
product schema, and durably marked as having started one verification attempt.
The initial policy permits one attempt so a reboot or brownout cannot create an
unbounded retry loop.

Successful verification requires Wi-Fi association and usable DHCPv4 state.
When a controlled probe is configured, DNS readiness and one bounded TCP
connection must also succeed. The overall attempt deadline is 90 seconds and
individual DNS/TCP operations retain their 10-second deadlines.

Success atomically promotes the pending generation to confirmed. Failure
atomically records a bounded rejection reason. Firmware waits 250 milliseconds
for diagnostics to drain, then performs a software reset. The next boot
completes rollback to the prior confirmed generation, or returns to
unprovisioned mode when no prior generation exists.

Development MQTT connectivity is intentionally not part of confirmation. It
is a separately reported fixture behavior and does not establish a production
security property.

## HIL fixture serial adapter

The development-only adapter is compiled with the additive
`hil-provisioning` feature. It owns USB Serial/JTAG during a 15-second window
at boot, before heartbeat and radio tasks start. Pending candidates skip this
window so connectivity verification begins without an artificial delay.

```sh
cargo xtask build-xiao-esp32c6-hil
```

The fixture is identified in process as `HilFixture` authority. The bounded
boot window and physical cable constitute the fixture-presence condition;
this is not a production authentication mechanism. Factory reset is permitted
only through this explicitly enabled fixture path and can clear a repository
already in recovery mode.

Serial frames use this fixed representation:

| Offset | Size | Meaning |
| ---: | ---: | --- |
| 0 | 4 | NUL-prefixed magic `00 50 52 56` |
| 4 | 1 | framing version, currently `1` |
| 5 | 1 | direction: request `0`, response `1` |
| 6 | 2 | big-endian CBOR-envelope length |
| 8 | variable | complete provisioning CBOR envelope |
| final | 4 | big-endian IEEE CRC-32 over header and payload |

The 12-byte overhead produces a maximum 1,100-byte request frame, within the
1,104-byte transport budget. Printable boot diagnostics cannot contain the
NUL-prefixed delimiter. The decoder rejects oversized, wrong-direction,
unsupported, and corrupt frames; clears partial frames after 500 milliseconds;
and closes the window after eight frame errors. Only one complete request is
owned at a time, giving the adapter an effective command-queue depth of one.
Request, candidate, response, and framing buffers are zeroized or overwritten
after processing.

## Current resource envelope

- Maximum product candidate: 1,024 bytes.
- Maximum durable slot record: 1,048 bytes, including metadata and CRC.
- Sequential-store working buffer: 1,053 bytes.
- Board-owned provisioning partition: 131,072 bytes.
- Pending verification: one boot attempt with a 90-second overall deadline.
- HIL serial: 15-second boot window, 500-millisecond inter-byte timeout,
  five-second transaction timeout, eight frame errors, queue depth one.
- Default release image after boot integration: 1,030,912 of 1,310,720 bytes
  (78.65%), leaving 279,808 bytes in the application partition. This is
  165,600 bytes above the 865,312-byte pre-provisioning baseline recorded in
  ADR 0005.
- Release image with `--no-default-features --features hil-provisioning`:
  1,054,928 bytes (80.48%), 24,016 bytes above the default boot-integration
  build.

The image measurement uses the checked-in partition table and
`espflash save-image` for the ESP32-C6 release artifact. Runtime stack use,
flash-operation latency, compaction behavior, and endurance still require HIL
measurement.

## Security and diagnostic boundary

Provisioning logs contain state, generation, attempt, and bounded reason only.
They must not contain SSID, credential, configured hostname, client identifier,
secret length, candidate bytes, or secret-derived values.

The current persistent format provides CRC-based corruption detection but no
credential confidentiality or authenticity. Logical reset is not verified
secure erase. Bluetooth credential provisioning remains disabled, and the
existing bring-up GATT service must not receive credential characteristics
without the separate authentication and ownership threat model required by the
provisioning plan.

## Validation status

Host fault-injection tests cover interrupted slot staging, state selection,
confirmation, rollback selection, and restartable reset. The storage-backed
firmware and its no-development-fallback variant cross-compile and fit the
checked-in application partition.

Initial hardware evidence covers blank recovery, framed commit, reboot,
attempt-exhaustion rollback, logical factory reset, and post-window radio task
coexistence; see the
[XIAO provisioning HIL scenario](../../tests/hil/xiao-esp32c6-provisioning.md).
Valid-network confirmation, repeated writes and compaction, radio-latency
measurement, and cut-power testing are still required. Until that evidence
exists, the board does not advertise `Capabilities::PERSISTENT_STORAGE` and
this implementation must not be treated as production-qualified provisioning.
