#![no_std]
#![forbid(unsafe_code)]
#![doc = "Portable persistent storage contracts and flash-backed implementations."]

use core::{fmt, ops::Range};

use embedded_storage::nor_flash as blocking_nor;
use embedded_storage_async::nor_flash as async_nor;
use sequential_storage::{
    cache::{Cache, Uncached},
    map::{MapConfig, MapConfigError, MapStorage, SerializationError, Value},
};

/// Blocking raw-storage traits from the Rust embedded ecosystem.
///
/// Platform flash drivers should implement these traits instead of an
/// SDK-specific raw-flash interface.
pub use embedded_storage as blocking;

/// Asynchronous raw-storage traits from the Rust embedded ecosystem.
///
/// [`SequentialStore`] consumes an asynchronous NOR flash. Wrap a blocking
/// implementation in [`BlockingFlash`] when the hardware operation itself is
/// synchronous.
pub use embedded_storage_async as asynchronous;

/// Stable identifier for one value in a [`KeyValueStore`].
///
/// The namespace separates independently versioned SDK or product components;
/// the record identifies a value owned by that component. Keeping keys numeric
/// makes their representation bounded and allocation-free. Published keys are
/// persistent-format identifiers and must not be renumbered after release.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Key {
    namespace: u16,
    record: u16,
}

impl Key {
    /// Creates a key from a component namespace and record identifier.
    #[must_use]
    pub const fn new(namespace: u16, record: u16) -> Self {
        Self { namespace, record }
    }

    /// Reconstructs a key from its stable persistent representation.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self {
            namespace: (raw >> 16) as u16,
            record: raw as u16,
        }
    }

    /// Returns the component namespace.
    #[must_use]
    pub const fn namespace(self) -> u16 {
        self.namespace
    }

    /// Returns the component-owned record identifier.
    #[must_use]
    pub const fn record(self) -> u16 {
        self.record
    }

    /// Returns the stable persistent representation.
    #[must_use]
    pub const fn to_raw(self) -> u32 {
        (self.namespace as u32) << 16 | self.record as u32
    }
}

/// Result of fetching a value into a caller-owned buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Fetch {
    /// No live value exists for the requested key.
    NotFound,
    /// A value was copied into the start of the supplied buffer.
    Found {
        /// Number of initialized bytes in the buffer.
        len: usize,
    },
}

impl Fetch {
    /// Returns the fetched byte count, or `None` when the key was absent.
    #[must_use]
    pub const fn found_len(self) -> Option<usize> {
        match self {
            Self::NotFound => None,
            Self::Found { len } => Some(len),
        }
    }

    /// Returns whether the key had no live value.
    #[must_use]
    pub const fn is_not_found(self) -> bool {
        matches!(self, Self::NotFound)
    }
}

/// Portable error category for storage health and telemetry policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The underlying storage device failed.
    Backend,
    /// No space remains for the requested value or live records.
    Full,
    /// Persistent data was corrupt and could not be repaired.
    Corrupted,
    /// A caller-provided buffer cannot hold the stored value.
    BufferTooSmall,
    /// The value cannot fit in this store's configured record size.
    ValueTooLarge,
    /// Persistent data is structurally invalid for this SDK format.
    InvalidData,
    /// A storage-engine invariant failed.
    Internal,
}

/// Error returned by [`SequentialStore`].
#[derive(Debug)]
#[non_exhaustive]
pub enum Error<E> {
    /// Error returned by the raw flash implementation.
    Backend(E),
    /// The store cannot retain the requested value and all live records.
    Full,
    /// Corruption remained after the storage engine attempted repair.
    Corrupted,
    /// The destination buffer is too small for the complete stored value.
    BufferTooSmall {
        /// Exact value length required by the fetch operation.
        required: usize,
    },
    /// The value exceeds the configured scratch buffer or flash page limit.
    ValueTooLarge,
    /// A stored record uses an invalid SDK encoding.
    InvalidData,
    /// The storage engine reported an internal invariant failure.
    Internal,
}

