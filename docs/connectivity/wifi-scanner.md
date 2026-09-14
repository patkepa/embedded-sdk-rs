# Beetle ESP32-C6 Wi-Fi scanner

`beetle-esp32c6-wifi-scanner` turns a **DFRobot Beetle ESP32-C6 (DFR1117)**
into a standalone passive observer for 2.4 GHz IoT Wi-Fi networks. It computes
diagnostics locally and writes a delimited information block plus one JSON
telemetry line per capture window over USB serial. It needs no credentials or server
for local use. The board is distinct from the XIAO and
FireBeetle 2; the firmware does not drive XIAO antenna-switch pins.

## Build and use

```sh
cargo xtask build beetle-esp32c6/wifi-scanner
cargo xtask run beetle-esp32c6/wifi-scanner
```

The second command builds, flashes, and starts the serial monitor. The ELF is
`target/riscv32imac-unknown-none-elf/release/beetle-esp32c6-wifi-scanner`.

To graph live results, start the backend, flash the scanner, then run the USB
bridge from another terminal (stop the serial monitor first so it releases the
port):

```sh
cargo xtask telemetry
cargo run -p beetle-wifi-bridge -- --port /dev/cu.usbmodem1201 --device beetle-01
```

The bridge publishes QoS 1 to
`embedded-sdk/beetle-wifi-scan/v1/beetle-01/telemetry` and reconnects after a
serial or broker interruption. Open the **Beetle Wi-Fi scanner** dashboard in
Grafana at <http://localhost:3000>. The host timestamps receipt; scanner uptime
remains a separate field. Only aggregate counts and signal values leave the
USB connection; SSIDs, BSSIDs, client MACs and event details stay in local logs.
The bridge must stay running to collect data. A scanner that is unplugged or a
stopped bridge cannot backfill missed windows.

Default survey mode visits channels **1 through 13**, dwelling five seconds on
each. A full sweep takes at least 65 seconds plus report time. This discovers
APs and active devices without association or probe transmission. All 13 primary channels exposed by the driver are surveyed. No 5/6 GHz scan
is possible.

For a device that intermittently loses connectivity, find its AP's primary
channel and lock there. Optionally highlight the device's Wi-Fi MAC:

```sh
WIFI_SCANNER_CHANNEL=6 WIFI_SCANNER_TARGET_MAC=02:11:22:33:44:55 \
  cargo xtask run beetle-esp32c6/wifi-scanner
```

These are compile-time settings, tracked by Cargo's `option_env!` dependencies.
`WIFI_SCANNER_CHANNEL=0` or unset selects survey; `1..13` selects lock. The
target must be a nonzero unicast MAC in colon-separated hex. Invalid settings
produce an explicit boot error instead of silently surveying the wrong network.
A target MAC highlights events and protects its peer record from eviction; it
does **not** restrict capture or automatically find/follow its channel.

Place the scanner where it can receive both the IoT device and its AP. Reproduce
the issue and correlate scanner timestamps with the device/AP logs. Channel
lock avoids long survey absences, but USB report output still creates a measured
blind interval. Capture is disabled and the bounded queue drained before each
report; `capture_gap_before_window_ms` inside the next block
includes printing and channel-switch time.

## Log blocks

The default is a readable report: a summary and warnings, followed by aligned
network and active-radio tables and connection events. Only networks heard in
this window and radios with current traffic are listed. Cached network counts
remain visible; stale target observations are called out explicitly. Networks
are sorted strongest first; the optional target leads the radio table.

`-` means unavailable. Signal values are signed dBm. Security names describe AP
advertisements (for example `WPA2/WPA3`), PMF is `req`/`opt`/`no`, and load is the
AP's advertised utilization as a percentage. `CH` in the network table comes
from the AP's DS/HT information elements; the header shows the scanner's receive
channel. Adjacent-channel reception does not mean the AP changed channel.
`*` after a name indicates incomplete information elements.

To keep every raw field and retained record in the logs, enable verbose mode:

```sh
WIFI_SCANNER_VERBOSE=1 cargo xtask run beetle-esp32c6/wifi-scanner
```

Unset/`0` selects the readable report, `1` selects verbose; other values are
rejected at boot. Both modes include the window's capture gap and drop counts.
The readable block uses `WI-FI DIAGNOSTICS` / `END WI-FI DIAGNOSTICS` delimiters;
verbose retains `WIFI DIAGNOSTICS BEGIN` / `WIFI DIAGNOSTICS END`.

The **verbose** block includes:

| Area | Measurements |
| --- | --- |
| Window | Uptime, primary channel, observation duration, table capacities |
| Captured traffic | Frames, bytes including FCS, frames/s, bytes/s, management/data/control counts |
| Radio | RSSI min/average/max, raw noise-floor average, latest driver receive timestamp |
| Capture quality | Queue drops, truncated prefixes, malformed headers, delivered RX errors, off-window frames, table evictions, omitted events |
| APs | SSID bytes, BSSID, channel, age, RSSI, beacon interval in TU, capability bits, privacy/RSN/WPA advertisements, HT/HE presence, secondary-channel code, DTIM period |
| AP security | RSN group cipher selector, pairwise/AKM type bitmaps, unknown suites, PMF capable/required |
| AP load | Advertised station count, channel utilization on a 0–255 scale, available admission capacity in 32 µs units, when BSS Load is present |
| Peers | MAC, last inferred BSSID, age, transmitted/addressed frames, transmitted bytes, RSSI min/average/max and change, largest observed TX gap within the window, last sequence and raw PHY format/rate, protected/power-save/beacon counts, data retry fraction |
| Events | Association/reassociation requests and responses, authentication, disassociation, deauthentication; transmitter, receiver, time, status/reason code and protected flag |
| Target | Highlighted peer/events, age and channel of its last observation |

