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
- Access to the owned ESP radio controller/interfaces for the future
  `embassy-net` integration.

Station association is a layer-2 operation. The current slice does not yet run
DHCP, assign an IP address, provide DNS or sockets, or implement reconnection.
Those functions belong in the networking layer rather than the Wi-Fi contract.

## XIAO ESP32C6 behavior

The reference firmware initializes a 72 KiB internal heap, starts the
`esp-rtos` scheduler before the radio, enables the XIAO RF switch on GPIO3, and
selects the on-board antenna by driving GPIO14 low. It then scans up to 20
access points and reports only the count and strongest RSSI.

Run scan-only firmware:

```sh
cargo xtask run-xiao-esp32c6
```

Associate with a WPA2/WPA3 personal network:

```sh
WIFI_SSID='network' WIFI_PASSWORD='passphrase' cargo xtask run-xiao-esp32c6
```

Associate with an open network:

```sh
WIFI_SSID='network' cargo xtask run-xiao-esp32c6
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

The next networking increment should attach the station interface to
`embassy-net`, run DHCPv4, expose portable link/IP/DNS state, and add bounded
reconnection with backoff and jitter. Hardware-in-the-loop coverage should
verify scan, association, DHCP lease acquisition, DNS, and recovery after AP
loss without placing credentials in repository or CI logs.