impl<E> Error<E> {
    /// Returns a backend-independent category for policy and telemetry.
    #[must_use]
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::Backend(_) => ErrorKind::Backend,
            Self::Full => ErrorKind::Full,
            Self::Corrupted => ErrorKind::Corrupted,
            Self::BufferTooSmall { .. } => ErrorKind::BufferTooSmall,
            Self::ValueTooLarge => ErrorKind::ValueTooLarge,
            Self::InvalidData => ErrorKind::InvalidData,
            Self::Internal => ErrorKind::Internal,
        }
    }
}

impl<E: fmt::Debug> fmt::Display for Error<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(error) => write!(formatter, "storage backend failed: {error:?}"),
            Self::Full => formatter.write_str("persistent storage is full"),
            Self::Corrupted => formatter.write_str("persistent storage is corrupt"),
            Self::BufferTooSmall { required } => {
                write!(formatter, "destination buffer needs {required} bytes")
            }
            Self::ValueTooLarge => formatter.write_str("value is too large for this store"),
            Self::InvalidData => formatter.write_str("stored value has an invalid encoding"),
            Self::Internal => formatter.write_str("storage engine invariant failed"),
        }
    }
}

impl<E: fmt::Debug> core::error::Error for Error<E> {}

/// Invalid flash range supplied to [`SequentialStore::new`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// The configured scratch buffer cannot hold the key and record tag.
    ScratchTooSmall,
    /// The range is reversed or extends beyond the raw flash capacity.
    RangeOutOfBounds,
    /// Range start is not aligned to an erase boundary.
    UnalignedStart,
    /// Range end is not aligned to an erase boundary.
    UnalignedEnd,
    /// At least two erase pages are required for recovery and compaction.
    RangeTooSmall,
    /// The flash erase page cannot contain the storage metadata and one item.
    PageTooSmall,
    /// The flash read or write word is larger than the engine supports.
    WordTooLarge,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScratchTooSmall => formatter.write_str("storage scratch buffer is too small"),
            Self::RangeOutOfBounds => formatter.write_str("storage range is outside the flash"),
            Self::UnalignedStart => formatter.write_str("storage range start is not erase-aligned"),
            Self::UnalignedEnd => formatter.write_str("storage range end is not erase-aligned"),
            Self::RangeTooSmall => formatter.write_str("storage range needs at least two pages"),
            Self::PageTooSmall => formatter.write_str("flash erase page is too small"),
            Self::WordTooLarge => formatter.write_str("flash word exceeds the supported size"),
        }
    }
}

impl core::error::Error for ConfigError {}

/// Asynchronous persistent key-value operations.
///
/// Implementations own serialization and power-loss recovery; callers supply
/// only stable [`Key`] values and byte slices. A successful `put` or `delete`
/// makes the new logical state visible after reopening the store. If either
/// future is cancelled or power is lost, reopening may expose the complete old
/// or complete new value, but never a partially decoded value. Mutations should
/// therefore be driven to completion whenever power remains available.
///
/// The exclusive `&mut self` receiver deliberately serializes access. A
/// firmware service that shares storage between tasks should own the store and
/// accept bounded requests over a channel.
#[allow(async_fn_in_trait)]
pub trait KeyValueStore {
    /// Implementation-specific failure preserving the raw backend error.
    type Error;

    /// Fetches one complete value into `output`.
    ///
    /// Partial values are never returned. If `output` is too small, the error
    /// should report the complete required size where the implementation can
    /// determine it.
    async fn get(&mut self, key: Key, output: &mut [u8]) -> Result<Fetch, Self::Error>;

    /// Atomically replaces the logical value associated with `key`.
    async fn put(&mut self, key: Key, value: &[u8]) -> Result<(), Self::Error>;

    /// Atomically makes `key` absent.
    ///
    /// Deletion is idempotent. Flash-backed implementations may retain the old
    /// bytes until the containing page is erased, so this is not secure erase.
    async fn delete(&mut self, key: Key) -> Result<(), Self::Error>;

    /// Erases every value in this store's exclusively owned region.
    ///
    /// This is destructive and can consume an erase cycle on every page. Unlike
    /// `put` and `delete`, interruption can leave a partially cleared region;
    /// firmware must complete another `clear` before using it again.
    async fn clear(&mut self) -> Result<(), Self::Error>;
}

