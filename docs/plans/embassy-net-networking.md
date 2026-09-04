# Embassy-Net IPv4 Networking Implementation Plan

## Status

- Status: Implemented; hardware validation pending
- Branch: `feat/embassy-net-networking`
- Target: Seeed Studio XIAO ESP32C6
- Primary outcome: turn an associated Wi-Fi station link into a recoverable
  DHCPv4, DNS, and TCP networking path

## Why this is the next slice

The current ESP32-C6 firmware can scan, associate, detect link loss, and
reassociate, but it stops at IEEE 802.11 layer 2. It cannot acquire an IP
configuration, resolve a hostname, or exchange application data. Adding cloud,
MQTT, time synchronization, or OTA before an IP stack would either duplicate
networking ownership or build those features on an untested foundation.

The selected dependency baseline is compatible without changing the pinned
Espressif stack:

- `esp-radio` 0.18.0 exposes its station `Interface` as an
  `embassy-net-driver` 0.2.0 driver.
- `embassy-net` 0.9.1 also consumes `embassy-net-driver` 0.2.0 and uses the
  already selected `embassy-time` 0.5.1 release.
- The first slice will enable only `dhcpv4`, `dns`, `medium-ethernet`,
  `proto-ipv4`, and `tcp`. Default features remain disabled.

The Cargo lockfile remains the reproducibility boundary. Updating
`esp-radio`, adding IPv6, or adding application protocols is outside this
change.

## Scope

### In scope

- Allocation-free portable link, IPv4 configuration, and DNS-server state.
- A reusable adapter from an `embassy-net::Stack` to the portable state model.
- ESP32-C6 ownership separation between the Wi-Fi controller and station
  interface.
- A statically allocated `embassy-net` runner using DHCPv4 and DNS.
- Automatic IP-state recovery after station link loss and reassociation.
- An opt-in DNS plus TCP smoke probe against a controlled test endpoint.
- Host tests, ESP32-C6 compile coverage, and networking hardware validation.
- Documentation of behavior, resource ownership, limitations, and validation.

### Out of scope

- IPv6, SLAAC, static IPv4 configuration, Wi-Fi access-point mode, and UDP
  application APIs.
- TLS, MQTT, HTTP abstractions, cloud integrations, SNTP, and OTA.
- BLE provisioning, persistent Wi-Fi credentials, and flash partitions.
- A general SDK-owned socket trait. Protocol crates should use established
  `embedded-io-async` or `embedded-nal-async` contracts when those protocols
  are implemented.
- An `esp-radio` upgrade. Version migration should be a separate change with
  its own hardware evidence.

## Architecture decisions to record first

Add `docs/adr/0002-portable-ip-networking.md` before implementation. It should
record these decisions:

1. The portable crate models observable network state; it does not implement
   an IP stack and does not invent a new socket interface.
2. `embassy-net` owns packet processing, DHCP, DNS, and sockets. A separate
   adapter crate keeps that dependency out of the portable state crate.
3. The platform port owns the vendor controller and exposes its network-driver
   interface. Firmware owns stack resources, task spawning, random seed,
   retry policy, and product probes.
4. Link-up and IP-configured are separate facts. Association must not be
   reported as end-to-end network readiness.
5. DNS availability is also separate: a valid IPv4 lease may contain no DNS
   server.
6. Network resources are statically sized and their socket/buffer budget is
   documented as part of the firmware contract.

## Proposed packages and APIs

### `embedded-sdk-networking`

Create `crates/networking`. It remains `no_std`, allocation-free, and has no
Embassy or vendor dependencies.

Proposed public model:

