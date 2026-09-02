#![no_std]
#![no_main]
#![doc = "Non-connectable iBeacon firmware for the Seeed Studio XIAO ESP32C6."]

use core::{fmt, str::FromStr};

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_time::{Duration, Timer};
use embedded_sdk_bluetooth::{
    AdvertisingInterval, BeaconUuid, IBEACON_COMPANY_IDENTIFIER, IBeacon, StaticRandomAddress,
};
use embedded_sdk_board_xiao_esp32c6::HARDWARE;
use embedded_sdk_platform_esp32c6::{
    bluetooth::{
        BluetoothConnector, ControllerConfig, ControllerTxPower, Esp32c6Bluetooth,
        static_random_address,
    },
    start_embassy,
};
use esp_backtrace as _;
use esp_hal::gpio::{Level, Output, OutputConfig};
use trouble_host::prelude::*;

const DEFAULT_BEACON_UUID: &str = "7a1e1000-4c2a-4f66-a1d4-3f55b55a1000";
const DEFAULT_BEACON_MAJOR: u16 = 1;
const DEFAULT_MEASURED_POWER_DBM: i8 = -59;
const DEFAULT_RADIO_TX_POWER_DBM: i8 = 9;
// Approach detection waits for the next advertising event, so use the fastest
// interval supported by the portable legacy-advertising configuration.
const DEFAULT_ADVERTISING_INTERVAL_MS: u16 = AdvertisingInterval::MIN_MILLIS;
const BLUETOOTH_RECOVERY_RETRY_MS: u64 = 100;

const BLUETOOTH_CONNECTIONS_MAX: usize = 1;
const BLUETOOTH_L2CAP_CHANNELS_MAX: usize = 1;
const MAX_BEACON_NAME_LENGTH: usize = 29;
const GENERATED_BEACON_NAME_LENGTH: usize = 11;
const GENERATED_BEACON_NAME_PREFIX: &[u8; 7] = b"Beacon ";

esp_bootloader_esp_idf::esp_app_desc!();

#[derive(Clone, Copy)]
struct BeaconSettings {
    name: BeaconName,
    frame: IBeacon,
    interval: AdvertisingInterval,
    radio_tx_power: ControllerTxPower,
    radio_tx_power_dbm: i8,
}

#[derive(Clone, Copy)]
struct BeaconName {
    bytes: [u8; MAX_BEACON_NAME_LENGTH],
    length: u8,
}

impl BeaconName {
    fn from_address(address: StaticRandomAddress) -> Self {
        let address = address.as_bytes();
        let mut bytes = [0; MAX_BEACON_NAME_LENGTH];
        bytes[..GENERATED_BEACON_NAME_PREFIX.len()].copy_from_slice(GENERATED_BEACON_NAME_PREFIX);
        // MAC prefixes commonly identify the vendor and are shared by many
        // boards. Use the final two bytes so nearby boards get useful labels;
        // these are also the bytes used for the default iBeacon minor value.
        bytes[7] = hexadecimal_digit(address[4] >> 4);
        bytes[8] = hexadecimal_digit(address[4] & 0x0f);
        bytes[9] = hexadecimal_digit(address[5] >> 4);
        bytes[10] = hexadecimal_digit(address[5] & 0x0f);
        Self {
            bytes,
            length: GENERATED_BEACON_NAME_LENGTH as u8,
        }
    }

    fn from_str(name: &str) -> Option<Self> {
        if name.is_empty() || name.len() > MAX_BEACON_NAME_LENGTH {
            return None;
        }
        let mut bytes = [0; MAX_BEACON_NAME_LENGTH];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        Some(Self {
            bytes,
            length: name.len() as u8,
        })
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
}

const fn hexadecimal_digit(nibble: u8) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble,
        _ => b'A' + nibble - 10,
    }
}

