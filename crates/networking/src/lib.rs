#![no_std]
#![forbid(unsafe_code)]
#![doc = "Allocation-free link and IP configuration state."]

use core::{fmt, net::Ipv4Addr};

/// Maximum number of DNS servers retained in a portable IPv4 configuration.
pub const MAX_DNS_SERVERS: usize = 3;

/// Error returned while constructing portable network configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ConfigError {
    /// An IPv4 network prefix cannot contain more than 32 bits.
    InvalidIpv4PrefixLength,
    /// The supplied DNS server list exceeds [`MAX_DNS_SERVERS`].
    TooManyDnsServers,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIpv4PrefixLength => {
                formatter.write_str("IPv4 prefix length exceeds 32 bits")
            }
            Self::TooManyDnsServers => formatter.write_str("too many IPv4 DNS servers"),
        }
    }
}

impl core::error::Error for ConfigError {}

/// Physical or radio link state observed by an IP stack.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum LinkState {
    /// The packet interface cannot currently exchange frames.
    #[default]
    Down,
    /// The packet interface can exchange frames.
    Up,
}

/// Bounded, allocation-free collection of IPv4 DNS server addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DnsServers {
    addresses: [Ipv4Addr; MAX_DNS_SERVERS],
    len: u8,
}

impl DnsServers {
    /// Copies a bounded list of IPv4 DNS server addresses.
    pub fn new(addresses: &[Ipv4Addr]) -> Result<Self, ConfigError> {
        if addresses.len() > MAX_DNS_SERVERS {
            return Err(ConfigError::TooManyDnsServers);
        }

        let mut stored = [Ipv4Addr::UNSPECIFIED; MAX_DNS_SERVERS];
        stored[..addresses.len()].copy_from_slice(addresses);
        Ok(Self {
            addresses: stored,
            len: addresses.len() as u8,
        })
    }

    /// Returns the configured DNS server addresses.
    #[must_use]
    pub fn as_slice(&self) -> &[Ipv4Addr] {
        &self.addresses[..usize::from(self.len)]
    }

    /// Returns the number of configured DNS servers.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns whether no DNS server is configured.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for DnsServers {
    fn default() -> Self {
        Self {
            addresses: [Ipv4Addr::UNSPECIFIED; MAX_DNS_SERVERS],
            len: 0,
        }
    }
}

/// Portable snapshot of an active IPv4 configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4Configuration {
    address: Ipv4Addr,
    prefix_len: u8,
    gateway: Option<Ipv4Addr>,
    dns_servers: DnsServers,
}

impl Ipv4Configuration {
    /// Validates and creates an IPv4 configuration snapshot.
    pub const fn new(
        address: Ipv4Addr,
        prefix_len: u8,
        gateway: Option<Ipv4Addr>,
        dns_servers: DnsServers,
    ) -> Result<Self, ConfigError> {
        if prefix_len > 32 {
            return Err(ConfigError::InvalidIpv4PrefixLength);
        }

        Ok(Self {
            address,
            prefix_len,
            gateway,
            dns_servers,
        })
    }

    /// Returns the assigned unicast IPv4 address.
    #[must_use]
    pub const fn address(&self) -> Ipv4Addr {
        self.address
    }

    /// Returns the IPv4 network prefix length.
    #[must_use]
    pub const fn prefix_len(&self) -> u8 {
        self.prefix_len
    }

    /// Returns the default IPv4 gateway when one is configured.
    #[must_use]
    pub const fn gateway(&self) -> Option<Ipv4Addr> {
        self.gateway
    }

    /// Returns the DNS servers supplied with this configuration.
    #[must_use]
    pub const fn dns_servers(&self) -> &DnsServers {
        &self.dns_servers
    }
}

/// Link and address state observed at one instant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetworkSnapshot {
    link: LinkState,
    ipv4: Option<Ipv4Configuration>,
}

impl NetworkSnapshot {
    /// Creates a snapshot from independently observed link and IPv4 state.
    #[must_use]
    pub const fn new(link: LinkState, ipv4: Option<Ipv4Configuration>) -> Self {
        Self { link, ipv4 }
    }

    /// Returns the observed link state.
    #[must_use]
    pub const fn link(&self) -> LinkState {
        self.link
    }

    /// Returns the current IPv4 configuration when one is active.
    #[must_use]
    pub const fn ipv4(&self) -> Option<&Ipv4Configuration> {
        self.ipv4.as_ref()
    }

    /// Returns whether both the link and an IPv4 configuration are active.
    ///
    /// This does not assert DNS or internet reachability.
    #[must_use]
    pub const fn is_ip_ready(&self) -> bool {
        matches!(self.link, LinkState::Up) && self.ipv4.is_some()
    }

    /// Returns whether IP is ready and at least one DNS server is configured.
    ///
    /// This does not assert that a DNS server is reachable.
    #[must_use]
    pub const fn is_dns_ready(&self) -> bool {
        match &self.ipv4 {
            Some(configuration) => self.is_ip_ready() && !configuration.dns_servers.is_empty(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::net::Ipv4Addr;

    use super::{ConfigError, DnsServers, Ipv4Configuration, LinkState, NetworkSnapshot};

    #[test]
    fn dns_servers_are_bounded() {
        let addresses = [
            Ipv4Addr::new(1, 1, 1, 1),
            Ipv4Addr::new(8, 8, 8, 8),
            Ipv4Addr::new(9, 9, 9, 9),
        ];
        let servers = DnsServers::new(&addresses).unwrap();

        assert_eq!(servers.as_slice(), &addresses);
        assert_eq!(servers.len(), 3);
        assert_eq!(
            DnsServers::new(&[Ipv4Addr::LOCALHOST; 4]),
            Err(ConfigError::TooManyDnsServers)
        );
    }

    #[test]
    fn ipv4_prefix_length_is_validated() {
        let servers = DnsServers::default();

        assert!(Ipv4Configuration::new(Ipv4Addr::new(192, 0, 2, 10), 24, None, servers).is_ok());
        assert_eq!(
            Ipv4Configuration::new(Ipv4Addr::LOCALHOST, 33, None, servers),
            Err(ConfigError::InvalidIpv4PrefixLength)
        );
    }

    #[test]
    fn readiness_keeps_link_ip_and_dns_separate() {
        let no_dns = Ipv4Configuration::new(
            Ipv4Addr::new(192, 0, 2, 10),
            24,
            Some(Ipv4Addr::new(192, 0, 2, 1)),
            DnsServers::default(),
        )
        .unwrap();
        let with_dns = Ipv4Configuration::new(
            no_dns.address(),
            no_dns.prefix_len(),
            no_dns.gateway(),
            DnsServers::new(&[Ipv4Addr::new(192, 0, 2, 53)]).unwrap(),
        )
        .unwrap();

        let link_only = NetworkSnapshot::new(LinkState::Up, None);
        assert!(!link_only.is_ip_ready());

        let stale_configuration = NetworkSnapshot::new(LinkState::Down, Some(with_dns));
        assert!(!stale_configuration.is_ip_ready());
        assert!(!stale_configuration.is_dns_ready());

        let ip_only = NetworkSnapshot::new(LinkState::Up, Some(no_dns));
        assert!(ip_only.is_ip_ready());
        assert!(!ip_only.is_dns_ready());

        let ready = NetworkSnapshot::new(LinkState::Up, Some(with_dns));
        assert!(ready.is_ip_ready());
        assert!(ready.is_dns_ready());
    }
}
