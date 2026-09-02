#![no_std]
#![no_main]
#![doc = "BLE advertisement scanner firmware for the Seeed Studio XIAO ESP32C6."]

use core::{cell::RefCell, fmt, fmt::Write as _, str};

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_sync::blocking_mutex::{Mutex, raw::NoopRawMutex};
use embassy_time::{Duration, Instant, Timer};
use embedded_sdk_board_xiao_esp32c6::HARDWARE;
use embedded_sdk_platform_esp32c6::{
    bluetooth::{BluetoothConnector, ControllerConfig, Esp32c6Bluetooth, static_random_address},
    start_embassy,
};
use esp_backtrace as _;
use esp_hal::gpio::{Level, Output, OutputConfig};
use trouble_host::advertise::AdStructure;
use trouble_host::prelude::*;

const SCAN_WINDOW: Duration = Duration::from_secs(5);
const SCAN_INTERVAL: Duration = Duration::from_millis(100);
const DEVICE_STALE_AFTER_MS: u64 = 15_000;
const DEVICE_CAPACITY: usize = 128;
const REMEMBERED_DEVICE_CAPACITY: usize = 48;
const SNAPSHOT_DISPLAY_LIMIT: usize = 12;
const BEACON_DISPLAY_LIMIT: usize = 24;
const LOCAL_NAME_CAPACITY: usize = 29;

const BEACON_FORMAT_IBEACON: u8 = 1 << 0;
const BEACON_FORMAT_EDDYSTONE_UID: u8 = 1 << 1;
const BEACON_FORMAT_EDDYSTONE_URL: u8 = 1 << 2;
const BEACON_FORMAT_EDDYSTONE_TLM: u8 = 1 << 3;
const BEACON_FORMAT_EDDYSTONE_EID: u8 = 1 << 4;
const BEACON_FORMAT_ALTBEACON: u8 = 1 << 5;

const BLUETOOTH_CONNECTIONS_MAX: usize = 1;
const BLUETOOTH_L2CAP_CHANNELS_MAX: usize = 1;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 64 * 1024);
    start_embassy(peripherals.TIMG0, peripherals.SW_INTERRUPT);

    // Keep the active-low user LED off while scanning.
    let _user_led = Output::new(peripherals.GPIO15, Level::High, OutputConfig::default());
    // GPIO3 enables the XIAO RF switch; GPIO14 selects its on-board antenna.
    let _rf_switch_enable = Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default());
    let _rf_switch_select = Output::new(peripherals.GPIO14, Level::Low, OutputConfig::default());

    esp_println::println!(
        "embedded-sdk beacon scanner boot: board={}, chip={}",
        HARDWARE.board,
        HARDWARE.chip
    );
    esp_println::println!(
        "embedded-sdk beacon scanner output: scan_window_ms={}, stale_after_ms={}, capacity={}, remembered={}, displayed={}",
        SCAN_WINDOW.as_millis(),
        DEVICE_STALE_AFTER_MS,
        DEVICE_CAPACITY,
        REMEMBERED_DEVICE_CAPACITY,
        SNAPSHOT_DISPLAY_LIMIT
    );

    let controller_config = ControllerConfig::default().with_max_connections(1);
    match Esp32c6Bluetooth::new_with_config(peripherals.BT, controller_config) {
        Ok(bluetooth) => {
            scanner_task(bluetooth.into_connector()).await;
        }
        Err(error) => {
            esp_println::println!("embedded-sdk beacon scanner controller init failed: {error}");
        }
    }

    loop {
        Timer::after(Duration::from_secs(30)).await;
    }
}

async fn scanner_task(connector: BluetoothConnector<'static>) {
    let controller: ExternalController<_, 20> = ExternalController::new(connector);
    let mut resources: HostResources<
        DefaultPacketPool,
        BLUETOOTH_CONNECTIONS_MAX,
        BLUETOOTH_L2CAP_CHANNELS_MAX,
    > = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random(static_random_address().to_hci_bytes()));
    let Host {
        central, runner, ..
    } = stack.build();
    let reports = ScanReportHandler::new();
    let mut scanner = Scanner::new(central);
    let config = ScanConfig {
        active: true,
        interval: SCAN_INTERVAL,
        window: SCAN_INTERVAL,
        ..Default::default()
    };

    let _ = join(bluetooth_runner(runner, &reports), async {
        loop {
            match scanner.scan(&config).await {
                Ok(session) => {
                    esp_println::println!("Scanning for {}s...", SCAN_WINDOW.as_secs());
                    Timer::after(SCAN_WINDOW).await;
                    drop(session);
                    reports.print_snapshot(Instant::now().as_millis());
                }
                Err(_) => {
                    esp_println::println!(
                        "embedded-sdk beacon scanner scan start failed; retrying"
                    );
                    Timer::after(Duration::from_secs(1)).await;
                }
            }
        }
    })
    .await;
}

