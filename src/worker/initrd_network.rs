//! Apply the identity-bound deployment network policy in Alpine's early userspace.
use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use crate::config::{DnsConfig, DnsMode, Ipv6Policy, NetworkConfig};
use crate::deployment::{
    DeploymentManifest, NetworkInterface, parse_interface_address, read_manifest,
};
use crate::fsutil::{atomic_replace, effective_uid};
use crate::host::sources;
use crate::model::{InstallPlan, Target};
use crate::plan_file::PrivatePlanFile;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::net::IpAddr;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const DNS_SEED: &str = "configs/source-resolver";
const MAX_RESOLVER_BYTES: usize = 16 * 1024;
const O_NONBLOCK: i32 = 0o4000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

impl AddressFamily {
    fn flag(self) -> &'static str {
        match self {
            Self::Ipv4 => "-4",
            Self::Ipv6 => "-6",
        }
    }

    fn ipv6(self) -> bool {
        self == Self::Ipv6
    }
}

#[derive(Debug)]
pub enum InitrdNetworkError {
    InvalidInput,
    Configuration(&'static str),
    Command(io::Error),
    CommandFailed { phase: String, status: Option<i32> },
    Filesystem(io::Error),
}

impl fmt::Display for InitrdNetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("Invalid initrd network configuration"),
            Self::Configuration(reason) => write!(f, "Alpine Live network policy failed: {reason}"),
            Self::Command(error) => write!(f, "Cannot run Alpine Live network command: {error}"),
            Self::CommandFailed { phase, status } => {
                write!(
                    f,
                    "Alpine Live network step {phase} failed with status {status:?}"
                )
            }
            Self::Filesystem(error) => write!(
                f,
                "Cannot access Alpine Live network configuration: {error}"
            ),
        }
    }
}

impl std::error::Error for InitrdNetworkError {}

impl From<io::Error> for InitrdNetworkError {
    fn from(value: io::Error) -> Self {
        Self::Filesystem(value)
    }
}

pub fn interface_for_mac(link_output: &str, mac_addr: &str) -> Option<String> {
    let names = super::firstboot::eth_names_by_mac(link_output, mac_addr, false);
    match names.as_slice() {
        [name] if crate::deployment::valid_interface_name(name) => Some(name.clone()),
        _ => None,
    }
}

fn addresses_from_ip_output(output: &str, family: AddressFamily) -> impl Iterator<Item = &str> {
    let token = if family.ipv6() { "inet6" } else { "inet" };
    output.lines().filter_map(move |line| {
        if line
            .split_whitespace()
            .any(|field| matches!(field, "tentative" | "dadfailed"))
        {
            return None;
        }
        let mut fields = line.split_whitespace().skip_while(|field| *field != token);
        let address = fields.nth(1)?;
        parse_interface_address(address, family.ipv6()).ok()?;
        Some(address)
    })
}

pub fn address_from_ip_output(output: &str, family: AddressFamily) -> Option<String> {
    addresses_from_ip_output(output, family)
        .next()
        .map(str::to_owned)
}

pub fn gateway_from_route_output(output: &str) -> Option<String> {
    let mut fields = output.split_whitespace();
    if fields.next()? != "default" || fields.next()? != "via" {
        return None;
    }
    fields
        .next()?
        .parse::<IpAddr>()
        .ok()
        .map(|addr| addr.to_string())
}

/// The initrd and Live installer read the same private handoff plan and manifest.
pub fn run_from_files<R: CommandRunner>(runner: &mut R) -> Result<(), InitrdNetworkError> {
    let root = Path::new("/");
    if effective_uid() != 0
        || !fs::read_to_string("/proc/cmdline")?
            .split_whitespace()
            .any(|arg| arg == "rustaller_live=1")
    {
        return Err(InitrdNetworkError::Configuration(
            "private Live boot marker required",
        ));
    }
    let mut private = PrivatePlanFile::open("/configs/install.plan")
        .map_err(|_| InitrdNetworkError::InvalidInput)?;
    private.persist();
    let plan = private
        .read()
        .map_err(|_| InitrdNetworkError::InvalidInput)?;
    let manifest = read_manifest(Path::new("/configs/deployment.manifest"))
        .map_err(|_| InitrdNetworkError::InvalidInput)?;
    let options = &plan.system;
    let dns_seed = read_dns_seed(root)?;
    apply_network_policy(
        runner,
        root,
        &manifest,
        &options.network,
        &options.dns,
        dns_seed.as_ref(),
    )
}

