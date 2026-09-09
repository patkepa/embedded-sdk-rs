# XIAO ESP32C6 Beacon Scanner

## Behavior

`xiao-esp32c6-beacon-scanner` continuously performs active BLE legacy scans and
prints a rolling list of nearby advertisers over the XIAO ESP32C6 USB serial
connection. It only observes advertising and scan-response packets: it does not
connect, pair, or write to nearby devices, and it does not start Wi-Fi.

Scanning stays enabled while the firmware prints a snapshot every second.
Controller duplicate filtering is disabled so repeated advertisements provide
RSSI samples for ranging. Devices remain in the rolling list for 15 seconds after
their last report. The allocation-free list holds up to 128 identities; in a
busier environment, the oldest entry is evicted and the snapshot's `evictions`
counter increases.

## Build, flash, and monitor

Build the scanner:

```sh
cargo xtask build xiao-esp32c6/beacon-scanner
```

Connect the board over USB, then flash and monitor it:

```sh
cargo xtask run xiao-esp32c6/beacon-scanner
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
  AVG DELTA  MAX AGEms PKTS ESTm     KIND ADDRESS             MFG    BEACON NAME
-54      +7 -49  42    8    0.6      rnd  AA:BB:CC:DD:EE:FF 0x004c Feasy/iBeacon "Beacon EEFF"
```

`DELTA` is the signed change in average RSSI since the preceding one-second
scan. For example, `+7` means the average signal is 7 dBm stronger and `-4`
means it is 4 dBm weaker. A newly seen device displays `-`.

Recognized beacon frames also appear in a separate category that searches all
128 live registry entries rather than only the strongest 12. It shows up to 24
beacons, ordered by average RSSI:

```text
Detected beacons: 1; showing strongest 1
  AVG DELTA ESTm     ADDRESS             MFG    BEACON NAME
-87      +2 25.1     AA:BB:CC:DD:EE:FF 0x004c Feasy/iBeacon "FSC-BP103"
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
- `ESTm`: approximate distance in metres for recognized beacon frames; `-`
  means unavailable, warming up, or stale. Values below 0.1 m display `<0.1`.
- `MFG`: Bluetooth SIG company identifier when manufacturer data was
  observed, or `-`.
- `BEACON`: recognized beacon vendor and frame formats, or `-`. Values include
  `Feasy`, `iBeacon`, `E-UID`, `E-URL`, `E-TLM`, `E-EID`, and `AltBeacon`.
- `NAME`: complete or shortened local name when advertised, or `-`.

## Distance estimation

The scanner assumes **-59 dBm at one metre** and path-loss exponent **n = 2**
for every recognized beacon, including third-party beacons. It intentionally
does not use the advertised measured-power byte. This is an assumed model,
not per-device calibration or a guarantee of physical distance:

```text
distance_m = 10 ^ ((-59 - filtered_rssi_dbm) / 20)
```

Each device has a median-of-five RSSI filter followed by time-based exponential
smoothing with a nominal one-second time constant. Only packets containing a
recognized beacon frame contribute; name-only scan responses do not. RSSI
outside the HCI valid range of -127 through +20 dBm is ignored entirely.
At least five valid beacon samples are required. After more than two seconds
without one, the distance is unavailable and reacquisition restarts warm-up.
Lost-device notices do not show old distances.

The raw `AVG` column still averages all valid reports in the current snapshot,
so it need not correspond exactly to the smoothed distance. Displaying one
decimal place is formatting, not a claim of decimetre accuracy. Walls, people,
antenna orientation, and different transmit powers can cause substantial bias.
Physical accuracy and sample throughput must be checked on the deployed pair.

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