async fn bluetooth_runner<C: Controller, P: PacketPool>(
    mut runner: Runner<'_, C, P>,
    reports: &ScanReportHandler,
) {
    loop {
        if runner.run_with_handler(reports).await.is_err() {
            esp_println::println!("embedded-sdk beacon scanner host runner failed; retrying");
            Timer::after(Duration::from_secs(1)).await;
        }
    }
}

struct ScanReportHandler {
    registry: Mutex<NoopRawMutex, RefCell<DeviceRegistry>>,
}

impl ScanReportHandler {
    const fn new() -> Self {
        Self {
            registry: Mutex::new(RefCell::new(DeviceRegistry::new())),
        }
    }

    fn record(&self, address: Address, rssi: i8, data: &[u8]) {
        let metadata = AdvertisementMetadata::parse(data);
        let now_ms = Instant::now().as_millis();
        self.registry.lock(|registry| {
            registry
                .borrow_mut()
                .record(address, rssi, metadata, now_ms);
        });
    }

    fn record_malformed_report(&self) {
        self.registry.lock(|registry| {
            let mut registry = registry.borrow_mut();
            registry.window_malformed_reports = registry.window_malformed_reports.wrapping_add(1);
        });
    }

    fn print_snapshot(&self, now_ms: u64) {
        let mut snapshot = self.registry.lock(|registry| {
            let mut registry = registry.borrow_mut();
            registry.remove_stale(now_ms);
            let snapshot = *registry;
            for entry in registry.entries.iter_mut().flatten() {
                if let Some(average) = entry.window_average() {
                    entry.previous_window_average = Some(average);
                }
                entry.window_rssi_sum = 0;
                entry.window_reports = 0;
                entry.window_peak_rssi = i8::MIN;
            }
            registry.window_reports = 0;
            registry.window_malformed_reports = 0;
            registry.window_evictions = 0;
            snapshot
        });
        let count = snapshot
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.window_reports > 0)
            .count();
        snapshot.entries.sort_unstable_by(|left, right| {
            right
                .and_then(DeviceEntry::window_average)
                .cmp(&left.and_then(DeviceEntry::window_average))
        });
        let shown = count.min(SNAPSHOT_DISPLAY_LIMIT);
        let lost_count = snapshot
            .previous_remembered
            .iter()
            .flatten()
            .filter(|previous| {
                !snapshot.entries.iter().flatten().any(|current| {
                    current.window_reports > 0 && previous.matches_address(current.address)
                })
            })
            .count();
        let mut remembered = [None; REMEMBERED_DEVICE_CAPACITY];
        for (slot, entry) in remembered.iter_mut().zip(
            snapshot
                .entries
                .iter()
                .flatten()
                .filter(|entry| entry.window_reports > 0),
        ) {
            *slot = Some(DisplayedDevice::from_entry(*entry));
        }
        self.registry.lock(|registry| {
            registry.borrow_mut().previous_remembered = remembered;
        });