```rust,ignore
pub const MAX_DNS_SERVERS: usize = 3;

pub enum LinkState {
    Down,
    Up,
}

pub struct DnsServers {
    // Bounded storage with construction and iteration methods; no public raw
    // length field and no allocation.
}

pub struct Ipv4Configuration {
    pub address: core::net::Ipv4Addr,
    pub prefix_len: u8,
    pub gateway: Option<core::net::Ipv4Addr>,
    pub dns_servers: DnsServers,
}

pub struct NetworkSnapshot {
    pub link: LinkState,
    pub ipv4: Option<Ipv4Configuration>,
}
```

The constructors must reject an IPv4 prefix longer than 32 and too many DNS
servers. `NetworkSnapshot::is_ip_ready()` means link up plus an IPv4
configuration. `is_dns_ready()` additionally requires at least one DNS server.
Neither method claims internet reachability.

Add the package to the facade as `embedded_sdk::networking` and cover facade
availability in `tests/host`.

### `embedded-sdk-networking-embassy-net`

Create `crates/networking-embassy-net`. It depends on
`embedded-sdk-networking` and `embassy-net`, but not on an MCU HAL, board, or
`esp-radio`.

Its small wrapper should:

- Hold the copyable `embassy_net::Stack` handle.
- Convert `Stack::is_link_up()` and `Stack::config_v4()` into a
  `NetworkSnapshot`.
- Provide `wait_ip_ready()` and `wait_ip_down()` around the corresponding
  stack wait operations.
- Resolve A records into a caller-owned `&mut [core::net::IpAddr]`, returning
  the number written and a bounded error if the output buffer is too small.
- Expose the raw stack handle for protocol crates and firmware TCP sockets;
  the wrapper must not reimplement socket behavior.

Keep conversion helpers independently testable so host tests do not require a
live network driver.

## ESP32-C6 port refactor

The current `Esp32c6Wifi` owns both `WifiController` and `Interfaces` and its
supervisor borrows the combined value forever. `embassy-net` must instead own
the station `Interface` in its runner task.

Refactor `ports/espressif/esp32c6/src/wifi.rs` as follows:

1. Keep initialization, scanning, and configuration on `Esp32c6Wifi` while it
   owns both values.
2. Introduce an `Esp32c6StationController` wrapper containing the vendor
   `WifiController` and the portable Wi-Fi lifecycle state.
3. Add a consuming `into_station_parts` operation returning the controller
   wrapper and a public station-interface type alias implementing
   `embassy_net_driver::Driver`.
4. Move `connect`, `wait_for_disconnect`, and `state` to the controller
   wrapper. Preserve existing portable error mapping and reconnect behavior.
5. Drop the unused access-point interface explicitly. Do not expose the whole
   vendor `Interfaces` bundle as the normal station API.

Host-visible port metadata must continue compiling on non-RISC-V targets. The
new vendor types remain behind the existing `target_arch = "riscv32"` gate.

## Reference firmware composition

Refactor `firmware/seeed/xiao-esp32c6/src/main.rs` after scan and station
configuration:

1. Consume `Esp32c6Wifi` into a station controller and station interface.
2. Generate the 64-bit `embassy-net` random seed from two hardware RNG words.
3. Allocate `StackResources<3>` in a `StaticCell`:
   - one socket slot for DHCP;
   - one socket slot for DNS;
   - one socket slot for the optional TCP smoke probe.
4. Construct the stack with DHCPv4 configuration.
5. Spawn three independently owned tasks:
   - `station_task`: association, disconnect wait, and bounded reconnection;
   - `network_runner_task`: `embassy_net::Runner::run()`;
   - `network_monitor_task`: link/config transitions and optional probe.
6. Keep heartbeat and BLE tasks independent. Networking failure must degrade
   networking rather than stop the executor or BLE peripheral.

The monitor must distinguish these transitions in diagnostics:

```text
link down -> link up/IP pending -> IPv4 configured -> link/config down
```

Normal logs should report state changes, retry counts, and probe results. They
must not log SSIDs, BSSIDs, device MAC addresses, credentials, or DNS query
contents. Printing a full lease should be limited to an explicitly documented
development diagnostic mode.

### Controlled smoke probe

