#![no_std]
#![no_main]
#![doc = "Passive Wi-Fi diagnostics for DFRobot Beetle ESP32-C6 (DFR1117)."]

use core::{
    fmt,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Instant, Timer};
use embedded_sdk_board_beetle_esp32c6::HARDWARE;
use embedded_sdk_platform_esp32c6::{signed_rx_dbm, start_embassy};
use embedded_sdk_wifi::diagnostics::{Analyzer, Reception};
use esp_backtrace as _;
use esp_radio::wifi::{
    self, ControllerConfig, CountryInfo, PowerSaveMode, SecondaryChannel, sniffer::PromiscuousPkt,
};
use static_cell::StaticCell;

mod telemetry;

const PREFIX_BYTES: usize = 384;
const QUEUE_SIZE: usize = 32;
const DWELL: Duration = Duration::from_secs(5);
const PEERS: usize = 96;
const APS: usize = 32;
static QUEUE: Channel<CriticalSectionRawMutex, Capture, QUEUE_SIZE> = Channel::new();
static DROPPED: AtomicU32 = AtomicU32::new(0);
static ANALYZER: StaticCell<Analyzer<PEERS, APS>> = StaticCell::new();
static PENDING: StaticCell<telemetry::Batch> = StaticCell::new();

struct Capture {
    bytes: [u8; PREFIX_BYTES],
    len: usize,
    rx: Reception,
}

fn receive(packet: PromiscuousPkt<'_>) {
    // esp-radio's default filter delivers management/data. Do not inspect
    // control/MISC payloads; those can have different driver buffer semantics.
    if packet.frame_type > 1 {
        return;
    }
    let len = packet.data.len().min(PREFIX_BYTES);
    let mut capture = Capture {
        bytes: [0; PREFIX_BYTES],
        len,
        rx: Reception {
            at_ms: Instant::now().as_millis(),
            channel: packet.rx_cntl.channel as u8,
            rssi: signed_rx_dbm(packet.rx_cntl.rssi),
            noise_floor: signed_rx_dbm(packet.rx_cntl.noise_floor),
            wire_len: packet.len,
            rx_state: packet.rx_cntl.rx_state,
            rate: packet.rx_cntl.rate,
            format: packet.rx_cntl.cur_bb_format,
            timestamp_us: packet.rx_cntl.timestamp.duration_since_epoch().as_micros(),
        },
    };
    capture.bytes[..len].copy_from_slice(&packet.data[..len]);
    if QUEUE.try_send(capture).is_err() {
        let _ = DROPPED.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            Some(n.saturating_add(1))
        });
    }
}

// Buffer complete lines so serial printing does not repeatedly take its lock
// for every formatting fragment. The callback never logs.
struct Serial {
    bytes: [u8; 768],
    len: usize,
}

impl Serial {
    fn flush(&mut self) {
        if let Ok(text) = core::str::from_utf8(&self.bytes[..self.len]) {
            esp_println::print!("{text}");
        }
        self.len = 0;
    }
}

impl fmt::Write for Serial {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for ch in text.chars() {
            let mut utf8 = [0; 4];
            let bytes = ch.encode_utf8(&mut utf8).as_bytes();
            if self.len + bytes.len() > self.bytes.len() {
                self.flush();
            }
            self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
            if ch == '\n' {
                self.flush();
            }
        }
        Ok(())
    }
}

fn channel_setting() -> Result<Option<u8>, &'static str> {
    match option_env!("WIFI_SCANNER_CHANNEL") {
        None | Some("") | Some("0") => Ok(None),
        Some(value) => match value.parse::<u8>() {
            Ok(channel @ 1..=13) => Ok(Some(channel)),
            _ => Err("WIFI_SCANNER_CHANNEL must be 0 (survey) or 1..13"),
        },
    }
}