        esp_println::println!(
            "Seen {} devices ({} packets, {} malformed, {} evicted); showing strongest {}",
            count,
            snapshot.window_reports,
            snapshot.window_malformed_reports,
            snapshot.window_evictions,
            shown
        );
        esp_println::println!(
            "  AVG DELTA  MAX AGEms PKTS KIND ADDRESS             MFG    BEACON NAME"
        );
        for entry in snapshot
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.window_reports > 0)
            .take(SNAPSHOT_DISPLAY_LIMIT)
        {
            let average = entry.window_average().unwrap_or(i16::MIN);
            esp_println::println!(
                "{:<5} {} {:<4} {:<5} {:<4} {}  {} {} {} {}",
                average,
                RssiDelta(
                    entry
                        .previous_window_average
                        .map(|previous| average - previous)
                ),
                entry.window_peak_rssi,
                now_ms.saturating_sub(entry.last_seen_ms),
                entry.window_reports,
                AddressKind(entry.address.kind),
                entry.address,
                Manufacturer(entry.manufacturer),
                BeaconLabel(entry.beacon),
                DisplayName(entry.name)
            );
        }
        let beacon_count = snapshot
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.window_reports > 0 && entry.beacon.is_beacon())
            .count();
        if beacon_count > 0 {
            esp_println::println!(
                "Detected beacons: {}; showing strongest {}",
                beacon_count,
                beacon_count.min(BEACON_DISPLAY_LIMIT)
            );
            esp_println::println!("  AVG DELTA ADDRESS             MFG    BEACON NAME");
            for entry in snapshot
                .entries
                .iter()
                .flatten()
                .filter(|entry| entry.window_reports > 0 && entry.beacon.is_beacon())
                .take(BEACON_DISPLAY_LIMIT)
            {
                let average = entry.window_average().unwrap_or(i16::MIN);
                esp_println::println!(
                    "{:<5} {} {} {} {} {}",
                    average,
                    RssiDelta(
                        entry
                            .previous_window_average
                            .map(|previous| average - previous)
                    ),
                    entry.address,
                    Manufacturer(entry.manufacturer),
                    BeaconLabel(entry.beacon),
                    DisplayName(entry.name)
                );
            }
        }
        if lost_count > 0 {
            esp_println::println!(
                "Lost since previous scan: {}; showing strongest {}",
                lost_count,
                lost_count.min(SNAPSHOT_DISPLAY_LIMIT)
            );
            esp_println::println!(" LAST AGEms KIND ADDRESS             MFG    BEACON NAME");
            for entry in snapshot
                .previous_remembered
                .iter()
                .flatten()
                .filter(|previous| {
                    !snapshot.entries.iter().flatten().any(|current| {
                        current.window_reports > 0 && previous.matches_address(current.address)
                    })
                })
                .take(SNAPSHOT_DISPLAY_LIMIT)
            {
                esp_println::println!(
                    "{:<5} {:<5} {}  {} {} {} {}",
                    entry.average_rssi,
                    now_ms.saturating_sub(entry.last_seen_ms),
                    AddressKind(entry.address.kind),
                    entry.address,
                    Manufacturer(entry.manufacturer),
                    BeaconLabel(entry.beacon),
                    DisplayName(entry.name)
                );
            }
        }
        esp_println::println!();
    }
}

impl EventHandler for ScanReportHandler {
    fn on_adv_reports(&self, reports: LeAdvReportsIter<'_>) {
        for report in reports {
            match report {
                Ok(report) => self.record(
                    Address {
                        kind: report.addr_kind,
                        addr: report.addr,
                    },
                    report.rssi,
                    report.data,
                ),
                Err(_) => self.record_malformed_report(),
            }
        }
    }
}

#[derive(Clone, Copy)]
struct DeviceRegistry {
    entries: [Option<DeviceEntry>; DEVICE_CAPACITY],
    previous_remembered: [Option<DisplayedDevice>; REMEMBERED_DEVICE_CAPACITY],
    window_reports: u32,
    window_malformed_reports: u32,
    window_evictions: u32,
}

impl DeviceRegistry {
    const fn new() -> Self {
        Self {
            entries: [None; DEVICE_CAPACITY],
            previous_remembered: [None; REMEMBERED_DEVICE_CAPACITY],
            window_reports: 0,
            window_malformed_reports: 0,
            window_evictions: 0,
        }
    }