Add optional build-time settings such as `NETWORK_TEST_HOST` and
`NETWORK_TEST_PORT`. If both are absent, firmware only acquires and monitors a
lease. If both are present, after every newly acquired configuration it should:

1. Resolve the configured hostname to an IPv4 address.
2. Open one TCP connection using fixed caller-owned RX/TX buffers.
3. Report connection success and close cleanly; no public internet endpoint is
   hard-coded.
4. Apply bounded DNS and connect timeouts so a failed probe cannot block
   recovery or heartbeat indefinitely.

Supplying only one of the two settings is a configuration error. The endpoint
must be controlled by the HIL environment; the probe is validation, not a
production application protocol.

## File-level change map

| File or directory | Planned change |
| --- | --- |
| `Cargo.toml` | Add both SDK packages and `embassy-net` 0.9.1 with minimal features. Add `embedded-io-async` 0.7 only if required by direct socket I/O. |
| `Cargo.lock` | Resolve and review the new stack dependencies and duplicate versions. |
| `crates/networking/` | Portable state types, validation, documentation, and unit tests. |
| `crates/networking-embassy-net/` | Stack-state conversion, waits, DNS adapter, and tests. |
| `crates/embedded-sdk/` | Export `networking` from the facade. Do not export the Embassy adapter from the portable facade. |
| `ports/espressif/esp32c6/src/wifi.rs` | Split station controller ownership from the station network interface. |
| `firmware/seeed/xiao-esp32c6/` | Add stack resources, runner, monitor, reconnect composition, and optional DNS/TCP probe. |
| `tests/host/` | Add cross-crate portable model and facade tests. |
| `tests/hil/` | Add an ESP32-C6 networking scenario and serial-event assertions. |
| `tools/xtask/` | Add a hardware-networking command only if it can run deterministically with explicit environment inputs; keep it out of the normal host gate. |
| `docs/adr/0002-portable-ip-networking.md` | Record ownership and abstraction decisions. |
| `docs/connectivity/networking.md` | Document DHCP, DNS, sockets, resource limits, diagnostics, and examples. |
| Existing Wi-Fi, porting, README, and compatibility docs | Replace the layer-2 limitation with the verified support boundary. |

## Implementation sequence

### 1. Architecture and portable model

- Add ADR 0002.
- Implement `embedded-sdk-networking` and its host tests.
- Export it through the facade.
- Run `cargo xtask check` before introducing target-specific work.

This checkpoint proves naming and state semantics without coupling them to
Embassy or ESP32-C6.

### 2. Embassy adapter

- Add the pinned, minimal-feature `embassy-net` dependency.
- Implement stack snapshot conversion, waits, and caller-buffer DNS results.
- Unit-test conversion edge cases: link without lease, lease without DNS,
  maximum DNS servers, IPv4 prefix handling, and output truncation errors.
- Inspect `cargo tree -d` and document unavoidable Embassy version duplicates.

### 3. ESP32-C6 ownership split

- Refactor the Wi-Fi port into initialization and station-controller phases.
- Preserve scan, configuration validation, association reporting, and backoff.
- Cross-compile the firmware immediately after this refactor, before adding
  DHCP, to isolate lifetime or task-ownership failures.

### 4. DHCP and lifecycle integration

- Construct and spawn the stack runner with static resources.
- Run association and packet processing concurrently.
- Add link/IP transition monitoring and recovery.
- Verify that AP loss causes both the portable snapshot and Embassy stack to
  become unconfigured, then reacquire a lease after reassociation.

### 5. DNS and TCP proof

- Add the opt-in controlled endpoint configuration.
- Resolve an A record and complete a bounded TCP connection after every fresh
  lease.
- Exercise failure paths without panic, task exit, credential disclosure, or
  tight retry loops.

### 6. Hardware evidence and documentation

