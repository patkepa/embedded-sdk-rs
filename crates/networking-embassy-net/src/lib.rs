#![no_std]
#![forbid(unsafe_code)]
#![doc = "Adapter from embassy-net to portable embedded-sdk network state."]

use core::{fmt, net::Ipv4Addr};

use embassy_net::{IpAddress, Stack, StaticConfigV4, dns};
use embedded_sdk_networking::{
    ConfigError, DnsServers, Ipv4Configuration, LinkState, NetworkSnapshot,
};

/// Error returned while resolving an IPv4 hostname.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ResolveError {
    /// The hostname is not syntactically valid for the DNS client.
    InvalidName,
    /// The hostname exceeds the DNS client's bounded name storage.
    NameTooLong,
    /// The query failed or returned no usable IPv4 address.
    Failed,
    /// The caller-owned output buffer cannot hold the complete result.
    OutputTooSmall {
        /// Number of IPv4 addresses required to return the complete result.
        required: usize,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => formatter.write_str("invalid DNS name"),
            Self::NameTooLong => formatter.write_str("DNS name is too long"),
            Self::Failed => formatter.write_str("DNS lookup failed"),
            Self::OutputTooSmall { required } => {
                write!(formatter, "DNS output buffer requires {required} entries")
            }
        }
    }
}

impl core::error::Error for ResolveError {}

impl From<dns::Error> for ResolveError {
    fn from(value: dns::Error) -> Self {
        match value {
            dns::Error::InvalidName => Self::InvalidName,
            dns::Error::NameTooLong => Self::NameTooLong,
            dns::Error::Failed => Self::Failed,
        }
    }
}

/// Reusable view of an [`embassy_net::Stack`] through portable SDK state.
#[derive(Clone, Copy)]
pub struct EmbassyNetwork<'d> {
    stack: Stack<'d>,
}

impl<'d> EmbassyNetwork<'d> {
    /// Wraps an initialized Embassy network stack handle.
    #[must_use]
    pub const fn new(stack: Stack<'d>) -> Self {
        Self { stack }
    }

    /// Returns the underlying stack handle for socket construction.
    #[must_use]
    pub const fn stack(&self) -> Stack<'d> {
        self.stack
    }

    /// Captures the current portable link and IPv4 configuration state.
    pub fn snapshot(&self) -> Result<NetworkSnapshot, ConfigError> {
        snapshot_from_parts(self.stack.is_link_up(), self.stack.config_v4())
    }

    /// Waits until both the link and an IPv4 configuration are active.
    ///
    /// This future has no built-in timeout. Product firmware should bound the
    /// wait according to its health and recovery policy.
    pub async fn wait_ip_ready(&self) -> Result<NetworkSnapshot, ConfigError> {
        loop {
            self.stack.wait_link_up().await;
            self.stack.wait_config_up().await;
            let snapshot = self.snapshot()?;
            if snapshot.is_ip_ready() {
                return Ok(snapshot);
            }
        }
    }

    /// Waits until the stack no longer has a valid IP configuration.
    pub async fn wait_ip_down(&self) -> Result<NetworkSnapshot, ConfigError> {
        self.stack.wait_config_down().await;
        self.snapshot()
    }

    /// Resolves all A records into a caller-owned IPv4 address buffer.
    ///
    /// The method returns an error instead of a partial result when `output`
    /// cannot hold every address returned by the bounded Embassy DNS client.
    pub async fn resolve_ipv4(
        &self,
        name: &str,
        output: &mut [Ipv4Addr],
    ) -> Result<usize, ResolveError> {
        let addresses = self.stack.dns_query(name, dns::DnsQueryType::A).await?;
        copy_ipv4_addresses(&addresses, output)
    }
}

fn snapshot_from_parts(
    link_up: bool,
    configuration: Option<StaticConfigV4>,
) -> Result<NetworkSnapshot, ConfigError> {
    let link = if link_up {
        LinkState::Up
    } else {
        LinkState::Down
    };
    let ipv4 = configuration.map(convert_ipv4_configuration).transpose()?;
    Ok(NetworkSnapshot::new(link, ipv4))
}

fn convert_ipv4_configuration(
    configuration: StaticConfigV4,
) -> Result<Ipv4Configuration, ConfigError> {
    let dns_servers = DnsServers::new(configuration.dns_servers.as_slice())?;
    Ipv4Configuration::new(
        configuration.address.address(),
        configuration.address.prefix_len(),
        configuration.gateway,
        dns_servers,
    )
}

fn copy_ipv4_addresses(
    addresses: &[IpAddress],
    output: &mut [Ipv4Addr],
) -> Result<usize, ResolveError> {
    let required = addresses
        .iter()
        .filter(|address| matches!(address, IpAddress::Ipv4(_)))
        .count();
    if required == 0 {
        return Err(ResolveError::Failed);
    }
    if required > output.len() {
        return Err(ResolveError::OutputTooSmall { required });
    }

    let mut len = 0;
    for address in addresses {
        let IpAddress::Ipv4(address) = address;
        output[len] = *address;
        len += 1;
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use core::net::Ipv4Addr;

    use embassy_net::{IpAddress, Ipv4Cidr, StaticConfigV4};

    use super::{ResolveError, copy_ipv4_addresses, snapshot_from_parts};

    #[test]
    fn maps_link_configuration_and_dns_independently() {
        let link_only = snapshot_from_parts(true, None).unwrap();
        assert!(!link_only.is_ip_ready());

        let configuration = StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Addr::new(192, 0, 2, 10), 24),
            gateway: Some(Ipv4Addr::new(192, 0, 2, 1)),
            dns_servers: [Ipv4Addr::new(192, 0, 2, 53)].into_iter().collect(),
        };
        let configured = snapshot_from_parts(true, Some(configuration.clone())).unwrap();
        assert!(configured.is_ip_ready());
        assert!(configured.is_dns_ready());
        assert_eq!(
            configured.ipv4().unwrap().address(),
            Ipv4Addr::new(192, 0, 2, 10)
        );

        let stale = snapshot_from_parts(false, Some(configuration)).unwrap();
        assert!(!stale.is_ip_ready());
        assert!(!stale.is_dns_ready());
    }

    #[test]
    fn dns_copy_is_complete_or_an_error() {
        let addresses = [
            IpAddress::Ipv4(Ipv4Addr::new(192, 0, 2, 1)),
            IpAddress::Ipv4(Ipv4Addr::new(192, 0, 2, 2)),
        ];
        let mut one = [Ipv4Addr::UNSPECIFIED; 1];
        assert_eq!(
            copy_ipv4_addresses(&addresses, &mut one),
            Err(ResolveError::OutputTooSmall { required: 2 })
        );
        assert_eq!(one, [Ipv4Addr::UNSPECIFIED]);

        let mut two = [Ipv4Addr::UNSPECIFIED; 2];
        assert_eq!(copy_ipv4_addresses(&addresses, &mut two), Ok(2));
        assert_eq!(
            two,
            [Ipv4Addr::new(192, 0, 2, 1), Ipv4Addr::new(192, 0, 2, 2)]
        );
    }

    #[test]
    fn empty_dns_result_is_a_failure() {
        let mut output = [Ipv4Addr::UNSPECIFIED; 1];
        assert_eq!(
            copy_ipv4_addresses(&[], &mut output),
            Err(ResolveError::Failed)
        );
    }
}
