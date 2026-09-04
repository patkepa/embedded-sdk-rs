//! ESP32-C6 Wi-Fi radio adapter.

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

use embassy_time::{Duration, with_timeout};
use embedded_sdk_wifi::{
    AccessPoint, Authentication, ConnectedStation, RegulatoryDomain, Security, Ssid, StationConfig,
    WifiState,
};
use esp_radio::wifi::{
    AuthenticationMethod, Config, ControllerConfig, CountryInfo, Interface, Interfaces,
    WifiController, scan::ScanConfig, sta::StationConfig as EspStationConfig,
};

/// ESP32-C6 station packet interface consumed by an IP stack.
pub type StationInterface<'d> = Interface<'d>;

/// Error reported by the ESP32-C6 Wi-Fi adapter.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The station SSID cannot be represented by the current ESP driver API.
    NonUtf8StationSsid,
    /// A discovered SSID could not be represented by the portable contract.
    InvalidDiscoveredSsid,
    /// The portable authentication mode is not supported by this adapter version.
    UnsupportedAuthentication,
    /// The radio did not finish scanning before the caller's deadline.
    ScanTimeout,
    /// The station did not associate before the caller's deadline.
    AssociationTimeout,
    /// The Espressif radio driver rejected an operation.
    Driver(esp_radio::wifi::WifiError),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonUtf8StationSsid => {
                formatter.write_str("ESP station configuration requires a UTF-8 SSID")
            }
            Self::InvalidDiscoveredSsid => {
                formatter.write_str("ESP driver returned an invalid discovered SSID")
            }
            Self::UnsupportedAuthentication => {
                formatter.write_str("unsupported Wi-Fi authentication mode")
            }
            Self::ScanTimeout => formatter.write_str("Wi-Fi scan timed out"),
            Self::AssociationTimeout => formatter.write_str("Wi-Fi association timed out"),
            Self::Driver(error) => write!(formatter, "ESP Wi-Fi driver error: {error:?}"),
        }
    }
}

impl core::error::Error for Error {}

impl From<esp_radio::wifi::WifiError> for Error {
    fn from(value: esp_radio::wifi::WifiError) -> Self {
        Self::Driver(value)
    }
}

/// Owned ESP32-C6 Wi-Fi controller and network interfaces.
///
/// The controller owns radio lifecycle. [`Self::into_station_parts`] exposes
/// the station interface when an application is ready to attach `embassy-net`.
pub struct Esp32c6Wifi<'d> {
    controller: WifiController<'d>,
    interfaces: Interfaces<'d>,
    state: WifiState,
}

impl<'d> Esp32c6Wifi<'d> {
    /// Initializes the radio in station mode.
    ///
    /// A global allocator and the `esp-rtos` scheduler must already be running.
    pub fn new(
        device: esp_hal::peripherals::WIFI<'d>,
        regulatory_domain: RegulatoryDomain,
    ) -> Result<Self, Error> {
        let country_info = CountryInfo::from(regulatory_domain.country_code().into_bytes())
            .with_start_channel(regulatory_domain.first_channel())
            .with_channel_count(regulatory_domain.channel_count())
            .with_max_tx_power_dbm(regulatory_domain.maximum_tx_power_dbm());
        let controller_config = ControllerConfig::default().with_country_info(country_info);
        let (controller, interfaces) = esp_radio::wifi::new(device, controller_config)?;
        Ok(Self {
            controller,
            interfaces,
            state: WifiState::Ready,
        })
    }

    /// Returns the adapter's high-level lifecycle state.
    pub const fn state(&self) -> WifiState {
        self.state
    }

    /// Performs an active scan and returns at most `maximum_results` APs.
    pub async fn scan(
        &mut self,
        maximum_results: usize,
        timeout: Duration,
    ) -> Result<Vec<AccessPoint>, Error> {
        if maximum_results == 0 {
            self.state = WifiState::Ready;
            return Ok(Vec::new());
        }

        self.state = WifiState::Scanning;
        let scan_config = ScanConfig::default().with_max(maximum_results);
        let result = with_timeout(timeout, self.controller.scan_async(&scan_config)).await;

        match result {
            Ok(Ok(discovered)) => {
                let mut access_points = Vec::with_capacity(discovered.len());
                for access_point in discovered.into_iter().take(maximum_results) {
                    let ssid = match Ssid::new(access_point.ssid.as_bytes()) {
                        Ok(ssid) => ssid,
                        Err(_) => {
                            self.state = WifiState::Ready;
                            return Err(Error::InvalidDiscoveredSsid);
                        }
                    };
                    access_points.push(AccessPoint {
                        ssid,
                        bssid: access_point.bssid,
                        channel: access_point.channel,
                        signal_strength_dbm: access_point.signal_strength,
                        security: map_security(access_point.auth_method),
                    });
                }
                self.state = WifiState::Ready;
                Ok(access_points)
            }
            Ok(Err(error)) => {
                self.state = WifiState::Ready;
                Err(error.into())
            }
            Err(_) => {
                self.state = WifiState::Ready;
                Err(Error::ScanTimeout)
            }
        }
    }

    /// Applies a validated portable station configuration.
    pub fn configure_station(&mut self, station: &StationConfig) -> Result<(), Error> {
        let ssid = station.ssid().as_str().ok_or(Error::NonUtf8StationSsid)?;
        let mut config = EspStationConfig::default()
            .with_ssid(ssid)
            .with_auth_method(map_authentication(station.authentication())?);
        if let Some(passphrase) = station.passphrase() {
            config = config.with_password(passphrase.as_str().into());
        }

        match self.controller.set_config(&Config::Station(config)) {
            Ok(()) => {
                self.state = WifiState::Ready;
                Ok(())
            }
            Err(error) => {
                self.state = WifiState::Failed;
                Err(error.into())
            }
        }
    }