#[derive(Clone, Copy, Debug)]
enum SettingsError {
    InvalidUuid,
    InvalidName,
    InvalidMajor,
    InvalidMinor,
    InvalidMeasuredPower,
    InvalidAdvertisingInterval,
    UnsupportedRadioTxPower,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUuid => formatter
                .write_str("BEACON_UUID must use canonical 8-4-4-4-12 hexadecimal UUID syntax"),
            Self::InvalidName => {
                formatter.write_str("BEACON_NAME must contain between 1 and 29 UTF-8 bytes")
            }
            Self::InvalidMajor => {
                formatter.write_str("BEACON_MAJOR must be an integer from 0 through 65535")
            }
            Self::InvalidMinor => {
                formatter.write_str("BEACON_MINOR must be an integer from 0 through 65535")
            }
            Self::InvalidMeasuredPower => formatter
                .write_str("BEACON_MEASURED_POWER_DBM must be an integer from -128 through 127"),
            Self::InvalidAdvertisingInterval => {
                formatter.write_str("BEACON_INTERVAL_MS must be an integer from 20 through 10240")
            }
            Self::UnsupportedRadioTxPower => formatter.write_str(
                "BEACON_RADIO_TX_POWER_DBM must be one of -15,-12,-9,-6,-3,0,3,6,9,12,15,18,20",
            ),
        }
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    esp_alloc::heap_allocator!(size: 64 * 1024);
    start_embassy(peripherals.TIMG0, peripherals.SW_INTERRUPT);

    // Keep the active-low user LED off to avoid wasting power while broadcasting.
    let _user_led = Output::new(peripherals.GPIO15, Level::High, OutputConfig::default());
    // GPIO3 enables the XIAO RF switch; GPIO14 selects its on-board antenna.
    let _rf_switch_enable = Output::new(peripherals.GPIO3, Level::Low, OutputConfig::default());
    let _rf_switch_select = Output::new(peripherals.GPIO14, Level::Low, OutputConfig::default());

    let address = static_random_address();
    let settings = match beacon_settings(address) {
        Ok(settings) => settings,
        Err(error) => configuration_failure(error).await,
    };

    let controller_config = ControllerConfig::default()
        .with_max_connections(1)
        .with_default_tx_power(settings.radio_tx_power);
    match Esp32c6Bluetooth::new_with_config(peripherals.BT, controller_config) {
        Ok(bluetooth) => match beacon_task(bluetooth.into_connector(), address, settings) {
            Ok(task) => spawner.spawn(task),
            Err(_) => esp_println::println!("embedded-sdk beacon task allocation failed"),
        },
        Err(error) => esp_println::println!("embedded-sdk beacon controller init failed: {error}"),
    }

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}

async fn configuration_failure(error: SettingsError) -> ! {
    esp_println::println!("embedded-sdk beacon configuration failed: {error}");
    loop {
        Timer::after(Duration::from_secs(30)).await;
    }
}

fn beacon_settings(address: StaticRandomAddress) -> Result<BeaconSettings, SettingsError> {
    let name = match option_env!("BEACON_NAME") {
        Some(name) => BeaconName::from_str(name).ok_or(SettingsError::InvalidName)?,
        None => BeaconName::from_address(address),
    };
    let uuid = BeaconUuid::parse(option_env!("BEACON_UUID").unwrap_or(DEFAULT_BEACON_UUID))
        .map_err(|_| SettingsError::InvalidUuid)?;
    let major = parse_optional(
        option_env!("BEACON_MAJOR"),
        DEFAULT_BEACON_MAJOR,
        SettingsError::InvalidMajor,
    )?;
    let address_bytes = address.as_bytes();
    let default_minor = u16::from_be_bytes([address_bytes[4], address_bytes[5]]);
    let minor = parse_optional(
        option_env!("BEACON_MINOR"),
        default_minor,
        SettingsError::InvalidMinor,
    )?;
    let measured_power = parse_optional(
        option_env!("BEACON_MEASURED_POWER_DBM"),
        DEFAULT_MEASURED_POWER_DBM,
        SettingsError::InvalidMeasuredPower,
    )?;
    let interval_millis = parse_optional(
        option_env!("BEACON_INTERVAL_MS"),
        DEFAULT_ADVERTISING_INTERVAL_MS,
        SettingsError::InvalidAdvertisingInterval,
    )?;
    let interval = AdvertisingInterval::from_millis(interval_millis)
        .map_err(|_| SettingsError::InvalidAdvertisingInterval)?;
    let radio_tx_power_dbm = parse_optional(
        option_env!("BEACON_RADIO_TX_POWER_DBM"),
        DEFAULT_RADIO_TX_POWER_DBM,
        SettingsError::UnsupportedRadioTxPower,
    )?;
    let radio_tx_power = controller_tx_power(radio_tx_power_dbm)?;

    Ok(BeaconSettings {
        name,
        frame: IBeacon::new(uuid, major, minor, measured_power),
        interval,
        radio_tx_power,
        radio_tx_power_dbm,
    })
}

fn parse_optional<T: FromStr>(
    value: Option<&str>,
    default: T,
    error: SettingsError,
) -> Result<T, SettingsError> {
    match value {
        Some(value) => value.parse().map_err(|_| error),
        None => Ok(default),
    }
}