In verbose mode, all retained AP records (32 maximum) and all peer records for the current
channel (96 total capacity) are printed; rows are not limited to the strongest
devices. Identities remain until oldest-record eviction. **Check ages**: a
retained AP is not necessarily present now. Peer traffic counts cover the current
window, while RSSI change compares with the previous measured window on that
channel. A receiver-only row has no signal or PHY samples. `None` means unavailable;
`Some(...)` contains a measurement. Names use byte escapes so arbitrary SSIDs
cannot inject new lines or terminal escape codes into logs.

RSN suite bitmaps encode standard `00:0f:ac` selector types: e.g. pairwise bit 4
means CCMP-128; AKM bit 2 means PSK and bit 8 means SAE. A PSK+SAE advertisement
therefore has AKM bitmap `0x00000104`. Unknown OUIs/types are counted separately.
These are AP advertisements, not a client's negotiated settings. The first 384
bytes of each frame are retained; `ies_incomplete=true` means absent AP fields
cannot be interpreted as unsupported features. No password or data payload is
printed. Capture prefixes are transient, not exported packet captures.

Both report modes use these warning thresholds (verbose field names below):

- `weak_signal_at_scanner`: average TX RSSI below −75 dBm, with at least 10
  transmitted frames sampled in the window.
- `high_observed_retry_fraction`: at least 20% of captured data frames have the
  retry flag, with at least 20 data frames sampled.

For example, `data_retries=5/20 (25.0%)` describes the sampled frames; it is
neither packet loss nor a collision rate. These are troubleshooting thresholds,
not protocol guarantees. An observed disconnect/rejection event includes the
raw reason/status to compare with the AP/device logs. Protected management
bodies have `code=None`; captured unprotected events are not authenticated.

## Interpretation and limits

The scanner measures RSSI **at its own antenna**. Frames addressed to an IoT
device do not prove reception by that device. Silent devices may be sleeping,
idle, out of range, on another channel, or missed by capture; silence is not
reported as an outage. Max TX gap excludes intervals spanning report boundaries
or channel changes. Sequence numbers are observations, not packet-loss counters.

The pinned `esp-radio 0.18.0` safe sniffer API exposes the default management/data
filter. This implementation processes those frame types only. Control-frame
completeness and CRC-error capture are **unavailable**, even if their displayed
delivered counts are zero. Noise floor and raw PHY values require hardware
validation; rate codes are only meaningful for legacy b/g frames. The driver
timestamp can wrap and is not a wall-clock timestamp. No exact collision count,
airtime utilization, interference-source identification, or packet-loss rate is
claimed. BSS Load utilization is an AP advertisement, not scanner-measured load.

Encrypted DHCP, DNS, TCP, MQTT, and other application health cannot be inferred
reliably from this passive observer. Correlate its RF/event evidence with those
devices' own connection, address, DNS, socket and application logs to establish
the cause of a service failure.

The callback copies at most 384 bytes into a 32-entry queue and increments an
overflow counter; parsing and printing happen outside the callback. Memory is
bounded: 96 KiB radio heap, fixed queue, and static analyzer tables. Busy-channel
loss before callback delivery remains unknown even when queue drops are zero.

## Validation

`cargo xtask check` covers portable parsing, malformed/truncated input, binary
SSIDs, direction mapping, protected reasons, retry denominators, bounded tables,
RSN/PMF parsing, and window resets. Build both scanner and reference firmware:

```sh
cargo xtask build beetle-esp32c6/wifi-scanner
cargo xtask build xiao-esp32c6
```

User-provided hardware logs confirmed capture and reporting, and exposed a
signed-byte decoding issue: raw RSSI 195 is -61 dBm, not +195 dBm. RSSI and
noise now receive eight-bit sign extension before aggregation. On 2026-09-14,
the telemetry-enabled scanner was flashed to the connected Beetle, and live
window messages reached MQTT, SQLite and Grafana. Controlled RF comparison
and long-duration validation remain pending. Follow
[the Beetle HIL procedure](../../tests/hil/beetle-esp32c6-wifi-scanner.md) before
relying on it for field diagnosis.

## Hardware and API references

- [DFRobot Beetle ESP32-C6 DFR1117 pinout and specifications](https://wiki.dfrobot.com/dfr1117/)
- [Espressif ESP32-C6 Wi-Fi sniffer modes](https://docs.espressif.com/projects/esp-idf/en/stable/esp32c6/api-guides/wifi-driver/wifi-modes.html)
- [Pinned esp-radio 0.18.0 sniffer source](https://docs.rs/crate/esp-radio/0.18.0/source/src/wifi/sniffer.rs)

This workspace uses native Rust `esp-hal`/`esp-radio`, not an ESP-IDF application;
the ESP-IDF documentation describes the underlying radio capability, not an
additional dependency.