    /// Separates radio lifecycle control from the station packet interface.
    ///
    /// The controller must continue running association and recovery while an
    /// IP stack such as `embassy-net` owns the returned station interface.
    pub fn into_station_parts(self) -> (Esp32c6StationController<'d>, StationInterface<'d>) {
        let station = self.interfaces.station;
        let controller = Esp32c6StationController {
            controller: self.controller,
            state: self.state,
        };
        (controller, station)
    }
}

/// ESP32-C6 station association and radio-lifecycle controller.
///
/// Packet processing is deliberately separated into [`StationInterface`] so
/// an IP-stack runner can own it concurrently with this controller.
pub struct Esp32c6StationController<'d> {
    controller: WifiController<'d>,
    state: WifiState,
}

impl Esp32c6StationController<'_> {
    /// Returns the station controller's high-level lifecycle state.
    pub const fn state(&self) -> WifiState {
        self.state
    }

    /// Associates with the configured station network.
    ///
    /// Association establishes the Wi-Fi link only. An IP stack such as
    /// `embassy-net` must subsequently run DHCP or apply a static address.
    pub async fn connect(&mut self, timeout: Duration) -> Result<ConnectedStation, Error> {
        if self.controller.is_connected() {
            return self.current_connection();
        }

        self.state = WifiState::Connecting;
        match with_timeout(timeout, self.controller.connect_async()).await {
            Ok(Ok(connected)) => {
                let ssid = match Ssid::new(connected.ssid.as_bytes()) {
                    Ok(ssid) => ssid,
                    Err(_) => {
                        self.state = WifiState::Failed;
                        return Err(Error::InvalidDiscoveredSsid);
                    }
                };
                let connected = ConnectedStation {
                    ssid,
                    bssid: connected.bssid,
                    channel: connected.channel,
                    security: map_security(Some(connected.authmode)),
                };
                self.state = WifiState::Connected;
                Ok(connected)
            }
            Ok(Err(error)) => {
                self.state = WifiState::Disconnected;
                Err(error.into())
            }
            Err(_) if self.controller.is_connected() => self.current_connection(),
            Err(_) => {
                self.state = WifiState::Disconnected;
                Err(Error::AssociationTimeout)
            }
        }
    }

    /// Waits until the current station association is lost.
    ///
    /// The driver state is polled at `poll_interval` so a missed disconnect
    /// event cannot leave this future pending forever.
    pub async fn wait_for_disconnect(&mut self, poll_interval: Duration) -> Result<(), Error> {
        loop {
            if !self.controller.is_connected() {
                self.state = WifiState::Disconnected;
                return Ok(());
            }

            match with_timeout(poll_interval, self.controller.wait_for_disconnect_async()).await {
                Ok(Ok(_)) => {
                    self.state = WifiState::Disconnected;
                    return Ok(());
                }
                Ok(Err(_)) | Err(_) if !self.controller.is_connected() => {
                    self.state = WifiState::Disconnected;
                    return Ok(());
                }
                Ok(Err(error)) => {
                    self.state = WifiState::Failed;
                    return Err(error.into());
                }
                Err(_) => {}
            }
        }
    }

    fn current_connection(&mut self) -> Result<ConnectedStation, Error> {
        let access_point = match self.controller.ap_info() {
            Ok(access_point) => access_point,
            Err(error) => {
                self.state = WifiState::Failed;
                return Err(error.into());
            }
        };
        let ssid = match Ssid::new(access_point.ssid.as_bytes()) {
            Ok(ssid) => ssid,
            Err(_) => {
                self.state = WifiState::Failed;
                return Err(Error::InvalidDiscoveredSsid);
            }
        };
        let connected = ConnectedStation {
            ssid,
            bssid: access_point.bssid,
            channel: access_point.channel,
            security: map_security(access_point.auth_method),
        };
        self.state = WifiState::Connected;
        Ok(connected)
    }
}

fn map_authentication(authentication: Authentication) -> Result<AuthenticationMethod, Error> {
    let authentication = match authentication {
        Authentication::Open => AuthenticationMethod::None,
        Authentication::Wpa2Personal => AuthenticationMethod::Wpa2Personal,
        Authentication::Wpa3Personal => AuthenticationMethod::Wpa3Personal,
        Authentication::Wpa2Wpa3Personal => AuthenticationMethod::Wpa2Wpa3Personal,
        _ => return Err(Error::UnsupportedAuthentication),
    };
    Ok(authentication)
}

fn map_security(authentication: Option<AuthenticationMethod>) -> Security {
    match authentication {
        None | Some(AuthenticationMethod::None) => Security::Open,
        Some(AuthenticationMethod::Wep) => Security::Wep,
        Some(AuthenticationMethod::Wpa | AuthenticationMethod::WpaWpa2Personal) => Security::Wpa,
        Some(AuthenticationMethod::Wpa2Personal) => Security::Wpa2Personal,
        Some(AuthenticationMethod::Wpa3Personal | AuthenticationMethod::Wpa2Wpa3Personal) => {
            Security::Wpa3Personal
        }
        Some(AuthenticationMethod::Wpa2Enterprise) => Security::Enterprise,
        Some(_) => Security::Unknown,
    }
}