fn target_setting() -> Result<Option<[u8; 6]>, &'static str> {
    let Some(value) = option_env!("WIFI_SCANNER_TARGET_MAC") else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    let mut mac = [0; 6];
    let mut parts = value.split(':');
    for byte in &mut mac {
        let part = parts
            .next()
            .filter(|s| s.len() == 2)
            .ok_or("target MAC must be AA:BB:CC:DD:EE:FF")?;
        *byte = u8::from_str_radix(part, 16).map_err(|_| "target MAC contains invalid hex")?;
    }
    if parts.next().is_some() || mac == [0; 6] || mac[0] & 1 != 0 {
        return Err("target MAC must be a nonzero unicast address");
    }
    Ok(Some(mac))
}

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 96 * 1024);
    start_embassy(peripherals.TIMG0, peripherals.SW_INTERRUPT);
    esp_println::println!(
        "embedded-sdk wifi-scanner boot: board={} chip={} driver=esp-radio-0.18",
        HARDWARE.board,
        HARDWARE.chip
    );
    let (locked, target) = match (channel_setting(), target_setting()) {
        (Ok(channel), Ok(target)) => (channel, target),
        (channel, target) => {
            panic!("invalid scanner settings: channel={channel:?} target={target:?}")
        }
    };
    let verbose = match option_env!("WIFI_SCANNER_VERBOSE") {
        None | Some("") | Some("0") => false,
        Some("1") => true,
        _ => panic!("WIFI_SCANNER_VERBOSE must be 0 or 1"),
    };
    esp_println::println!(
        "mode={} channel={locked:?} target={target:?} dwell_ms={} prefix_bytes={PREFIX_BYTES} queue={QUEUE_SIZE}",
        if locked.is_some() {
            "locked"
        } else {
            "survey-1-to-13"
        },
        DWELL.as_millis()
    );
    // No XIAO antenna-switch GPIOs: this is the Beetle DFR1117 board.
    let (mut controller, mut interfaces) = wifi::new(
        peripherals.WIFI,
        ControllerConfig::default().with_country_info(CountryInfo::from(*b"01")),
    )
    .expect("Wi-Fi initialization failed");
    controller
        .set_power_saving(PowerSaveMode::None)
        .expect("disable modem sleep");
    interfaces.sniffer.set_receive_cb(receive);
    let mqtt = telemetry::Settings::from_env().expect("invalid telemetry settings");
    let network = mqtt.as_ref().map(|settings| {
        settings
            .configure_station(&mut controller)
            .expect("configure telemetry station");
        telemetry::start_network(&spawner, interfaces.station)
    });
    let mut buffers = network.map(|_| telemetry::buffers());
    esp_println::println!(
        "direct MQTT telemetry: {}",
        if mqtt.is_some() {
            "enabled"
        } else {
            "disabled"
        }
    );
    let analyzer = ANALYZER.init(Analyzer::new(target));
    let pending = PENDING.init_with(telemetry::Batch::new);
    let mut channel = locked.unwrap_or(1);
    let mut report_end = Instant::now();
    loop {
        controller
            .set_channel(channel, SecondaryChannel::None)
            .expect("set capture channel");
        let started = Instant::now();
        analyzer.begin_window(channel, started.as_millis());
        DROPPED.store(0, Ordering::Relaxed);
        interfaces
            .sniffer
            .set_promiscuous_mode(true)
            .expect("enable sniffer");
        let deadline = started + DWELL;
        loop {
            match select(Timer::at(deadline), QUEUE.receive()).await {
                Either::First(()) => break,
                Either::Second(capture) => {
                    analyzer.observe(&capture.bytes[..capture.len], capture.rx)
                }
            }
        }
        // Stop and drain before snapshotting, so every retained frame belongs
        // to this channel/window and USB backpressure has a measurable gap.
        interfaces
            .sniffer
            .set_promiscuous_mode(false)
            .expect("disable sniffer");
        let stopped = Instant::now();
        while let Ok(capture) = QUEUE.try_receive() {
            analyzer.observe(&capture.bytes[..capture.len], capture.rx);
        }
        let mut serial = Serial {
            bytes: [0; 768],
            len: 0,
        };
        let dropped = DROPPED.load(Ordering::Relaxed);
        let gap = started.duration_since(report_end).as_millis();
        analyzer
            .write_telemetry(&mut serial, stopped.as_millis(), dropped, gap)
            .expect("telemetry formatting");
        if verbose {
            analyzer.write_verbose_report(&mut serial, stopped.as_millis(), dropped, gap)
        } else {
            analyzer.write_report(&mut serial, stopped.as_millis(), dropped, gap)
        }
        .expect("serial formatting");
        serial.flush();
        if mqtt.is_some() {
            pending.push(analyzer, stopped.as_millis(), dropped, gap);
        }
        // Include printing time in the next gap measurement.
        report_end = stopped;
        if pending.is_full()
            && let (Some(settings), Some(network), Some(buffers)) =
                (mqtt.as_ref(), network, buffers.as_deref_mut())
        {
            telemetry::upload(&mut controller, network, settings, pending, buffers).await;
            pending.clear();
        }
        if locked.is_none() {
            channel = if channel == 13 { 1 } else { channel + 1 };
        }
    }
}
