# Wi-Fi

## Implemented scope

The SDK's first Wi-Fi vertical slice provides:

- Allocation-free portable SSID, passphrase, authentication, scan-result, and
  lifecycle types in `embedded-sdk-wifi`.
- Redacted credential formatting, WPA passphrase validation, and clearing of
  passphrase storage on drop.
- An ESP32-C6 adapter in `embedded-sdk-platform-esp32c6::wifi`, built on the
  pinned `esp-radio` release.
- Active scanning with portable results and identity-free summary reporting.
- Open, WPA2, WPA3, and WPA2/WPA3 station configuration and association.
- Disconnect-event monitoring and automatic reconnection with exponential
  backoff, bounded jitter, and reset after successful association.
- Separation of the owned ESP radio controller and station packet interface so
  association recovery and `embassy-net` packet processing run concurrently.
- DHCPv4, DNS, and bounded TCP probe integration through the SDK networking
  crates and the reference firmware.

Station association remains a layer-2 operation. DHCP, DNS, and sockets are
implemented in the separate networking layer rather than the Wi-Fi contract.
See the [IP Networking guide](networking.md).

## XIAO ESP32C6 behavior

The reference firmware initializes a 96 KiB internal heap, starts the
`esp-rtos` scheduler before the radio, enables the XIAO RF switch on GPIO3, and
selects the on-board antenna by driving GPIO14 low. It then scans up to 20
access points and reports only the count and strongest RSSI. With station
credentials configured, a supervisor waits for disconnect events and retries
forever. Retry delay starts at 1 second, doubles up to a 60-second bound, adds
up to 500 ms of hardware-random jitter within that bound, and resets after a
successful association. The heartbeat runs independently during every delay.
After association, an independent `embassy-net` runner obtains a DHCPv4 lease
and monitors IP configuration loss. It does not require the optional test
probe to be configured.

Run scan-only firmware:

```sh
cargo xtask run xiao-esp32c6
```

Associate with a WPA2/WPA3 personal network:

```sh
WIFI_SSID='network' WIFI_PASSWORD='passphrase' cargo xtask run xiao-esp32c6
```

Associate with an open network:

```sh
WIFI_SSID='network' cargo xtask run xiao-esp32c6
```

The build fails validation at runtime without attempting association when an
SSID is empty, longer than 32 bytes, or paired with an invalid WPA passphrase.
The firmware continues to provide scan diagnostics in that case.

## Credential policy

`WIFI_SSID` and `WIFI_PASSWORD` are a development convenience, not a production
provisioning design. Build-time credentials are embedded in the firmware ELF
and may remain in local build artifacts. They are never printed by the SDK, and
the passphrase type's `Debug` output is always redacted, but those controls do
not make the binary a secure secret store.

Production firmware must obtain credentials through a provisioning service and
store them in a protected, versioned configuration backend. Device-unique
credentials, secure boot, flash encryption, credential rotation, and recovery
policy must be defined before production deployment.

## Dependency and ownership rules

- Portable application code depends on `embedded-sdk-wifi`, not `esp-radio`.
- The ESP32-C6 port owns translation to Espressif authentication and scan types.
- Firmware owns the allocator, scheduler startup, board RF controls, credential
  source, retry policy, and whether radio failures are fatal or degraded.
- SSIDs are modeled as up to 32 arbitrary bytes. The current ESP station API
  accepts textual SSIDs, so the adapter rejects non-UTF-8 station identities.
- Scan records may contain network identity. Product telemetry should prefer
  `ScanSummary` unless explicit disclosure is required and approved.

## Next increment

Hardware-in-the-loop coverage should verify scan, association, DHCP lease
acquisition, DNS, TCP connection, BLE coexistence, and recovery after AP loss
without placing credentials in repository or CI logs. Production provisioning
and protected persistent configuration remain separate follow-up work.