/// Seed only valid, non-local upstream resolvers. The systemd stub cannot work
/// across kexec. An explicit user resolver takes precedence over the seed.
pub fn source_dns_seed(root: &Path) -> Option<DnsConfig> {
    [
        root.join("etc/resolv.conf"),
        root.join("run/systemd/resolve/resolv.conf"),
    ]
    .iter()
    .filter_map(|path| read_bounded_resolver(path).ok())
    .find_map(|text| parse_resolver(&text))
}

pub(crate) fn stage_dns_seed(destination: &Path, network: &NetworkConfig) -> io::Result<()> {
    if let Some(mut dns) = source_dns_seed(Path::new("/")) {
        if network.ipv6 == Ipv6Policy::Disable {
            dns.servers.retain(IpAddr::is_ipv4);
        }
        if let Ok(Some(contents)) = dns.resolv_conf() {
            atomic_replace(&destination.join(DNS_SEED), contents.as_bytes(), 0o600)?;
        }
    }
    Ok(())
}

fn read_bounded_resolver(path: &Path) -> io::Result<String> {
    // O_NONBLOCK avoids opening an unexpected FIFO without a writer. The
    // usual distro resolver symlink is allowed on the read-only source host.
    let input = OpenOptions::new()
        .read(true)
        .custom_flags(O_NONBLOCK)
        .open(path)?;
    if !input.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "resolver is not a regular file",
        ));
    }
    let mut text = String::new();
    input
        .take((MAX_RESOLVER_BYTES + 1) as u64)
        .read_to_string(&mut text)?;
    if text.len() > MAX_RESOLVER_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "resolver exceeds size limit",
        ));
    }
    Ok(text)
}

fn parse_resolver(text: &str) -> Option<DnsConfig> {
    let mut dns = DnsConfig {
        mode: DnsMode::Static,
        ..DnsConfig::default()
    };
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() == Some("nameserver")
            && let Some(server) = fields.next().and_then(|value| value.parse::<IpAddr>().ok())
        {
            let one = DnsConfig {
                mode: DnsMode::Static,
                servers: vec![server],
                search_domains: vec![],
            };
            if one.validate().is_ok()
                && !dns.servers.contains(&server.to_canonical())
                && dns.servers.len() < 3
            {
                dns.servers.push(server.to_canonical());
            }
        }
    }
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        if matches!(fields.next(), Some("search" | "domain")) {
            let domains = fields
                .take_while(|field| !field.starts_with('#'))
                .map(str::to_owned)
                .collect();
            dns.search_domains = domains;
        }
    }
    if dns.validate().is_err() {
        dns.search_domains.clear();
    }
    dns.validate().is_ok().then_some(dns)
}

fn read_dns_seed(root: &Path) -> Result<Option<DnsConfig>, InitrdNetworkError> {
    let path = root.join(DNS_SEED);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.uid() != effective_uid() || metadata.mode() & 0o077 != 0 {
        return Err(InitrdNetworkError::InvalidInput);
    }
    parse_resolver(&read_bounded_resolver(&path)?)
        .map(Some)
        .ok_or(InitrdNetworkError::InvalidInput)
}

