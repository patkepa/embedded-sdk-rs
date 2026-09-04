# XIAO ESP32C6 Provisioning HIL Scenario

## Fixture requirements

- A Seeed Studio XIAO ESP32C6 connected through native USB Serial/JTAG.
- The no-fallback fixture image built with
  `cargo xtask build-xiao-esp32c6-hil`.
- A controlled 2.4 GHz access point, DHCP/DNS service, and TCP probe endpoint
  for confirmation tests.
- A fixture capable of interrupting board power during individual flash
  operations for the full recovery matrix.

Fixture output and checked-in evidence must not contain SSIDs, credentials,
configured hostnames, client identifiers, candidate bytes, or device network
addresses.

## Automated non-secret smoke commands

```sh
cargo xtask hil-smoke-xiao-esp32c6 /dev/cu.usbmodemXXXX
cargo xtask hil-negative-xiao-esp32c6 /dev/cu.usbmodemXXXX
cargo xtask hil-reset-xiao-esp32c6 /dev/cu.usbmodemXXXX
```

The smoke command submits a built-in open-network configuration with the SSID
`wifi`. It exists only to exercise framing and durable transitions without
handling a secret. The candidate is expected not to associate in an isolated
fixture and must never be used as a successful-connectivity test. After commit,
the command deliberately starts a second boot and asserts through a framed
status request that attempt exhaustion rolled the device back to
`Unprovisioned`.

The negative command verifies reordered-command rejection, identical-request
replay, conflicting request-ID rejection, malformed and semantically invalid
candidate rejection without durable mutation, repeated abort, and transient
cleanup after a pre-commit serial disconnect and transaction timeout.

## Initial evidence — 2026-09-04

Hardware and build:

- chip: ESP32-C6 revision v0.1;
- flash: 4 MiB;
- secure boot: disabled;
- flash encryption: disabled;
- partition layout: version 1, provisioning at `0x3d0000`, 128 KiB;
- source base: commit `36ac8fb`, plus the diagnostic and host-client follow-up
  represented by this document's working tree;
- release ELF SHA-256:
  `70eb3527a7aac9527f99302ba28e3dd3ceadd510a0da065eb1bb6324be76565c`;
- flash image SHA-256:
  `12264a87e471cba38aa95105aa91a3202dfa4dc4b84c13fba59fd5a530ce11d4`;
- application size: 1,054,928 of 1,310,720 bytes (80.48%).

Observed cases:

| Case | Result | Evidence |
| --- | --- | --- |
| Blank partition boot | Pass | Full 128 KiB pre-test partition contained only erased `0xff`; firmware reported unprovisioned after the 15-second window. |
| Serial transaction | Pass | Begin, submit, validate, and commit each returned the expected correlated response. |
| Pending persistence | Pass | Commit returned reboot-required and the candidate survived the firmware-initiated reboot. |
| Attempt exhaustion | Pass | An additional reset after attempt 1 produced `AttemptsExhausted`, completed rollback of rejected generation 1, and returned to unprovisioned mode. |
| Factory reset | Pass | The authorized framed reset returned success, rebooted, and the next boot reported unprovisioned. |
| Logical-reset boundary | Pass | After reset, 517 bytes in the partition differed from erased `0xff`, confirming that logical deletion is not secure erase. |
| Runtime coexistence | Pass | BLE advertising, heartbeat, Wi-Fi scan, and scan-only operation continued after the fixture window and after rollback/reset. |

Redacted event excerpt:

```text
provisioning fixture ready: window_ms=15000
provisioning fixture closed
provisioning ready: state=unprovisioned
provisioning rollback completed: generation=none, rejected_generation=1, reason=AttemptsExhausted
embedded-sdk bluetooth advertising
embedded-sdk wifi station: unprovisioned; scan-only mode
```

The before/after partition SHA-256 values were respectively
`b5a41c3758763bbec72769fab4a2533bf2db0b6312d93d25a695f9e4b9e02260`
and `71dd6f36e6520d30773eef0169b4ab675efd18eab51914e9099d6329b24cc299`.
Raw partition captures are intentionally not checked in because deleted
records can retain configuration bytes.

## Remaining release evidence

- Confirm a valid secret-bearing configuration against the controlled AP,
  DHCP/DNS service, and TCP endpoint without capturing secret values.
- Reject bad credentials while restoring a prior confirmed generation.
- Interrupt power at every slot, state, attempt, confirmation, rollback, and
  reset mutation boundary.
- Exercise repeated updates through sequential-storage compaction and record
  latency, scheduling impact, and endurance assumptions.
- Measure heartbeat and BLE latency during flash operations and the full
  90-second verification deadline.
- Audit fixture output and CI artifacts for configured identities and secret
  material.

This initial evidence does not enable the persistent-storage compatibility
claim and does not qualify provisioning for production use.
