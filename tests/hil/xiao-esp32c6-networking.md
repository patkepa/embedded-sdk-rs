# XIAO ESP32C6 Networking HIL Scenario

## Fixture requirements

- A Seeed Studio XIAO ESP32C6 connected to the serial runner.
- A controlled 2.4 GHz access point whose power or availability can be changed.
- DHCP and DNS supplied by the fixture network.
- A fixture-owned A record and TCP listener.
- A BLE central capable of connecting, reading the status characteristic, and
  receiving notifications.

Credentials and endpoint settings must be injected through the documented
environment variables. The harness must redact their values from commands,
logs, and artifacts.

The fixture must set the country code, channel range, and maximum transmit power
to its approved physical-deployment limits. Verify that absent or malformed
values prevent radio startup.

## Expected serial events

For a healthy connection, assert these ordered event classes without matching
network identities:

```text
embedded-sdk wifi associated
embedded-sdk network link up: ipv4=pending
embedded-sdk network IPv4 configured
embedded-sdk network probe succeeded
```

After removing the access point, assert both Wi-Fi and IP loss plus a bounded
retry. After restoring it, assert association, IPv4 configuration, and probe
success again without a device reboot.

## Test cases

1. Boot without credentials and verify scan-only mode, heartbeat, and BLE.
2. Boot with valid credentials and verify DHCPv4 configuration.
3. Resolve the fixture record and connect to its TCP listener.
4. Return DNS failure, restore DNS, and verify a later boot or lease can probe.
5. Refuse and then accept TCP connections without blocking heartbeat or BLE.
6. Remove and restore the AP for 20 cycles without rebooting the device.
7. During DHCP and recovery, connect over BLE, read the status characteristic,
   and observe notifications.
8. Compare allocator and firmware-size evidence with the pre-networking build.
9. Audit captured output for SSID, password, BSSID, local/peer MAC address, and
   the configured test hostname.
10. Stall or suppress scan and association completion events and verify the
    ten-second scan and 30-second association deadlines without losing the
    heartbeat or BLE service.
11. Force rapid association flapping and verify retry delay continues growing;
    then hold the link for 30 seconds and verify the delay resets.

This scenario is release evidence, not part of the credential-free host CI
gate.
