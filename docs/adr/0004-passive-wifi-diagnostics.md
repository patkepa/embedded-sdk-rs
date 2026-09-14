# ADR 0004: Passive Wi-Fi diagnostics

- Status: Accepted
- Date: 2026-09-14

## Decision

Add a dedicated DFRobot Beetle ESP32-C6 (DFR1117) Wi-Fi scanner. A portable
`embedded-sdk-wifi::diagnostics` module parses bounded 802.11 header captures
and aggregates observations; firmware owns channel policy and serial reports.
The existing station adapter remains independent. Firmware uses the pinned
`esp-radio` safe sniffer API without adding an unsafe FFI boundary.

The radio callback only copies a bounded prefix and receive metadata into a
bounded queue. Parsing, aggregation, and logging run outside the callback.
Queue drops, truncated captures, table evictions, channel dwell, and report
gaps are explicit. Tables retain identities across windows, while traffic
counters reset per window. No payload captures are printed or retained beyond
the queue. A configured target MAC is protected from table eviction.

Survey mode visits channels 1–13 by default; channel lock enables continuous
observation of one IoT network. Receive signal is measured at the scanner,
not at the IoT device. Silence, retries, receive errors, advertised BSS load,
and management events are separate observations, not proof of collisions or
application failure. Protected management bodies are never decoded as reasons.

## Consequences

No network credentials, association, raw transmission, IP stack, or backend are
required. A passive observer cannot establish encrypted DHCP/DNS/MQTT health,
true packet loss, physical collisions, or complete channel utilization. Control
and CRC-error delivery are limited by the pinned driver's default filter; missing
data is not reported as a zero error rate. Hardware validation is required for
RF accuracy, capture throughput, board operation, and long-running stability.

## Readable report update

Default reports show current observations with aligned tables, decoded security
labels and warnings. Stale identities and raw fields remain in opt-in verbose
reports (`WIFI_SCANNER_VERBOSE=1`). Receive and advertised AP channels are
separate facts. Wildcard probe BSSIDs do not identify associations. The platform
normalizes the pinned driver's eight-bit RSSI/noise fields before aggregation.
