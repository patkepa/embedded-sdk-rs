# Persistent Storage

## Implemented scope

The `embedded-sdk-storage` crate provides:

- The ecosystem `embedded-storage` and `embedded-storage-async` raw flash
  traits through the `blocking` and `asynchronous` modules.
- `BlockingFlash`, an adapter for using a synchronous NOR flash driver with an
  asynchronous storage engine. It does not make the hardware operation
  non-blocking.
- An allocation-free `KeyValueStore` contract for complete byte values.
- `SequentialStore`, a CRC-protected, log-structured implementation with page
  compaction, wear leveling, and interrupted-operation repair.
- Stable numeric keys divided into a 16-bit component namespace and a 16-bit
  component-owned record identifier.
- Backend-independent error categories suitable for health and telemetry
  policy while retaining the concrete flash error.

The portable layer does not define a filesystem, dynamically sized keys,
serialization for application types, encryption, or a flash partition table.
Applications serialize versioned values before storing them. Board packages
own physical storage regions and must ensure that a region cannot overlap the
bootloader, partition table, firmware image, OTA slots, or another writer.

## Basic use

Storage is asynchronous at the key-value boundary because compaction and erase
can be long operations. It uses caller-owned output buffers and a compile-time
working buffer:

```rust,ignore
use embedded_sdk::storage::{
    BlockingFlash, Fetch, Key, KeyValueStore, SequentialStore,
};

const PRODUCT_NAMESPACE: u16 = 0x1000;
const WIFI_CONFIG: Key = Key::new(PRODUCT_NAMESPACE, 1);

// `flash` implements embedded_storage::nor_flash::NorFlash. The board owns
// this erase-aligned range and guarantees that it is not used by firmware.
let flash = BlockingFlash::new(flash);
let mut store = SequentialStore::<_, 256>::new(flash, storage_start..storage_end)?;

store.put(WIFI_CONFIG, encoded_config).await?;

let mut bytes = [0; 251];
match store.get(WIFI_CONFIG, &mut bytes).await? {
    Fetch::Found { len } => activate_config(&bytes[..len])?,
    Fetch::NotFound => provision_device().await?,
}
```

For `SequentialStore<S, N>`, the maximum value size is
`SequentialStore::<S, N>::MAX_VALUE_SIZE`: the working buffer also contains a
four-byte key and one-byte record tag. A stored item must additionally fit in
one flash erase page after storage-engine metadata.

## Durability and cancellation

`put` and `delete` append CRC-protected records. After a successful call, a
reopened store sees the new logical state. If power is lost or either future is
cancelled, recovery can expose either the complete old value or the complete
new value. It does not expose a partially decoded value. Firmware should still
drive mutations to completion whenever power remains available and should test
brownouts on its real flash device.

`clear` directly erases all pages and is deliberately not atomic across the
whole region. An interrupted clear can leave only some prior values. Firmware
must finish another `clear` before treating that region as usable.

The store requires at least two erase pages so one page can act as a recovery
and compaction buffer. Its default uncached index uses no additional dynamic
memory, but reads may scan flash. A product should measure lookup latency,
write amplification, erase endurance, and worst-case compaction time with its
real key and value distribution.

Access is intentionally serialized through `&mut self`. When several Embassy
tasks need persistence, one task should own the store and receive bounded
requests. This also gives product firmware one place to enforce priorities,
timeouts, write coalescing, and shutdown policy.

## Key and format ownership

Published namespace and record numbers are persistent-format identifiers. Do
not renumber or reuse them for different data. Teams should maintain a product
key registry and reserve namespace ranges for independently versioned
components.

Application values need their own schema version and migration policy. The
storage crate treats values as opaque bytes and does not couple them to
`embedded-sdk-config::SchemaVersion` or a particular serializer. Upgrading the
major version of `sequential-storage` is also a persistent-format change and
requires changelog review and migration or factory-reset policy.

## Security boundary

Deletion writes a logical tombstone. Old bytes can remain until normal page
compaction erases them; neither `delete` nor `clear` is a verified secure-erase
primitive. CRCs detect accidental corruption but provide no authenticity or
confidentiality. Secret storage requires a product threat model plus a secure
element or platform-backed flash encryption and authenticated value format.

The portable capability `Capabilities::PERSISTENT_STORAGE` should be
advertised only after a platform and board expose a tested, non-overlapping
region. The current XIAO ESP32-C6 target does not advertise it yet because its
partition layout has not been committed.