/// Adapts a blocking ecosystem NOR flash to the asynchronous traits.
///
/// This adapter does not make a blocking hardware operation cooperative. It
/// merely allows honestly synchronous flash drivers to be used by storage
/// engines with an async API.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockingFlash<S> {
    inner: S,
}

impl<S> BlockingFlash<S> {
    /// Wraps a blocking flash implementation.
    #[must_use]
    pub const fn new(inner: S) -> Self {
        Self { inner }
    }

    /// Returns shared access to the wrapped flash.
    #[must_use]
    pub const fn get_ref(&self) -> &S {
        &self.inner
    }

    /// Returns exclusive access to the wrapped flash.
    pub const fn get_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// Releases the wrapped flash.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: blocking_nor::ErrorType> async_nor::ErrorType for BlockingFlash<S> {
    type Error = S::Error;
}

impl<S: blocking_nor::ReadNorFlash> async_nor::ReadNorFlash for BlockingFlash<S> {
    const READ_SIZE: usize = S::READ_SIZE;

    async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        blocking_nor::ReadNorFlash::read(&mut self.inner, offset, bytes)
    }

    fn capacity(&self) -> usize {
        blocking_nor::ReadNorFlash::capacity(&self.inner)
    }
}

impl<S: blocking_nor::NorFlash> async_nor::NorFlash for BlockingFlash<S> {
    const WRITE_SIZE: usize = S::WRITE_SIZE;
    const ERASE_SIZE: usize = S::ERASE_SIZE;

    async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        blocking_nor::NorFlash::erase(&mut self.inner, from, to)
    }

    async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        blocking_nor::NorFlash::write(&mut self.inner, offset, bytes)
    }
}

impl<S: blocking_nor::MultiwriteNorFlash> async_nor::MultiwriteNorFlash for BlockingFlash<S> {}

type UncachedMap<S> = MapStorage<u32, S, Cache<Uncached, Uncached, Uncached, u32>>;

#[repr(align(32))]
struct Scratch<const N: usize>([u8; N]);

enum StoredValue<'a> {
    Live(&'a [u8]),
    Tombstone,
}

impl<'a> Value<'a> for StoredValue<'a> {
    fn serialize_into(&self, buffer: &mut [u8]) -> Result<usize, SerializationError> {
        let (tag, value) = match self {
            Self::Live(value) => (1, *value),
            Self::Tombstone => (0, &[][..]),
        };
        let destination = buffer
            .get_mut(..value.len() + 1)
            .ok_or(SerializationError::BufferTooSmall)?;
        destination[0] = tag;
        destination[1..].copy_from_slice(value);
        Ok(destination.len())
    }

    fn deserialize_from(buffer: &'a [u8]) -> Result<(Self, usize), SerializationError> {
        match buffer.split_first() {
            Some((0, [])) => Ok((Self::Tombstone, 1)),
            Some((1, value)) => Ok((Self::Live(value), buffer.len())),
            _ => Err(SerializationError::InvalidFormat),
        }
    }
}

/// CRC-protected, wear-leveled key-value storage over NOR flash.
///
/// `SCRATCH` is the statically allocated working-buffer size. It must hold the
/// four-byte key, one-byte SDK record tag, and the largest value. The usable
/// value limit is available as [`Self::MAX_VALUE_SIZE`]. The flash range must
/// be exclusive to this store and contain at least two erase pages.
///
/// Records are append-only until page compaction. Both record metadata and data
/// are CRC protected, and recoverable interrupted operations are repaired by
/// the underlying sequential storage engine. The uncached configuration uses
/// no additional persistent index RAM and provides deterministic ownership at
/// the cost of scanning flash during lookups.
pub struct SequentialStore<S: async_nor::NorFlash, const SCRATCH: usize> {
    map: UncachedMap<S>,
    scratch: Scratch<SCRATCH>,
}

impl<S: async_nor::NorFlash, const SCRATCH: usize> SequentialStore<S, SCRATCH> {
    const ENCODING_OVERHEAD: usize = core::mem::size_of::<u32>() + 1;

    /// Largest value that fits in this instance's working buffer.
    pub const MAX_VALUE_SIZE: usize = SCRATCH.saturating_sub(Self::ENCODING_OVERHEAD);