- Run the complete HIL matrix below while BLE remains active.
- Record flash/RAM changes and the chosen stack/socket buffer sizes.
- Update compatibility claims only after hardware validation passes.
- Run all repository quality gates and review logs for secret/identity leaks.

## Verification matrix

### Host and compile checks

- `cargo xtask check`
- `cargo xtask build xiao-esp32c6`
- `cargo tree -d`
- Portable crates build with default features and remain `no_std`.
- Public APIs have documentation and no new warnings under `-D warnings`.

### Hardware-in-the-loop checks

Use a controlled access point, DNS record, and TCP listener. Credentials are
provided through the existing environment mechanism and never stored in the
repository or captured in test output.

1. Cold boot with no credentials: scan-only behavior, BLE, and heartbeat still
   work; no network stack panic.
2. Valid credentials: association is followed by a DHCP lease.
3. Controlled hostname: DNS returns the fixture address and TCP connects.
4. DNS failure: bounded error, no task death, and later queries can succeed.
5. TCP refusal/timeout: bounded error and later probes can succeed.
6. AP loss: link and IPv4 state go down, station backoff runs, heartbeat and BLE
   remain responsive.
7. AP restoration: reassociation, a fresh DHCP configuration, DNS, and TCP all
   recover without reboot.
8. Repeated loss/recovery: at least 20 cycles without heap growth, allocator
   failure, or stalled radio coexistence.
9. BLE coexistence: connect, read, and receive notifications during DHCP,
   probes, and Wi-Fi recovery.
10. Log audit: no SSID, password, BSSID, peer address, or unintended hostname
    appears.

## Resource and reliability constraints

- Stack resources and TCP buffers must be statically allocated and named with
  compile-time constants.
- The implementation must document socket count and every RX/TX buffer size.
- Capture firmware image size before and after the change.
- Measure free heap after radio initialization and after 20 recovery cycles;
  the existing 96 KiB heap must not simply be enlarged without evidence.
- DHCP, DNS, and TCP operations require bounded waits at the product layer.
- A stack-runner task exit is terminal for networking and must be surfaced as
  a failed health state rather than silently ignored.
- Association success resets link retry backoff. DHCP or DNS failure must not
  masquerade as link loss; their retry policy belongs to the network monitor.

## Acceptance criteria

The slice is complete when all of the following are true:

- Portable application code can observe link, IPv4, gateway, and DNS-server
  state without importing `embassy-net` or an Espressif crate.
- The XIAO ESP32C6 firmware acquires DHCPv4 configuration after Wi-Fi
  association.
- A controlled hostname resolves and a TCP connection completes using bounded
  static resources.
- AP loss clears IP readiness and AP restoration recovers association, DHCP,
  DNS, and TCP without reboot.
- Heartbeat and the BLE GATT service remain responsive throughout recovery.
- Host tests and the ESP32-C6 release build pass.
- Hardware evidence covers the validation matrix and resource deltas are
  documented.
- README, networking/Wi-Fi guides, the porting guide, and compatibility matrix
  accurately describe the implemented boundary without implying TLS or
  internet reachability.

## Follow-up order

Once this slice is verified, the recommended next sequence is:

1. Commit an ESP32-C6 flash partition layout and storage backend.
2. Add authenticated BLE provisioning with versioned persistent Wi-Fi config.
3. Add TLS and device identity abstractions.
4. Add MQTT structured telemetry.
5. Add signed OTA with rollback and power-loss testing.

## Primary references

- `esp-radio` 0.18.0 source in the Cargo registry, particularly its
  `embassy_net_driver::Driver` implementation for `wifi::Interface`.
- [`embassy-net` 0.9.1 package documentation](https://docs.rs/embassy-net/0.9.1/embassy_net/)
- [`embassy-net-driver` 0.2.0 interoperability contract](https://docs.rs/embassy-net-driver/0.2.0/embassy_net_driver/)
- [ESP-HAL upstream repository](https://github.com/esp-rs/esp-hal)
