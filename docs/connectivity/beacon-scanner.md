# XIAO ESP32C6 Beacon Scanner

## Behavior

`xiao-esp32c6-beacon-scanner` continuously performs active BLE legacy scans and
prints a rolling list of nearby advertisers over the XIAO ESP32C6 USB serial
connection. It only observes advertising and scan-response packets: it does not
connect, pair, or write to nearby devices, and it does not start Wi-Fi.

Each scan window lasts five seconds. The controller filters duplicate packets
within a window, then the firmware restarts scanning so RSSI and last-seen data
continue to refresh. Devices remain in the rolling list for 15 seconds after
their last report. The allocation-free list holds up to 128 identities; in a
busier environment, the oldest entry is evicted and the snapshot's `evictions`
counter increases.

## Build, flash, and monitor

Build the scanner:

```sh
cargo xtask build-xiao-esp32c6-beacon-scanner
```

Connect the board over USB, then flash and monitor it:

```sh
cargo xtask run-xiao-esp32c6-beacon-scanner
```

The ELF is written to:

```text
target/riscv32imac-unknown-none-elf/release/xiao-esp32c6-beacon-scanner
```

## Serial output

After each scan window, the firmware prints a compact table ordered by average
signal strength during that window. To keep a busy radio environment readable,
it shows at most the 12 strongest devices while the summary still reports the
full count seen during the window. For example:

```text
Seen 1 devices (8 packets, 0 malformed, 0 evicted); showing strongest 1
  AVG DELTA  MAX AGEms PKTS KIND ADDRESS             MFG    BEACON NAME
-54      +7 -49  42    8    rnd  AA:BB:CC:DD:EE:FF 0x004c Feasy/iBeacon "Beacon EEFF"
```

`DELTA` is the signed change in average RSSI since the preceding five-second
scan. For example, `+7` means the average signal is 7 dBm stronger and `-4`
means it is 4 dBm weaker. A newly seen device displays `-`.

Recognized beacon frames also appear in a separate category that searches all
128 live registry entries rather than only the strongest 12. It shows up to 24
beacons, ordered by average RSSI:

```text
Detected beacons: 1; showing strongest 1
  AVG DELTA ADDRESS             MFG    BEACON NAME
-87      +2 AA:BB:CC:DD:EE:FF 0x004c Feasy/iBeacon "FSC-BP103"
```

The scanner separately remembers the strongest 48 devices from the preceding
scan. If one of them produces no packets for an entire scan window, the scanner
reports it once in a separate section:

```text
Lost since previous scan: 1; showing strongest 1
 LAST AGEms KIND ADDRESS             MFG    BEACON NAME
-54   5242  rnd  AA:BB:CC:DD:EE:FF 0x004c Feasy/iBeacon "Beacon EEFF"
```

`LAST` is its average RSSI in the last scan where it was observed. `AGEms` is
the time since its final packet. The prior displayed list is retained separately
so the notice still works when the busy rolling registry evicts the device.

The fields are:

- `ADDRESS` and `KIND` (`pub` or `rnd`): the advertiser address as received
  over BLE.
- `AVG`: average RSSI in dBm across packets received during this scan.
- `DELTA`: signed average-RSSI change in dBm since the preceding scan.
- `MAX`: strongest RSSI in dBm received during this scan.
- `AGEms`: milliseconds since its latest packet.
- `PKTS`: packets received for that identity during this scan.
- `MFG`: Bluetooth SIG company identifier when manufacturer data was
  observed, or `-`.
- `BEACON`: recognized beacon vendor and frame formats, or `-`. Values include
  `Feasy`, `iBeacon`, `E-UID`, `E-URL`, `E-TLM`, `E-EID`, and `AltBeacon`.
- `NAME`: complete or shortened local name when advertised, or `-`.

## Privacy and interpretation

The output intentionally exposes nearby BLE addresses and public advertising
metadata. Treat captured logs as potentially sensitive location and device
presence data. Many operating systems use rotating private addresses, so one
physical device can appear under different addresses over time; conversely, a
stable address is not proof of device ownership or identity. RSSI is a noisy
signal-strength observation, not a reliable distance measurement.

The implementation is compile-tested. Receiver sensitivity, crowded-RF
behavior, USB logging throughput, long-duration stability, and the accuracy of
the rolling-list policy still require validation on physical hardware.