    /// Creates a store over an exclusively owned, erase-aligned flash range.
    pub fn new(flash: S, range: Range<u32>) -> Result<Self, ConfigError> {
        if SCRATCH < Self::ENCODING_OVERHEAD {
            return Err(ConfigError::ScratchTooSmall);
        }
        let range_end = usize::try_from(range.end).unwrap_or(usize::MAX);
        if range.start > range.end || range_end > flash.capacity() {
            return Err(ConfigError::RangeOutOfBounds);
        }
        let config = MapConfig::try_new(range).map_err(ConfigError::from)?;
        Ok(Self {
            map: MapStorage::new(flash, config, Cache::new_uncached()),
            scratch: Scratch([0; SCRATCH]),
        })
    }

    /// Returns the flash range exclusively owned by this store.
    #[must_use]
    pub fn range(&self) -> Range<u32> {
        self.map.flash_range()
    }

    /// Returns exclusive raw access to the flash.
    ///
    /// Mutating the store's range can corrupt the key-value format.
    pub fn flash_mut(&mut self) -> &mut S {
        self.map.flash()
    }

    /// Releases the flash implementation.
    pub fn into_flash(self) -> S {
        self.map.destroy().0
    }
}

impl<S: async_nor::NorFlash, const SCRATCH: usize> KeyValueStore for SequentialStore<S, SCRATCH> {
    type Error = Error<S::Error>;

    async fn get(&mut self, key: Key, output: &mut [u8]) -> Result<Fetch, Self::Error> {
        let stored = self
            .map
            .fetch_item::<StoredValue<'_>>(&mut self.scratch.0, &key.to_raw())
            .await
            .map_err(Error::from)?;

        match stored {
            None | Some(StoredValue::Tombstone) => Ok(Fetch::NotFound),
            Some(StoredValue::Live(value)) if output.len() < value.len() => {
                Err(Error::BufferTooSmall {
                    required: value.len(),
                })
            }
            Some(StoredValue::Live(value)) => {
                output[..value.len()].copy_from_slice(value);
                Ok(Fetch::Found { len: value.len() })
            }
        }
    }

    async fn put(&mut self, key: Key, value: &[u8]) -> Result<(), Self::Error> {
        if value.len() > Self::MAX_VALUE_SIZE {
            return Err(Error::ValueTooLarge);
        }
        self.map
            .store_item(
                &mut self.scratch.0,
                &key.to_raw(),
                &StoredValue::Live(value),
            )
            .await
            .map_err(Error::from)
    }

    async fn delete(&mut self, key: Key) -> Result<(), Self::Error> {
        self.map
            .store_item(&mut self.scratch.0, &key.to_raw(), &StoredValue::Tombstone)
            .await
            .map_err(Error::from)
    }

    async fn clear(&mut self) -> Result<(), Self::Error> {
        self.map.erase_all().await.map_err(Error::from)
    }
}

impl From<MapConfigError> for ConfigError {
    fn from(value: MapConfigError) -> Self {
        match value {
            MapConfigError::StartRangeNotAtPageBoundary => Self::UnalignedStart,
            MapConfigError::EndRangeNotAtPageBoundary => Self::UnalignedEnd,
            MapConfigError::RangeTooSmall => Self::RangeTooSmall,
            MapConfigError::PagesTooSmall => Self::PageTooSmall,
            MapConfigError::WordSizeTooLarge => Self::WordTooLarge,
        }
    }
}