pub fn apply_network_policy<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    manifest: &DeploymentManifest,
    policy: &NetworkConfig,
    dns: &DnsConfig,
    dns_seed: Option<&DnsConfig>,
) -> Result<(), InitrdNetworkError> {
    manifest
        .validate()
        .map_err(|_| InitrdNetworkError::InvalidInput)?;
    let resolver = dns
        .resolv_conf()
        .map_err(|_| InitrdNetworkError::InvalidInput)?;
    checked(
        runner,
        "ip",
        ["link", "set", "dev", "lo", "up"],
        "initrd-loopback-up",
    )?;
    for network in &manifest.network {
        let interface = wait_for_interface(runner, &network.mac)?;
        reject_wireless(root, &interface)?;
        configure_ipv6(root, &interface, policy, network)?;
        if let Some(mtu) = policy.mtu {
            checked(
                runner,
                "ip",
                ["link", "set", "dev", &interface, "mtu", &mtu.to_string()],
                "initrd-interface-mtu",
            )?;
        }
        checked(
            runner,
            "ip",
            ["link", "set", "dev", &interface, "up"],
            "initrd-interface-up",
        )?;

        for family in [AddressFamily::Ipv4, AddressFamily::Ipv6] {
            if family.ipv6() && policy.ipv6 == Ipv6Policy::Disable {
                continue;
            }
            let (dynamic, wanted, gateway) = family_policy(network, family);
            if !dynamic {
                for address in wanted {
                    checked(
                        runner,
                        "ip",
                        [family.flag(), "addr", "replace", address, "dev", &interface],
                        "initrd-address",
                    )?;
                }
            }
            if let Some(gateway) = gateway
                .filter(|_| !dynamic || (family.ipv6() && network.dhcp6 && !network.accept_ra))
            {
                // An explicitly supplied VPS gateway may be outside the
                // address prefix. Install its link route before its default.
                checked(
                    runner,
                    "ip",
                    [
                        family.flag(),
                        "route",
                        "replace",
                        gateway,
                        "dev",
                        &interface,
                    ],
                    "initrd-onlink-gateway",
                )?;
                checked(
                    runner,
                    "ip",
                    [
                        family.flag(),
                        "route",
                        "replace",
                        "default",
                        "via",
                        gateway,
                        "dev",
                        &interface,
                    ],
                    "initrd-default-route",
                )?;
            }
            let dhcp = if family.ipv6() {
                network.dhcp6
            } else {
                network.dhcp4
            };
            if dhcp {
                let client = if family.ipv6() { "udhcpc6" } else { "udhcpc" };
                let lease = checked(
                    runner,
                    "timeout",
                    ["15", client, "-i", &interface, "-f", "-q", "-n"],
                    "initrd-configured-dhcp",
                );
                if let Err(error) = lease {
                    // RA-driven policies may obtain an address without a
                    // stateful DHCPv6 lease. Only accept independently
                    // established addressing and routing for that policy.
                    if !(family.ipv6()
                        && network.accept_ra
                        && check_family(runner, &interface, network, family)?)
                    {
                        return Err(error);
                    }
                    eprintln!(
                        "Router advertisements satisfy the planned Live IPv6 address and route policy."
                    );
                }
            }
            if !wanted.is_empty() || dynamic {
                wait_for_family(runner, &interface, network, family)?;
            }
        }
    }
    let path = root.join("etc/resolv.conf");
    if let Some(explicit) = resolver {
        atomic_replace(&path, explicit.as_bytes(), 0o644)?;
    } else if read_bounded_resolver(&path)
        .ok()
        .and_then(|text| parse_resolver(&text))
        .is_none()
        && let Some(seed) = dns_seed
        && let Some(contents) = seed
            .resolv_conf()
            .map_err(|_| InitrdNetworkError::InvalidInput)?
    {
        atomic_replace(&path, contents.as_bytes(), 0o644)?;
    }
    Ok(())
}

fn family_policy(
    network: &NetworkInterface,
    family: AddressFamily,
) -> (bool, &[String], Option<&str>) {
    if family.ipv6() {
        (
            network.dhcp6 || network.accept_ra,
            &network.ipv6_addresses,
            network.ipv6_gateway.as_deref(),
        )
    } else {
        (
            network.dhcp4,
            &network.ipv4_addresses,
            network.ipv4_gateway.as_deref(),
        )
    }
}

fn configure_ipv6(
    root: &Path,
    interface: &str,
    policy: &NetworkConfig,
    network: &NetworkInterface,
) -> Result<(), InitrdNetworkError> {
    let directory = root.join("proc/sys/net/ipv6/conf").join(interface);
    if !directory.is_dir() {
        if policy.ipv6 != Ipv6Policy::Disable
            && (policy.ipv6 == Ipv6Policy::Enable
                || network.accept_ra
                || network.dhcp6
                || !network.ipv6_addresses.is_empty())
        {
            return Err(InitrdNetworkError::Configuration(
                "planned IPv6 is unavailable on the Live interface",
            ));
        }
        return Ok(());
    }
    let enabled = policy.ipv6 != Ipv6Policy::Disable;
    let ra = enabled && network.accept_ra;
    for (name, value) in [
        ("disable_ipv6", !enabled),
        ("accept_ra", ra),
        ("autoconf", ra),
    ] {
        fs::write(directory.join(name), if value { "1\n" } else { "0\n" })?;
    }
    Ok(())
}

fn reject_wireless(root: &Path, interface: &str) -> Result<(), InitrdNetworkError> {
    let device = root.join("sys/class/net").join(interface);
    if device.join("wireless").exists() || device.join("phy80211").exists() {
        return Err(InitrdNetworkError::Configuration(
            "wireless requires verified firmware and private re-association before installation",
        ));
    }
    Ok(())
}

