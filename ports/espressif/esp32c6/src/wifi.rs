//! ESP32-C6 Wi-Fi radio adapter.

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

use embedded_sdk_wifi::{
    AccessPoint, Authentication, ConnectedStation, Security, Ssid, StationConfig, WifiState,
};
use esp_radio::wifi::{
    AuthenticationMethod, Config, ControllerConfig, Interfaces, WifiController, scan::ScanConfig,
    sta::StationConfig as EspStationConfig,
};

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
/// The controller owns radio lifecycle. [`Self::into_parts`] exposes the
/// station interface when an application is ready to attach `embassy-net`.
pub struct Esp32c6Wifi<'d> {
    controller: WifiController<'d>,
    interfaces: Interfaces<'d>,
    state: WifiState,
}

impl<'d> Esp32c6Wifi<'d> {
    /// Initializes the radio in station mode.
    ///
    /// A global allocator and the `esp-rtos` scheduler must already be running.
    pub fn new(device: esp_hal::peripherals::WIFI<'d>) -> Result<Self, Error> {
        let (controller, interfaces) = esp_radio::wifi::new(device, ControllerConfig::default())?;
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
    pub async fn scan(&mut self, maximum_results: usize) -> Result<Vec<AccessPoint>, Error> {
        self.state = WifiState::Scanning;
        let scan_config = ScanConfig::default().with_max(maximum_results);
        let result = self.controller.scan_async(&scan_config).await;

        match result {
            Ok(discovered) => {
                let mut access_points = Vec::with_capacity(discovered.len());
                for access_point in discovered {
                    let ssid = Ssid::try_from(access_point.ssid.as_str())
                        .map_err(|_| Error::InvalidDiscoveredSsid)?;
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
            Err(error) => {
                self.state = WifiState::Failed;
                Err(error.into())
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

        self.controller.set_config(&Config::Station(config))?;
        self.state = WifiState::Ready;
        Ok(())
    }

    /// Associates with the configured station network.
    ///
    /// Association establishes the Wi-Fi link only. An IP stack such as
    /// `embassy-net` must subsequently run DHCP or apply a static address.
    pub async fn connect(&mut self) -> Result<ConnectedStation, Error> {
        self.state = WifiState::Connecting;
        match self.controller.connect_async().await {
            Ok(connected) => {
                self.state = WifiState::Connected;
                Ok(ConnectedStation {
                    ssid: Ssid::try_from(connected.ssid.as_str())
                        .map_err(|_| Error::InvalidDiscoveredSsid)?,
                    bssid: connected.bssid,
                    channel: connected.channel,
                    security: map_security(Some(connected.authmode)),
                })
            }
            Err(error) => {
                self.state = WifiState::Failed;
                Err(error.into())
            }
        }
    }

    /// Waits until the current station association is lost.
    pub async fn wait_for_disconnect(&mut self) -> Result<(), Error> {
        match self.controller.wait_for_disconnect_async().await {
            Ok(_) => {
                self.state = WifiState::Disconnected;
                Ok(())
            }
            Err(error) => {
                self.state = WifiState::Disconnected;
                Err(error.into())
            }
        }
    }

    /// Releases the platform controller and interfaces for advanced networking.
    pub fn into_parts(self) -> (WifiController<'d>, Interfaces<'d>) {
        (self.controller, self.interfaces)
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