impl<E> From<sequential_storage::Error<E>> for Error<E> {
    fn from(value: sequential_storage::Error<E>) -> Self {
        match value {
            sequential_storage::Error::Storage { value } => Self::Backend(value),
            sequential_storage::Error::FullStorage => Self::Full,
            sequential_storage::Error::Corrupted {} => Self::Corrupted,
            sequential_storage::Error::BufferTooSmall(_) => Self::ValueTooLarge,
            sequential_storage::Error::ItemTooBig | sequential_storage::Error::BufferTooBig => {
                Self::ValueTooLarge
            }
            sequential_storage::Error::SerializationError(SerializationError::BufferTooSmall) => {
                Self::ValueTooLarge
            }
            sequential_storage::Error::SerializationError(_) => Self::InvalidData,
            sequential_storage::Error::LogicBug {} => Self::Internal,
            _ => Self::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::fmt;

    use embassy_futures::block_on;
    use embedded_storage_async::nor_flash::{
        ErrorType, NorFlash, NorFlashError, NorFlashErrorKind, ReadNorFlash,
    };

    use super::{ConfigError, Error, Fetch, Key, KeyValueStore, SequentialStore};

    const CAPACITY: usize = 1_024;
    const PAGE_SIZE: usize = 256;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FlashError {
        OutOfBounds,
        NotAligned,
        NotErased,
        Interrupted,
    }

    impl fmt::Display for FlashError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "{self:?}")
        }
    }

    impl NorFlashError for FlashError {
        fn kind(&self) -> NorFlashErrorKind {
            match self {
                Self::OutOfBounds => NorFlashErrorKind::OutOfBounds,
                Self::NotAligned => NorFlashErrorKind::NotAligned,
                Self::NotErased | Self::Interrupted => NorFlashErrorKind::Other,
            }
        }
    }

    struct RamFlash {
        bytes: [u8; CAPACITY],
        bytes_until_failure: Option<usize>,
    }

    impl RamFlash {
        const fn erased() -> Self {
            Self {
                bytes: [u8::MAX; CAPACITY],
                bytes_until_failure: None,
            }
        }

        fn interrupt_after(&mut self, bytes: usize) {
            self.bytes_until_failure = Some(bytes);
        }

        fn write_byte(&mut self, address: usize, value: u8) -> Result<(), FlashError> {
            if let Some(remaining) = self.bytes_until_failure.as_mut() {
                if *remaining == 0 {
                    self.bytes_until_failure = None;
                    return Err(FlashError::Interrupted);
                }
                *remaining -= 1;
            }
            if self.bytes[address] & value != value {
                return Err(FlashError::NotErased);
            }
            self.bytes[address] &= value;
            Ok(())
        }
    }

    impl ErrorType for RamFlash {
        type Error = FlashError;
    }

    impl ReadNorFlash for RamFlash {
        const READ_SIZE: usize = 1;

        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let start = offset as usize;
            let source = self
                .bytes
                .get(start..start.saturating_add(bytes.len()))
                .ok_or(FlashError::OutOfBounds)?;
            bytes.copy_from_slice(source);
            Ok(())
        }

        fn capacity(&self) -> usize {
            self.bytes.len()
        }
    }

    impl NorFlash for RamFlash {
        const WRITE_SIZE: usize = 1;
        const ERASE_SIZE: usize = PAGE_SIZE;

        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            let range = from as usize..to as usize;
            if range.start > range.end || range.end > self.bytes.len() {
                return Err(FlashError::OutOfBounds);
            }
            if !range.start.is_multiple_of(PAGE_SIZE) || !range.end.is_multiple_of(PAGE_SIZE) {
                return Err(FlashError::NotAligned);
            }
            for address in range {
                if let Some(remaining) = self.bytes_until_failure.as_mut() {
                    if *remaining == 0 {
                        self.bytes_until_failure = None;
                        return Err(FlashError::Interrupted);
                    }
                    *remaining -= 1;
                }
                self.bytes[address] = u8::MAX;
            }
            Ok(())
        }

        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            let start = offset as usize;
            if start.saturating_add(bytes.len()) > self.bytes.len() {
                return Err(FlashError::OutOfBounds);
            }
            for (index, value) in bytes.iter().copied().enumerate() {
                self.write_byte(start + index, value)?;
            }
            Ok(())
        }
    }

    type Store = SequentialStore<RamFlash, 64>;

    #[test]
    fn keys_have_stable_namespaced_representation() {
        let key = Key::new(0x1234, 0xabcd);

        assert_eq!(key.to_raw(), 0x1234_abcd);
        assert_eq!(Key::from_raw(key.to_raw()), key);
        assert_eq!(key.namespace(), 0x1234);
        assert_eq!(key.record(), 0xabcd);
    }

    #[test]
    fn values_survive_reopen_update_delete_and_clear() {
        block_on(async {
            let first = Key::new(1, 1);
            let second = Key::new(2, 1);
            let mut store = Store::new(RamFlash::erased(), 0..CAPACITY as u32).unwrap();

            store.put(first, b"old").await.unwrap();
            store.put(second, b"other namespace").await.unwrap();
            store.put(first, b"new value").await.unwrap();

            let flash = store.into_flash();
            let mut reopened = Store::new(flash, 0..CAPACITY as u32).unwrap();
            let mut output = [0; 32];
            let found = reopened.get(first, &mut output).await.unwrap();
            assert_eq!(found, Fetch::Found { len: 9 });
            assert_eq!(&output[..found.found_len().unwrap()], b"new value");

            reopened.delete(first).await.unwrap();
            assert_eq!(
                reopened.get(first, &mut output).await.unwrap(),
                Fetch::NotFound
            );
            assert_eq!(
                reopened.get(second, &mut output).await.unwrap().found_len(),
                Some(15)
            );

            reopened.clear().await.unwrap();
            assert_eq!(
                reopened.get(second, &mut output).await.unwrap(),
                Fetch::NotFound
            );
        });
    }

    #[test]
    fn fetch_never_returns_a_partial_value() {
        block_on(async {
            let key = Key::new(7, 9);
            let mut store = Store::new(RamFlash::erased(), 0..CAPACITY as u32).unwrap();
            store.put(key, b"complete").await.unwrap();

            let mut short = [0xcc; 3];
            let error = store.get(key, &mut short).await.unwrap_err();
            assert!(matches!(error, Error::BufferTooSmall { required: 8 }));
            assert_eq!(short, [0xcc; 3]);
        });
    }

    #[test]
    fn rejects_invalid_ranges_and_oversized_values() {
        assert!(matches!(
            SequentialStore::<RamFlash, 4>::new(RamFlash::erased(), 0..CAPACITY as u32),
            Err(ConfigError::ScratchTooSmall)
        ));
        assert!(matches!(
            Store::new(RamFlash::erased(), 0..(CAPACITY as u32 + PAGE_SIZE as u32)),
            Err(ConfigError::RangeOutOfBounds)
        ));
        let reversed_start = 512;
        let reversed_end = 256;
        assert!(matches!(
            Store::new(RamFlash::erased(), reversed_start..reversed_end),
            Err(ConfigError::RangeOutOfBounds)
        ));
        assert!(matches!(
            Store::new(RamFlash::erased(), 1..513),
            Err(ConfigError::UnalignedStart)
        ));
        assert!(matches!(
            Store::new(RamFlash::erased(), 0..256),
            Err(ConfigError::RangeTooSmall)
        ));

        block_on(async {
            let mut store = Store::new(RamFlash::erased(), 0..CAPACITY as u32).unwrap();
            let oversized = [0; Store::MAX_VALUE_SIZE + 1];
            assert!(matches!(
                store.put(Key::new(1, 1), &oversized).await,
                Err(Error::ValueTooLarge)
            ));
        });
    }

    #[test]
    fn interrupted_update_recovers_to_one_complete_version() {
        block_on(async {
            let key = Key::new(4, 2);
            let mut store = Store::new(RamFlash::erased(), 0..CAPACITY as u32).unwrap();
            store.put(key, b"before").await.unwrap();

            store.flash_mut().interrupt_after(7);
            assert!(matches!(
                store.put(key, b"after power loss").await,
                Err(Error::Backend(FlashError::Interrupted))
            ));

            let flash = store.into_flash();
            let mut reopened = Store::new(flash, 0..CAPACITY as u32).unwrap();
            let mut output = [0; 32];
            let found = reopened.get(key, &mut output).await.unwrap();
            let value = &output[..found.found_len().unwrap()];
            assert!(value == b"before" || value == b"after power loss");
        });
    }

    #[test]
    fn repeated_updates_compact_pages_without_losing_the_live_value() {
        block_on(async {
            let key = Key::new(9, 1);
            let mut store = Store::new(RamFlash::erased(), 0..CAPACITY as u32).unwrap();

            for generation in 0_u32..100 {
                store.put(key, &generation.to_le_bytes()).await.unwrap();
            }

            let flash = store.into_flash();
            let mut reopened = Store::new(flash, 0..CAPACITY as u32).unwrap();
            let mut output = [0; 4];
            assert_eq!(
                reopened.get(key, &mut output).await.unwrap(),
                Fetch::Found { len: 4 }
            );
            assert_eq!(u32::from_le_bytes(output), 99);
        });
    }
}
