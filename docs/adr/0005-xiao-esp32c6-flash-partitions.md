# ADR 0005: XIAO ESP32C6 Flash Partition Ownership

- Status: Accepted for host and compile validation
- Date: 2026-09-04
- Hardware validation: Pending

## Context

The supported Seeed Studio XIAO ESP32C6 has 4 MiB of internal flash. The
existing flashing path used espflash's generated single-factory layout, which
gave the application every byte after `0x10000`. Claiming any tail region for
provisioning would therefore overlap the application partition even when the
current image happened to be smaller.

Provisioning needs an exclusively owned, erase-aligned region large enough for
the sequential storage log, two complete configuration slots, state updates,
and compaction. The layout should also avoid making a later OTA design
impossible.

ESP-IDF partition tables occupy the 4 KiB sector at `0x8000`, data partitions
are aligned to the 4 KiB flash erase sector, and application partitions are
aligned to 64 KiB boundaries. The current combined Wi-Fi, BLE, and MQTT fixture
image is 865,312 bytes before adding provisioning.

## Decision

The XIAO board package owns `partitions.csv` and partition-layout version 1:

| Region | Offset | Size | Purpose |
| --- | ---: | ---: | --- |
| Bootloader/reserved | `0x000000` | `0x008000` | ROM-compatible second-stage bootloader area |
| Partition table | `0x008000` | `0x001000` | ESP-IDF partition table and checksum |
| NVS | `0x009000` | `0x004000` | Reserved ESP-IDF-compatible system data |
| OTA state | `0x00d000` | `0x002000` | Future boot-selection state |
| PHY init | `0x00f000` | `0x001000` | Reserved radio initialization data |
| Factory app | `0x010000` | `0x140000` | Current application image |
| OTA slot 0 | `0x150000` | `0x140000` | Reserved future application slot |
| OTA slot 1 | `0x290000` | `0x140000` | Reserved future application slot |
| Provisioning | `0x3d0000` | `0x020000` | SDK key-value log, exclusively owned |
| Reserved data | `0x3f0000` | `0x010000` | Unassigned; no component may use implicitly |

Every application slot is 1,310,720 bytes. The baseline image therefore uses
66.0% of a slot and leaves 445,408 bytes for provisioning, storage, and other
planned firmware work before an OTA-capable image-size review is required.

The ESP32-C6 port uses `esp-storage`'s standard blocking NOR implementation and
the SDK's existing `BlockingFlash` adapter. Firmware must restrict the complete
flash device to `[0x3d0000, 0x3f0000)` before constructing `SequentialStore`.
No component may infer ownership from currently erased bytes or from the linked
image size.

`espflash.toml` makes the checked-in table and 4 MiB capacity part of normal
flash and save-image operations. The factory partition remains the initial
target; OTA behavior is not enabled or claimed by this decision.

The two product-owned data regions use ESP-IDF's `undefined` data subtype and
stable labels. `espflash` 4.5 cannot currently parse a custom numeric subtype
on a `data` entry even though the underlying format permits it; the labels and
checked-in offsets, rather than the generic subtype, are the ownership keys.

## Consequences

- Application growth beyond 1,310,720 bytes fails image creation instead of
  silently consuming provisioning records.
- Provisioning receives 32 erase sectors (128 KiB), enough to measure a
  conservative sequential-storage page and scratch budget before hardware use.
- Future OTA work has two equally sized reserved slots and explicit OTA state,
  but still requires its own boot, signing, rollback, and interruption design.
- Logical deletion may leave credential bytes in flash. This partition does
  not provide confidentiality, authenticity, or verified secure erase.
- `Capabilities::PERSISTENT_STORAGE` remains unset until repeated write,
  compaction, reboot, radio-latency, and cut-power tests pass on hardware.

## Validation required before capability advertisement

1. Read the flashed partition table back and compare it byte-for-byte with the
   checked-in layout.
2. Prove the flash region adapter rejects reads, writes, and erases outside the
   provisioning partition.
3. Measure erase and write stalls while heartbeat, BLE, and Wi-Fi are active.
4. Exercise repeated compaction and interruption at every storage mutation.
5. Record observed flash identity, erase size, image size, firmware hash, and
   partition-layout version with the HIL result.