fn wait_for_interface<R: CommandRunner>(
    runner: &mut R,
    mac: &str,
) -> Result<String, InitrdNetworkError> {
    for _ in 0..20 {
        let links = probe(runner, ["-o", "link"])?;
        if let Some(name) = interface_for_mac(&links, mac) {
            return Ok(name);
        }
        checked(runner, "sleep", ["1"], "initrd-wait-interface")?;
    }
    Err(InitrdNetworkError::Configuration(
        "cannot bind a unique interface to the recorded MAC",
    ))
}

fn wait_for_family<R: CommandRunner>(
    runner: &mut R,
    interface: &str,
    network: &NetworkInterface,
    family: AddressFamily,
) -> Result<(), InitrdNetworkError> {
    for attempt in 0..15 {
        if check_family(runner, interface, network, family)? {
            return Ok(());
        }
        if attempt < 14 {
            checked(runner, "sleep", ["1"], "initrd-wait-address-route")?;
        }
    }
    Err(InitrdNetworkError::Configuration(if family.ipv6() {
        "planned IPv6 address or route is missing"
    } else {
        "planned IPv4 address or route is missing"
    }))
}

fn check_family<R: CommandRunner>(
    runner: &mut R,
    interface: &str,
    network: &NetworkInterface,
    family: AddressFamily,
) -> Result<bool, InitrdNetworkError> {
    let (dynamic, wanted, gateway) = family_policy(network, family);
    if !dynamic && wanted.is_empty() {
        return Ok(true);
    }
    let output = probe(
        runner,
        [
            family.flag(),
            "-o",
            "addr",
            "show",
            "scope",
            "global",
            "dev",
            interface,
        ],
    )?;
    let assigned = addresses_from_ip_output(&output, family)
        .filter_map(|address| parse_interface_address(address, family.ipv6()).ok())
        .collect::<Vec<_>>();
    if assigned.is_empty() {
        return Ok(false);
    }
    if !dynamic
        && (assigned.len() != wanted.len()
            || !wanted.iter().all(|address| {
                parse_interface_address(address, family.ipv6())
                    .is_ok_and(|address| assigned.contains(&address))
            }))
    {
        return Ok(false);
    }
    if let Some(gateway) = gateway {
        let routes = probe(
            runner,
            [family.flag(), "route", "show", "default", "dev", interface],
        )?;
        return Ok(routes.lines().any(|route| {
            if dynamic && !(family.ipv6() && network.dhcp6 && !network.accept_ra) {
                route.split_whitespace().next() == Some("default")
            } else {
                gateway_from_route_output(route).as_deref()
                    == gateway
                        .parse::<IpAddr>()
                        .ok()
                        .map(|ip| ip.to_string())
                        .as_deref()
            }
        }));
    }
    Ok(true)
}

pub(crate) fn verify_live_network<R: CommandRunner>(
    runner: &mut R,
    plan: &InstallPlan,
    manifest: &DeploymentManifest,
) -> Result<(), InitrdNetworkError> {
    let links = probe(runner, ["-o", "link"])?;
    for network in &manifest.network {
        let interface = interface_for_mac(&links, &network.mac).ok_or(
            InitrdNetworkError::Configuration("planned MAC is missing or ambiguous after kexec"),
        )?;
        reject_wireless(Path::new("/"), &interface)?;
        let attributes = probe(runner, ["-o", "link", "show", "dev", &interface])?;
        let carrier_up = attributes
            .lines()
            .next()
            .and_then(|line| line.split_once('<'))
            .and_then(|(_, flags)| flags.split_once('>'))
            .is_some_and(|(flags, _)| flags.split(',').any(|flag| flag == "LOWER_UP"));
        if !carrier_up
            || !check_family(runner, &interface, network, AddressFamily::Ipv4)?
            || (plan.system.network.ipv6 != Ipv6Policy::Disable
                && !check_family(runner, &interface, network, AddressFamily::Ipv6)?)
        {
            return Err(InitrdNetworkError::Configuration(
                "planned link, address or route changed before disk erase",
            ));
        }
    }
    let endpoints = match plan.target {
        Target::Debian => vec![format!(
            "{}/dists/{}/InRelease",
            sources::DEBIAN_ARCHIVE,
            manifest.debian_codename
        )],
        Target::Arch => ["core", "extra"]
            .map(|repo| format!("{}/{repo}/os/x86_64/{repo}.db", sources::ARCH_MIRROR))
            .to_vec(),
        // Alpine's authenticated APK transactions and Gentoo's authenticated
        // stage3 already exercise their repositories before disk erase.
        _ => vec![],
    };
    for endpoint in endpoints {
        // Preserve normal curl environment (proxy and public CA file) without
        // copying any credentials into the command arguments or error output.
        checked(
            runner,
            "curl",
            [
                "--disable",
                "--fail",
                "--silent",
                "--head",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "10",
                "--max-time",
                "30",
                endpoint.as_str(),
            ],
            "live-repository-reachability",
        )?;
    }
    Ok(())
}

