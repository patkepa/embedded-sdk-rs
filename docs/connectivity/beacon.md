# XIAO ESP32C6 iBeacon

## Behavior

`xiao-esp32c6-beacon` is a dedicated, continuously broadcasting BLE beacon
firmware. It sends a legacy iBeacon-compatible manufacturer frame on all three
primary advertising channels. Advertising is non-connectable: a nearby device
can observe the frame, but cannot open a BLE connection or read a GATT service.
It is scannable and exposes a local name through its scan response while keeping
the complete iBeacon frame in the primary advertisement. Passive scanners still
receive the iBeacon frame; active scanners additionally receive the name.

The firmware builds the ESP32-C6 platform with its BLE-only feature, does not
include Wi-Fi coexistence, and keeps the user LED off. It derives a
stable random BLE address from the chip's Bluetooth MAC and uses the low 16 bits
of that identity as the default iBeacon minor value. This normally distinguishes
small numbers of development boards without separate builds, but a 16-bit
derived value is not a fleet-wide uniqueness guarantee. Assign and set
`BEACON_MINOR` when uniqueness is required. Its default local name is `Beacon
XXXX`, where `XXXX` is the final four hexadecimal characters of that same stable
BLE address, making nearby boards easy to correlate with scanner output and the
default minor value.

The default frame is:

| Field | Default |
| --- | --- |
| Local name | `Beacon XXXX`, derived from the BLE address |
| Proximity UUID | `7a1e1000-4c2a-4f66-a1d4-3f55b55a1000` |
| Major | `1` |
| Minor | Derived from the board's Bluetooth MAC |
| Measured power | `-59 dBm` placeholder |
| Advertising interval | `20 ms` |
| Controller TX power | `+9 dBm` |

The 30-byte legacy advertising payload consists of the standard LE-only flags,
Apple's Bluetooth SIG company identifier (`0x004c`), the iBeacon type and
length bytes (`0x02`, `0x15`), UUID, big-endian major and minor values, and the
signed calibrated one-metre RSSI byte.
The scan response contains the complete local name. A custom `BEACON_NAME`
continues to override the address-derived default.

## Build, flash, and monitor

Build the default beacon:

```sh
cargo xtask build-xiao-esp32c6-beacon
```

Connect a XIAO ESP32C6 over USB, then flash and monitor it:

```sh
cargo xtask run-xiao-esp32c6-beacon
```

The ELF is written to:

```text
target/riscv32imac-unknown-none-elf/release/xiao-esp32c6-beacon
```

The serial monitor reports the effective local name, UUID, major, minor,
interval, calibrated RSSI, radio power, and the transition to broadcasting. It
does not print the device's complete Bluetooth address.

## Deployment configuration

All settings are captured at compile time. Changing one of these variables and
rebuilding produces a deployment-specific image:

| Environment variable | Accepted values |
| --- | --- |
| `BEACON_NAME` | `1` through `29` UTF-8 bytes; omit for `Beacon XXXX` |
| `BEACON_UUID` | Canonical 8-4-4-4-12 hexadecimal UUID |
| `BEACON_MAJOR` | `0` through `65535` |
| `BEACON_MINOR` | `0` through `65535`; omit for the board-derived value |
| `BEACON_MEASURED_POWER_DBM` | `-128` through `127` |
| `BEACON_INTERVAL_MS` | `20` through `10240` milliseconds |
| `BEACON_RADIO_TX_POWER_DBM` | `-15`, `-12`, `-9`, `-6`, `-3`, `0`, `3`, `6`, `9`, `12`, `15`, `18`, or `20` |

For example:

```sh
BEACON_NAME='Entrance Beacon' \
BEACON_UUID='fda50693-a4e2-4fb1-afcf-c6eb07647825' \
BEACON_MAJOR=100 \
BEACON_MINOR=7 \
BEACON_MEASURED_POWER_DBM=-62 \
BEACON_INTERVAL_MS=500 \
BEACON_RADIO_TX_POWER_DBM=-3 \
cargo xtask run-xiao-esp32c6-beacon
```

Invalid values produce an explicit serial error and prevent advertising. UUID,
name, major, and minor are public identifiers rather than secrets; do not
encode credentials or user data into the frame. The stable BLE address and
beacon identity can be used to track the device over time, so deployments must
account for that privacy property.

## Calibrating measured power

`BEACON_MEASURED_POWER_DBM` tells receivers what RSSI to expect one metre from
this specific assembled beacon. It does not change the actual radio output.
The default `-59` is only a bring-up value.

For each enclosure and antenna configuration:

1. Set the final `BEACON_RADIO_TX_POWER_DBM` and interval.
2. Place the scanner one metre from the beacon in the intended orientation and
   deployment environment.
3. Collect RSSI for at least 30 seconds, discard obvious interference spikes,
   and calculate the median.
4. Rebuild with that signed median as `BEACON_MEASURED_POWER_DBM`.
5. Confirm ranging behavior at several distances and orientations.

RSSI-based distance is inherently approximate and changes with walls, people,
enclosures, antenna selection, and receiver hardware.

## Power and production boundary

A longer interval and lower TX power generally reduce average radio energy but
also reduce discovery speed and range. The firmware avoids Wi-Fi, connections,
GATT, and LED activity, but the current ESP radio/RTOS integration has not been
characterized as an ultra-low-power implementation. Measure current on the
final hardware before making battery-life claims.

The default 20 ms interval prioritizes minimum approach-detection latency. A
legacy advertising event also includes a Bluetooth-mandated random delay, so
the effective over-the-air interval will be slightly longer. Deployments that
value battery life over response time should explicitly set a longer
`BEACON_INTERVAL_MS` and validate the resulting discovery latency.
Active scanners can send scan requests that cause the beacon to transmit its
name response; include that extra radio activity in power and congestion tests.

The implementation is compile-tested. Interoperability, RF range, calibrated
RSSI, current consumption, brownout behavior, and long-duration broadcasting
still require validation on physical hardware before production deployment.