    fn record(&mut self, address: Address, rssi: i8, metadata: AdvertisementMetadata, now_ms: u64) {
        self.window_reports = self.window_reports.wrapping_add(1);

        let existing = self.entries.iter().position(|entry| {
            entry.is_some_and(|entry| {
                entry.address.kind == address.kind && entry.address.addr == address.addr
            })
        });
        if let Some(index) = existing {
            let entry = self.entries[index]
                .as_mut()
                .expect("an existing device index always contains an entry");
            entry.window_rssi_sum = entry.window_rssi_sum.saturating_add(i32::from(rssi));
            entry.window_reports = entry.window_reports.saturating_add(1);
            entry.window_peak_rssi = entry.window_peak_rssi.max(rssi);
            entry.last_seen_ms = now_ms;
            if metadata.name.is_some() {
                entry.name = metadata.name;
            }
            if metadata.manufacturer.is_some() {
                entry.manufacturer = metadata.manufacturer;
            }
            entry.beacon.merge(metadata.beacon);
            return;
        }

        let index = self
            .entries
            .iter()
            .position(Option::is_none)
            .unwrap_or_else(|| {
                let oldest = self
                    .entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, entry)| entry.map_or(u64::MAX, |entry| entry.last_seen_ms))
                    .map_or(0, |(index, _)| index);
                self.window_evictions = self.window_evictions.wrapping_add(1);
                oldest
            });
        self.entries[index] = Some(DeviceEntry {
            address,
            previous_window_average: None,
            window_rssi_sum: i32::from(rssi),
            window_reports: 1,
            window_peak_rssi: rssi,
            last_seen_ms: now_ms,
            name: metadata.name,
            manufacturer: metadata.manufacturer,
            beacon: metadata.beacon,
        });
    }

    fn remove_stale(&mut self, now_ms: u64) {
        for entry in &mut self.entries {
            if entry.is_some_and(|entry| {
                now_ms.saturating_sub(entry.last_seen_ms) > DEVICE_STALE_AFTER_MS
            }) {
                *entry = None;
            }
        }
    }
}

#[derive(Clone, Copy)]
struct DeviceEntry {
    address: Address,
    previous_window_average: Option<i16>,
    window_rssi_sum: i32,
    window_reports: u32,
    window_peak_rssi: i8,
    last_seen_ms: u64,
    name: Option<LocalName>,
    manufacturer: Option<u16>,
    beacon: BeaconMetadata,
}

impl DeviceEntry {
    fn window_average(self) -> Option<i16> {
        if self.window_reports == 0 {
            return None;
        }
        Some((i64::from(self.window_rssi_sum) / i64::from(self.window_reports)) as i16)
    }
}

#[derive(Clone, Copy)]
struct DisplayedDevice {
    address: Address,
    average_rssi: i16,
    last_seen_ms: u64,
    name: Option<LocalName>,
    manufacturer: Option<u16>,
    beacon: BeaconMetadata,
}

impl DisplayedDevice {
    fn from_entry(entry: DeviceEntry) -> Self {
        Self {
            address: entry.address,
            average_rssi: entry.window_average().unwrap_or(i16::MIN),
            last_seen_ms: entry.last_seen_ms,
            name: entry.name,
            manufacturer: entry.manufacturer,
            beacon: entry.beacon,
        }
    }

    fn matches_address(self, address: Address) -> bool {
        self.address.kind == address.kind && self.address.addr == address.addr
    }
}

#[derive(Clone, Copy)]
struct AdvertisementMetadata {
    name: Option<LocalName>,
    manufacturer: Option<u16>,
    beacon: BeaconMetadata,
}

impl AdvertisementMetadata {
    fn parse(data: &[u8]) -> Self {
        let mut metadata = Self {
            name: None,
            manufacturer: None,
            beacon: BeaconMetadata::new(),
        };
        for structure in AdStructure::decode(data).flatten() {
            match structure {
                AdStructure::CompleteLocalName(name) | AdStructure::ShortenedLocalName(name) => {
                    metadata.name = LocalName::from_bytes(name);
                }
                AdStructure::ServiceData16 {
                    uuid: [0xf0, 0xff],
                    data,
                } => {
                    // Feasycom's read-only general frame uses service UUID 0xFFF0.
                    if data.len() >= 11 {
                        metadata.beacon.feasycom = true;
                    }
                }
                AdStructure::ServiceData16 {
                    uuid: [0xaa, 0xfe],
                    data,
                } => {
                    metadata.beacon.formats |= match data.first().copied() {
                        Some(0x00) => BEACON_FORMAT_EDDYSTONE_UID,
                        Some(0x10) => BEACON_FORMAT_EDDYSTONE_URL,
                        Some(0x20) => BEACON_FORMAT_EDDYSTONE_TLM,
                        Some(0x30) => BEACON_FORMAT_EDDYSTONE_EID,
                        _ => 0,
                    };
                }
                AdStructure::ManufacturerSpecificData {
                    company_identifier,
                    payload,
                } => {
                    metadata.manufacturer = Some(company_identifier);
                    if company_identifier == 0x004c
                        && payload.len() >= 23
                        && payload.starts_with(&[0x02, 0x15])
                    {
                        metadata.beacon.formats |= BEACON_FORMAT_IBEACON;
                    }
                    if payload.len() >= 24 && payload.starts_with(&[0xbe, 0xac]) {
                        metadata.beacon.formats |= BEACON_FORMAT_ALTBEACON;
                    }
                    if company_identifier == 0xfff0 {
                        metadata.beacon.feasycom = true;
                    }
                }
                _ => {}
            }
        }
        metadata
    }
}