fn probe<R: CommandRunner, const N: usize>(
    runner: &mut R,
    args: [&str; N],
) -> Result<String, InitrdNetworkError> {
    let command = CommandSpec::new("ip", args)
        .phase("live-network-state")
        .inspection_limits();
    let output = runner.run(&command).map_err(InitrdNetworkError::Command)?;
    require_success(&command, output.status)?;
    String::from_utf8(output.stdout).map_err(|_| InitrdNetworkError::InvalidInput)
}

fn checked<R: CommandRunner, const N: usize>(
    runner: &mut R,
    program: &str,
    args: [&str; N],
    phase: &str,
) -> Result<(), InitrdNetworkError> {
    let command = CommandSpec::new(program, args)
        .phase(phase)
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Discard)
        .stderr_mode(OutputMode::Discard);
    let output = runner.run(&command).map_err(InitrdNetworkError::Command)?;
    require_success(&command, output.status)
}

fn require_success(command: &CommandSpec, status: Option<i32>) -> Result<(), InitrdNetworkError> {
    if status != Some(0) {
        return Err(InitrdNetworkError::CommandFailed {
            phase: command.phase.clone(),
            status,
        });
    }
    Ok(())
}

pub(crate) fn valid_mac(value: &str) -> bool {
    let mut octets = value.split(':');
    let Some(first) = octets
        .clone()
        .next()
        .and_then(|part| u8::from_str_radix(part, 16).ok())
    else {
        return false;
    };
    first & 1 == 0
        && value != "00:00:00:00:00:00"
        && (0..6).all(|_| {
            octets.next().is_some_and(|octet| {
                octet.len() == 2 && octet.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        })
        && octets.next().is_none()
}
#[derive(Debug, Eq, PartialEq)]
pub struct ProxyEndpoint {
    pub host: String,
    pub port: u16,
}
impl ProxyEndpoint {
    pub fn parse(value: &str) -> Result<Self, InitrdNetworkError> {
        if value.len() > 8192 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(InitrdNetworkError::InvalidInput);
        }
        let (scheme, rest) = value
            .split_once("://")
            .ok_or(InitrdNetworkError::InvalidInput)?;
        let default = match scheme {
            "http" => 80,
            "https" => 443,
            _ => return Err(InitrdNetworkError::InvalidInput),
        };
        let authority = rest
            .split('/')
            .next()
            .unwrap_or_default()
            .rsplit('@')
            .next()
            .unwrap_or_default();
        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            let (host, suffix) = bracketed
                .split_once(']')
                .ok_or(InitrdNetworkError::InvalidInput)?;
            if host.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(InitrdNetworkError::InvalidInput);
            }
            let port = if suffix.is_empty() {
                default
            } else {
                suffix
                    .strip_prefix(':')
                    .ok_or(InitrdNetworkError::InvalidInput)?
                    .parse()
                    .map_err(|_| InitrdNetworkError::InvalidInput)?
            };
            (host, port)
        } else {
            match authority.rsplit_once(':') {
                Some((host, port)) => (
                    host,
                    port.parse().map_err(|_| InitrdNetworkError::InvalidInput)?,
                ),
                None => (authority, default),
            }
        };
        if host.is_empty()
            || host.len() > 253
            || port == 0
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
        {
            return Err(InitrdNetworkError::InvalidInput);
        }
        if host.parse::<IpAddr>().is_err()
            && !host.trim_end_matches('.').split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label.as_bytes()[0].is_ascii_alphanumeric()
                    && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(InitrdNetworkError::InvalidInput);
        }
        if host.trim_end_matches('.').eq_ignore_ascii_case("localhost")
            || host
                .parse::<IpAddr>()
                .is_ok_and(|ip| ip.is_unspecified() || ip.is_loopback() || ip.is_multicast())
        {
            return Err(InitrdNetworkError::InvalidInput);
        }
        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }
}
