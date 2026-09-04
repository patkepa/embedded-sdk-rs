# IP Networking

## Implemented scope

The SDK provides one IPv4 networking vertical slice:

- `embedded-sdk-networking` contains allocation-free portable link, IPv4,
  gateway, and bounded DNS-server state.
- `embedded-sdk-networking-embassy-net` maps an `embassy-net::Stack` into the
  portable model, waits for configuration changes, and resolves A records into
  caller-owned storage.
- The ESP32-C6 station interface implements `embassy-net-driver` through the
  pinned `esp-radio` release.
- The XIAO ESP32C6 firmware runs station association, the IP-stack runner, and
  network monitoring as independent Embassy tasks.
- DHCPv4 is enabled by default whenever valid development Wi-Fi credentials are
  configured.
- An optional controlled DNS and TCP probe proves the complete path without
  hard-coding a public endpoint.

IPv6, static address policy, TLS, HTTP, SNTP, and cloud integrations are not
implied by this slice. MQTT is a separate protocol slice with its own support
boundary.

## Portable state model

`NetworkSnapshot` records link and IPv4 configuration independently. A radio
link can be up while DHCP is still pending, and an old configuration is not
considered ready after link loss. `is_ip_ready` requires both link-up and an
IPv4 configuration. `is_dns_ready` additionally requires at least one
configured DNS server; it does not assert that the server or internet is
reachable.

The portable model uses `core::net::Ipv4Addr`, validates prefix lengths, and
stores at most three DNS servers without allocation. It deliberately has no
socket trait. Future protocol crates should use ecosystem `embedded-io-async`
or `embedded-nal-async` contracts where appropriate.

## Runtime ownership

The ESP32-C6 Wi-Fi adapter is split after scan and station configuration:

- `Esp32c6StationController` owns association, disconnect detection, and radio
  recovery.
- `StationInterface` is moved into the `embassy-net` runner and owns packet
  exchange.
- Firmware owns task spawning, the random stack seed, DHCP policy, stack
  resources, socket buffers, timeouts, and connectivity probes.

The reference firmware reserves four stack socket slots so DHCP, DNS, the
optional TCP probe, and the MQTT fixture connection can overlap. Probe RX and
TX buffers are each 512 bytes and live in the statically allocated Embassy task
future. The existing 96 KiB radio heap is unchanged.

Release image measurements use the same non-secret benchmark station settings
and the `espflash save-image` application-size report:

| Build | Application bytes | App partition |
| --- | ---: | ---: |
| Branch point with Wi-Fi association | 871,952 | 21.12% |
| DHCPv4 and DNS, probe disabled | 947,088 | 22.94% |
| DHCPv4, DNS, and TCP probe enabled | 947,520 | 22.95% |

The always-on IP path adds 75,136 bytes relative to the branch point; retaining
the optional probe adds another 432 bytes. These figures measure flash image
size, not live RAM. Hardware heap and stack high-water measurements remain part
of HIL validation.

## Running DHCPv4

Configure the station using the existing development-only environment values:

```sh
WIFI_COUNTRY_CODE='PL' WIFI_FIRST_CHANNEL='1' WIFI_CHANNEL_COUNT='13' \
WIFI_MAX_TX_POWER_DBM='20' WIFI_SSID='network' WIFI_PASSWORD='passphrase' \
  cargo xtask run-xiao-esp32c6
```

After association, diagnostics distinguish link-up with DHCP pending from an
active IPv4 configuration. Lease and link loss are monitored independently;
the station task continues its existing bounded association backoff.

Logs report lifecycle transitions and the number of configured DNS servers.
They do not print the SSID, BSSID, station MAC, credentials, DNS names, or the
complete lease.

## Controlled DNS and TCP probe

Set both probe values to resolve a controlled hostname and open one TCP
connection after every newly acquired lease:

```sh
WIFI_COUNTRY_CODE='PL' WIFI_FIRST_CHANNEL='1' WIFI_CHANNEL_COUNT='13' \
WIFI_MAX_TX_POWER_DBM='20' WIFI_SSID='network' WIFI_PASSWORD='passphrase' \
NETWORK_TEST_HOST='sdk-test.internal' NETWORK_TEST_PORT='9000' \
  cargo xtask run-xiao-esp32c6
```

The hostname and listener must be owned by the test environment. No public
service is contacted by default. DNS and TCP operations have ten-second bounds.
Supplying only one probe value, an empty hostname, port zero, or a non-numeric
port disables the probe and reports a configuration error without disabling
DHCP.

The probe establishes and cleanly closes a TCP connection; it is not an
application protocol or a production health check.

## Recovery behavior

Association, IP configuration, and endpoint reachability are intentionally
separate recovery domains:

- Association failure uses the portable exponential Wi-Fi backoff.
- DHCP pending is reported every 30 seconds while the link remains up.
- Link loss cancels the current DHCP wait and returns the monitor to link-down
  state.
- DNS or TCP probe failure is bounded and does not force a radio reconnect.
- A later lease acquisition runs the optional probe again.
- Heartbeat and BLE tasks continue independently of network state.

## Verification boundary

Host tests cover portable readiness invariants, DNS bounds, facade exports,
Embassy configuration conversion, and complete-or-error DNS result copying.
CI cross-compiles the reference firmware with DHCP, DNS, and TCP enabled.

Hardware validation remains required for the compatibility claim. The HIL
scenario must cover lease acquisition, controlled DNS and TCP, AP loss and
restoration, repeated recovery, allocator stability, and BLE operation during
network churn. Until that evidence exists, the compatibility matrix identifies
IP networking as compile-tested with hardware validation pending.
