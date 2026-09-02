# XIAO ESP32C6 Beacon Hardware Test

This checklist validates behavior that cross-compilation and host tests cannot
prove. Record the board revision, antenna selection, power source, scanner,
firmware commit, and build-time beacon settings with the results.

## Advertising interoperability

1. Flash `xiao-esp32c6-beacon` and capture its startup log.
2. Scan for at least one minute with two independent BLE scanners.
3. Confirm both decode the configured UUID, major, minor, and measured-power
   byte as an iBeacon frame.
4. Confirm the advertiser is reported as non-connectable and scannable.
5. Use an active scan and confirm its scan response contains the complete local
   name `Beacon XXXX`, where `XXXX` equals the final four hexadecimal characters
   of the display-order BLE address. Confirm a passive scan still receives the
   complete iBeacon advertisement without requiring the scan response.
6. Attempt a connection and confirm that it cannot be established.
7. Power-cycle the board and confirm its BLE address, derived local name, and
   default derived minor value remain stable.

## Interval and identity settings

1. Capture over-the-air timestamps for at least 1,000 events using the default
   `20 ms` interval. Confirm there are no unexpected scheduling gaps and report
   median, p95, p99, and maximum inter-event time.
2. Rebuild with non-default UUID, major, minor, measured power, and a `500 ms`
   interval.
3. Confirm all payload fields on both scanners.
4. Capture over-the-air timestamps for at least 100 events and confirm the
   observed interval is consistent with the configured value plus the BLE
   advertising random delay.
5. Build once with an invalid setting and confirm serial output reports the
   setting error and no advertising occurs.

## Discovery response

1. Run the target timed-unlock receiver in continuous passive-scan mode.
2. Begin a receiver capture before powering the beacon on.
3. Repeat at least 100 cold starts and record the time from beacon power-on to
   the receiver's first matching advertising report.
4. With both devices already running, introduce the beacon at the configured
   approach boundary at least 100 times and record first-report latency.
5. Report median, p95, p99, and maximum latency, along with missed approaches.

## RF and power

1. Repeat range and packet-reception measurements for each intended TX power,
   enclosure, orientation, and antenna path.
2. Calibrate the one-metre measured-power byte according to the Beacon Guide.
3. Measure average and peak supply current at the intended advertising interval.
4. Run from the intended battery or supply through minimum operating voltage
   and confirm clean startup and continued advertising.
5. Run continuously for the product's soak-test duration and confirm no loss of
   advertising or unexpected resets.

Do not promote beacon support beyond Tier 2 until repeatable results and their
raw evidence are checked into the release process.