#[derive(Clone, Copy)]
struct BeaconMetadata {
    feasycom: bool,
    formats: u8,
}

impl BeaconMetadata {
    const fn new() -> Self {
        Self {
            feasycom: false,
            formats: 0,
        }
    }

    fn merge(&mut self, other: Self) {
        self.feasycom |= other.feasycom;
        self.formats |= other.formats;
    }

    const fn is_beacon(self) -> bool {
        self.feasycom || self.formats != 0
    }
}

#[derive(Clone, Copy)]
struct LocalName {
    bytes: [u8; LOCAL_NAME_CAPACITY],
    len: u8,
}

impl LocalName {
    fn from_bytes(value: &[u8]) -> Option<Self> {
        if value.is_empty() || value.len() > LOCAL_NAME_CAPACITY || str::from_utf8(value).is_err() {
            return None;
        }
        let mut bytes = [0; LOCAL_NAME_CAPACITY];
        bytes[..value.len()].copy_from_slice(value);
        Some(Self {
            bytes,
            len: value.len() as u8,
        })
    }

    fn as_str(&self) -> &str {
        str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }
}

struct AddressKind(AddrKind);

struct RssiDelta(Option<i16>);

impl fmt::Display for RssiDelta {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(delta) => write!(formatter, "{delta:+5}"),
            None => formatter.write_str("    -"),
        }
    }
}

impl fmt::Display for AddressKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == AddrKind::PUBLIC {
            formatter.write_str("pub")
        } else if self.0 == AddrKind::RANDOM {
            formatter.write_str("rnd")
        } else {
            write!(formatter, "{:<3}", self.0.as_raw())
        }
    }
}

struct Manufacturer(Option<u16>);

struct BeaconLabel(BeaconMetadata);

impl fmt::Display for BeaconLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut wrote_label = false;
        if self.0.feasycom {
            formatter.write_str("Feasy")?;
            wrote_label = true;
        }
        for (flag, label) in [
            (BEACON_FORMAT_IBEACON, "iBeacon"),
            (BEACON_FORMAT_EDDYSTONE_UID, "E-UID"),
            (BEACON_FORMAT_EDDYSTONE_URL, "E-URL"),
            (BEACON_FORMAT_EDDYSTONE_TLM, "E-TLM"),
            (BEACON_FORMAT_EDDYSTONE_EID, "E-EID"),
            (BEACON_FORMAT_ALTBEACON, "AltBeacon"),
        ] {
            if self.0.formats & flag != 0 {
                if wrote_label {
                    formatter.write_char('/')?;
                }
                formatter.write_str(label)?;
                wrote_label = true;
            }
        }
        if !wrote_label {
            formatter.write_char('-')?;
        }
        Ok(())
    }
}

impl fmt::Display for Manufacturer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(identifier) => write!(formatter, "0x{identifier:04x}"),
            None => formatter.write_str("-     "),
        }
    }
}

struct DisplayName(Option<LocalName>);

impl fmt::Display for DisplayName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(name) = self.0 else {
            return formatter.write_str("-");
        };

        formatter.write_char('"')?;
        for character in name.as_str().chars() {
            match character {
                '"' | '\\' => {
                    formatter.write_char('\\')?;
                    formatter.write_char(character)?;
                }
                character if character.is_control() => formatter.write_char('.')?,
                character => formatter.write_char(character)?,
            }
        }
        formatter.write_char('"')
    }
}