fn controller_tx_power(dbm: i8) -> Result<ControllerTxPower, SettingsError> {
    match dbm {
        -15 => Ok(ControllerTxPower::N15),
        -12 => Ok(ControllerTxPower::N12),
        -9 => Ok(ControllerTxPower::N9),
        -6 => Ok(ControllerTxPower::N6),
        -3 => Ok(ControllerTxPower::N3),
        0 => Ok(ControllerTxPower::N0),
        3 => Ok(ControllerTxPower::P3),
        6 => Ok(ControllerTxPower::P6),
        9 => Ok(ControllerTxPower::P9),
        12 => Ok(ControllerTxPower::P12),
        15 => Ok(ControllerTxPower::P15),
        18 => Ok(ControllerTxPower::P18),
        20 => Ok(ControllerTxPower::P20),
        _ => Err(SettingsError::UnsupportedRadioTxPower),
    }
}

#[embassy_executor::task]
async fn beacon_task(
    connector: BluetoothConnector<'static>,
    address: StaticRandomAddress,
    settings: BeaconSettings,
) {
    let controller: ExternalController<_, 20> = ExternalController::new(connector);
    let mut resources: HostResources<
        DefaultPacketPool,
        BLUETOOTH_CONNECTIONS_MAX,
        BLUETOOTH_L2CAP_CHANNELS_MAX,
    > = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random(address.to_hci_bytes()));
    let Host {
        mut peripheral,
        runner,
        ..
    } = stack.build();

    let _ = join(
        bluetooth_runner(runner),
        advertise_forever(&mut peripheral, settings),
    )
    .await;
}

async fn bluetooth_runner<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) {
    loop {
        if runner.run().await.is_err() {
            esp_println::println!("embedded-sdk beacon host runner failed; retrying");
            Timer::after(Duration::from_millis(BLUETOOTH_RECOVERY_RETRY_MS)).await;
        }
    }
}

async fn advertise_forever<C: Controller>(
    peripheral: &mut Peripheral<'_, C, DefaultPacketPool>,
    settings: BeaconSettings,
) {
    let manufacturer_payload = settings.frame.manufacturer_payload();
    let mut advertising_data = [0_u8; 31];
    let length = match AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::ManufacturerSpecificData {
                company_identifier: IBEACON_COMPANY_IDENTIFIER,
                payload: &manufacturer_payload,
            },
        ],
        &mut advertising_data,
    ) {
        Ok(length) => length,
        Err(_) => {
            esp_println::println!("embedded-sdk beacon payload encoding failed");
            return;
        }
    };
    let mut scan_response_data = [0_u8; 31];
    let scan_response_length = match AdStructure::encode_slice(
        &[AdStructure::CompleteLocalName(settings.name.as_bytes())],
        &mut scan_response_data,
    ) {
        Ok(length) => length,
        Err(_) => {
            esp_println::println!("embedded-sdk beacon scan response encoding failed");
            return;
        }
    };
    let interval = Duration::from_millis(u64::from(settings.interval.as_millis()));
    let parameters = AdvertisementParameters {
        interval_min: interval,
        interval_max: interval,
        ..Default::default()
    };
    let mut report_configuration = true;

    loop {
        let advertiser = match peripheral
            .advertise(
                &parameters,
                Advertisement::NonconnectableScannableUndirected {
                    adv_data: &advertising_data[..length],
                    scan_data: &scan_response_data[..scan_response_length],
                },
            )
            .await
        {
            Ok(advertiser) => advertiser,
            Err(_) => {
                esp_println::println!("embedded-sdk beacon advertising failed; retrying");
                Timer::after(Duration::from_millis(BLUETOOTH_RECOVERY_RETRY_MS)).await;
                continue;
            }
        };

        esp_println::println!("embedded-sdk beacon broadcasting");
        if report_configuration {
            // Report only after the controller has enabled advertising so
            // synchronous serial output cannot delay the first beacon event.
            esp_println::println!(
                "embedded-sdk beacon boot: board={}, chip={}",
                HARDWARE.board,
                HARDWARE.chip
            );
            esp_println::println!(
                "embedded-sdk beacon identity: name={}, uuid={}, major={}, minor={}",
                settings.name.as_str(),
                settings.frame.uuid(),
                settings.frame.major(),
                settings.frame.minor()
            );
            esp_println::println!(
                "embedded-sdk beacon radio: interval_ms={}, measured_power_dbm={}, tx_power_dbm={}",
                settings.interval.as_millis(),
                settings.frame.measured_power(),
                settings.radio_tx_power_dbm
            );
            report_configuration = false;
        }
        // This advertisement cannot accept connections. `accept` keeps the
        // advertiser handle alive and returns only if the controller stops it.
        let _ = advertiser.accept().await;
        esp_println::println!("embedded-sdk beacon advertising stopped; retrying");
        Timer::after(Duration::from_millis(BLUETOOTH_RECOVERY_RETRY_MS)).await;
    }
}
