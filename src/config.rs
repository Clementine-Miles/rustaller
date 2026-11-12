//! Installation policy: profile, DNS, network and optional target swapfile.
use crate::model::Target;
use std::{fmt, net::IpAddr, str::FromStr};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InstallProfile {
    #[default]
    Standard,
    Minimal,
}
impl InstallProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Minimal => "minimal",
        }
    }
}
impl FromStr for InstallProfile {
    type Err = SystemConfigError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "standard" => Ok(Self::Standard),
            "minimal" => Ok(Self::Minimal),
            _ => Err(SystemConfigError::InvalidProfile),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DnsMode {
    #[default]
    Auto,
    Static,
}
impl DnsMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Static => "static",
        }
    }
}
impl FromStr for DnsMode {
    type Err = SystemConfigError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "static" => Ok(Self::Static),
            _ => Err(SystemConfigError::InvalidDnsMode),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DnsConfig {
    pub mode: DnsMode,
    pub servers: Vec<IpAddr>,
    pub search_domains: Vec<String>,
}

impl DnsConfig {
    pub fn validate(&self) -> Result<(), SystemConfigError> {
        match self.mode {
            DnsMode::Auto if !self.servers.is_empty() || !self.search_domains.is_empty() => {
                return Err(SystemConfigError::AutomaticDnsWithValues);
            }
            DnsMode::Static if self.servers.is_empty() || self.servers.len() > 3 => {
                return Err(SystemConfigError::InvalidDnsCount);
            }
            _ => {}
        }
        for (index, address) in self.servers.iter().enumerate() {
            let address = address.to_canonical();
            let invalid = address.is_unspecified()
                || address.is_loopback()
                || address.is_multicast()
                || match address {
                    IpAddr::V4(value) => value.is_broadcast(),
                    IpAddr::V6(value) => value.is_unicast_link_local(),
                };
            if invalid {
                return Err(SystemConfigError::InvalidDnsServer);
            }
            if self.servers[..index]
                .iter()
                .any(|old| old.to_canonical() == address)
            {
                return Err(SystemConfigError::DuplicateDnsServer);
            }
        }
        if self.search_domains.len() > 6
            || self
                .search_domains
                .iter()
                .map(|value| value.len() + 1)
                .sum::<usize>()
                > 255
        {
            return Err(SystemConfigError::InvalidSearchDomain);
        }
        for (index, domain) in self.search_domains.iter().enumerate() {
            if !valid_domain(domain)
                || self.search_domains[..index]
                    .iter()
                    .any(|old| old.eq_ignore_ascii_case(domain))
            {
                return Err(SystemConfigError::InvalidSearchDomain);
            }
        }
        Ok(())
    }
    pub fn resolv_conf(&self) -> Result<Option<String>, SystemConfigError> {
        self.validate()?;
        if self.mode == DnsMode::Auto {
            return Ok(None);
        }
        let mut text = "# Managed by rustaller: explicit target DNS\n".to_owned();
        if !self.search_domains.is_empty() {
            text.push_str(&format!("search {}\n", self.search_domains.join(" ")));
        }
        for server in &self.servers {
            text.push_str(&format!("nameserver {server}\n"));
        }
        Ok(Some(text))
    }
    pub fn networkd_settings(&self) -> Result<Option<String>, SystemConfigError> {
        self.validate()?;
        if self.mode == DnsMode::Auto {
            return Ok(None);
        }
        let mut text = "[Network]\nDNS=\nDomains=\n".to_owned();
        for server in &self.servers {
            text.push_str(&format!("DNS={server}\n"));
        }
        if !self.search_domains.is_empty() {
            text.push_str(&format!("Domains={}\n", self.search_domains.join(" ")));
        }
        text.push_str("[DHCPv4]\nUseDNS=no\nUseDomains=no\n[DHCPv6]\nUseDNS=no\nUseDomains=no\n[IPv6AcceptRA]\nUseDNS=no\nUseDomains=no\n");
        Ok(Some(text))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RootFilesystem {
    #[default]
    Xfs,
    Ext4,
}
impl RootFilesystem {
    pub const ALL: &'static [Self] = &[Self::Xfs, Self::Ext4];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Xfs => "xfs",
            Self::Ext4 => "ext4",
        }
    }
}
impl FromStr for RootFilesystem {
    type Err = SystemConfigError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "xfs" => Ok(Self::Xfs),
            "ext4" => Ok(Self::Ext4),
            _ => Err(SystemConfigError::InvalidFilesystem),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SystemConfig {
    pub root_filesystem: RootFilesystem,
    pub profile: InstallProfile,
    pub with_tools: bool,
    pub keep_docs: bool,
    pub keep_package_cache: bool,
    pub dns: DnsConfig,
    pub network: NetworkConfig,
    pub swap: SwapConfig,
}
impl SystemConfig {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
    pub fn validate(&self, target: Target, hold: Option<u8>) -> Result<(), SystemConfigError> {
        if !self.is_default() && (!target.is_automatic() || hold == Some(1)) {
            return Err(SystemConfigError::UnexpectedRoute);
        }
        self.dns.validate()?;
        self.network
            .validate(target)
            .map_err(SystemConfigError::Network)?;
        self.swap.validate().map_err(SystemConfigError::Swap)?;
        if self.network.ipv6 == Ipv6Policy::Disable
            && self
                .dns
                .servers
                .iter()
                .any(|ip| ip.to_canonical().is_ipv6())
        {
            return Err(SystemConfigError::Network(NetworkConfigError::DnsNeedsIpv6));
        }
        Ok(())
    }
    pub fn strip_docs(&self) -> bool {
        self.profile == InstallProfile::Minimal && !self.keep_docs
    }
    pub fn clear_package_cache(&self) -> bool {
        self.profile == InstallProfile::Minimal && !self.keep_package_cache
    }
    pub fn required_packages(&self, target: Target) -> &'static [&'static str] {
        if self.profile != InstallProfile::Minimal {
            return &[];
        }
        match target {
            Target::Alpine => &["openssh", "ca-certificates", "dhcpcd"],
            Target::Gentoo => &["net-misc/openssh", "app-misc/ca-certificates"],
            _ => &[],
        }
    }
    pub fn tool_packages(&self, target: Target) -> &'static [&'static str] {
        if !self.with_tools {
            return &[];
        }
        match target {
            Target::Alpine | Target::Debian | Target::Arch => &["curl", "less"],
            Target::Gentoo => &["net-misc/curl", "sys-apps/less"],
            _ => &[],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemConfigError {
    InvalidFilesystem,
    InvalidProfile,
    InvalidDnsMode,
    AutomaticDnsWithValues,
    InvalidDnsCount,
    InvalidDnsServer,
    DuplicateDnsServer,
    InvalidSearchDomain,
    UnexpectedRoute,
    Network(NetworkConfigError),
    Swap(SwapConfigError),
}
impl fmt::Display for SystemConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Network(error) = self {
            return error.fmt(f);
        }
        if let Self::Swap(error) = self {
            return error.fmt(f);
        }
        f.write_str(match self {
            Self::InvalidFilesystem => "Root filesystem must be xfs or ext4",
            Self::InvalidProfile => "Profile must be standard or minimal",
            Self::InvalidDnsMode => "DNS mode must be auto or static",
            Self::AutomaticDnsWithValues => "Automatic DNS cannot contain explicit servers or search domains",
            Self::InvalidDnsCount => "Static DNS requires 1-3 upstream IP addresses",
            Self::InvalidDnsServer => "DNS requires a unicast upstream IPv4/IPv6 address without a port or scope",
            Self::DuplicateDnsServer => "Duplicate DNS server address",
            Self::InvalidSearchDomain => "DNS search domains must be unique ASCII domains (at most 6, total 255 bytes)",
            Self::UnexpectedRoute => "Target system settings require an automatic installation, not rescue or interactive media",
            Self::Network(_) | Self::Swap(_) => unreachable!(),
        })
    }
}
impl std::error::Error for SystemConfigError {}

fn valid_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

macro_rules! choice {
    ($name:ident, $default:ident, {$($variant:ident => $text:literal),+}) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum $name { $($variant),+ }
        impl Default for $name { fn default() -> Self { Self::$default } }
        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub const fn as_str(self) -> &'static str { match self { $(Self::$variant => $text),+ } }
        }
        impl FromStr for $name {
            type Err = NetworkConfigError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value { $($text => Ok(Self::$variant),)+ _ => Err(NetworkConfigError::InvalidChoice) }
            }
        }
    };
}
choice!(CongestionControl, Default, {Default => "default", Bbr => "bbr", Cubic => "cubic"});
choice!(QueueDiscipline, Default, {Default => "default", Fq => "fq", FqCodel => "fq_codel"});
choice!(Ipv6Policy, Inherit, {Inherit => "inherit", Enable => "enable", Disable => "disable"});
choice!(AddressPreference, Default, {Default => "default", Ipv4 => "ipv4"});

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkConfig {
    pub congestion: CongestionControl,
    pub qdisc: QueueDiscipline,
    pub ipv6: Ipv6Policy,
    pub address_preference: AddressPreference,
    pub mtu: Option<u16>,
    pub buffer_mib: Option<u16>,
    pub somaxconn: Option<u32>,
    pub syn_backlog: Option<u32>,
    pub netdev_backlog: Option<u32>,
    pub mtu_probing: Option<u8>,
    pub fastopen: Option<u8>,
    pub syncookies: Option<bool>,
}
impl NetworkConfig {
    pub fn validate(&self, target: Target) -> Result<(), NetworkConfigError> {
        if self.congestion == CongestionControl::Bbr && self.qdisc != QueueDiscipline::Fq {
            return Err(NetworkConfigError::BbrRequiresFq);
        }
        if target == Target::Alpine && self.address_preference != AddressPreference::Default {
            return Err(NetworkConfigError::MuslPreference);
        }
        // An inherited IPv6 stack may be active: use IPv6's minimum MTU throughout.
        if self.mtu.is_some_and(|n| !(1280..=9000).contains(&n))
            || self.buffer_mib.is_some_and(|n| !(1..=64).contains(&n))
            || [self.somaxconn, self.syn_backlog, self.netdev_backlog]
                .iter()
                .any(|n| n.is_some_and(|n| !(128..=262144).contains(&n)))
            || self.mtu_probing.is_some_and(|n| n > 2)
            || self.fastopen.is_some_and(|n| n > 3)
        {
            return Err(NetworkConfigError::OutOfRange);
        }
        Ok(())
    }
    pub fn sysctl(&self) -> String {
        let mut text = String::new();
        if self.congestion != CongestionControl::Default {
            text.push_str(&format!(
                "net.ipv4.tcp_congestion_control = {}\n",
                self.congestion.as_str()
            ));
        }
        if self.qdisc != QueueDiscipline::Default {
            text.push_str(&format!(
                "net.core.default_qdisc = {}\n",
                self.qdisc.as_str()
            ));
        }
        if self.ipv6 != Ipv6Policy::Inherit {
            let disable = u8::from(self.ipv6 == Ipv6Policy::Disable);
            for scope in ["all", "default", "lo"] {
                text.push_str(&format!("net.ipv6.conf.{scope}.disable_ipv6 = {disable}\n"));
            }
        }
        if let Some(mib) = self.buffer_mib {
            let bytes = u32::from(mib) * 1024 * 1024;
            text.push_str(&format!("net.core.rmem_max = {bytes}\nnet.core.wmem_max = {bytes}\nnet.ipv4.tcp_rmem = 4096 87380 {bytes}\nnet.ipv4.tcp_wmem = 4096 65536 {bytes}\n"));
        }
        for (key, value) in [
            ("net.core.somaxconn", self.somaxconn),
            ("net.ipv4.tcp_max_syn_backlog", self.syn_backlog),
            ("net.core.netdev_max_backlog", self.netdev_backlog),
            ("net.ipv4.tcp_mtu_probing", self.mtu_probing.map(u32::from)),
            ("net.ipv4.tcp_fastopen", self.fastopen.map(u32::from)),
            ("net.ipv4.tcp_syncookies", self.syncookies.map(u32::from)),
        ] {
            if let Some(value) = value {
                text.push_str(&format!("{key} = {value}\n"));
            }
        }
        text
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkConfigError {
    InvalidChoice,
    OutOfRange,
    BbrRequiresFq,
    MuslPreference,
    DnsNeedsIpv6,
}
impl fmt::Display for NetworkConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidChoice => "Unsupported network policy choice",
            Self::OutOfRange => "Network values: MTU 1280-9000, buffer 1-64 MiB, queues 128-262144, probing 0-2, Fast Open 0-3",
            Self::BbrRequiresFq => "BBR requires explicit fq queue discipline and verified target kernel support",
            Self::MuslPreference => "Alpine/musl does not support glibc gai.conf address preference",
            Self::DnsNeedsIpv6 => "IPv6 DNS cannot be used when target IPv6 is disabled",
        })
    }
}
impl std::error::Error for NetworkConfigError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SwapConfig {
    /// None preserves the existing installer behavior. The target path is /swapfile.
    pub size_mib: Option<u32>,
    pub swappiness: Option<u8>,
}
impl SwapConfig {
    pub fn validate(&self) -> Result<(), SwapConfigError> {
        if self.size_mib.is_some_and(|n| !(64..=65536).contains(&n)) {
            return Err(SwapConfigError::InvalidSize);
        }
        if self.swappiness.is_some_and(|n| n > 200)
            || (self.swappiness.is_some() && self.size_mib.is_none())
        {
            return Err(SwapConfigError::InvalidSwappiness);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwapConfigError {
    InvalidSize,
    InvalidSwappiness,
}
impl fmt::Display for SwapConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidSize => "Target swapfile must be 64-65536 MiB (2048 MiB = 2 GiB)",
            Self::InvalidSwappiness => {
                "Swappiness requires an enabled target swapfile and a value in 0-200"
            }
        })
    }
}
impl std::error::Error for SwapConfigError {}
