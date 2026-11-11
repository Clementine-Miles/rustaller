use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::IpAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const IPV4_DNS: [&str; 2] = ["1.1.1.1", "8.8.8.8"];
const IPV6_DNS: [&str; 2] = ["2606:4700:4700::1111", "2001:4860:4860::8888"];
const DHCP_TIMEOUT: &str = "15";
const DNS_FILE_TIMEOUT: &str = "5";
const TEST_TIMEOUT: &str = "10";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitrdNetworkConfig {
    mac_addr: String,
    ipv4_addr: String,
    ipv4_gateway: String,
    ipv6_addr: String,
    ipv6_gateway: String,
    ipv6_extra_addrs: String,
}

impl InitrdNetworkConfig {
    pub fn new(
        mac_addr: impl Into<String>,
        ipv4_addr: impl Into<String>,
        ipv4_gateway: impl Into<String>,
        ipv6_addr: impl Into<String>,
        ipv6_gateway: impl Into<String>,
        ipv6_extra_addrs: impl Into<String>,
    ) -> Result<Self, InitrdNetworkError> {
        let config = Self {
            mac_addr: mac_addr.into(),
            ipv4_addr: ipv4_addr.into(),
            ipv4_gateway: ipv4_gateway.into(),
            ipv6_addr: ipv6_addr.into(),
            ipv6_gateway: ipv6_gateway.into(),
            ipv6_extra_addrs: ipv6_extra_addrs.into(),
        };
        if !valid_mac(&config.mac_addr)
            || !valid_address(&config.ipv4_addr, AddressFamily::Ipv4, true)
            || !valid_address(&config.ipv4_gateway, AddressFamily::Ipv4, false)
            || !valid_address(&config.ipv6_addr, AddressFamily::Ipv6, true)
            || !valid_address(&config.ipv6_gateway, AddressFamily::Ipv6, false)
            || !config.ipv6_extra_addrs.split(',').all(|address| {
                address.is_empty() || valid_address(address, AddressFamily::Ipv6, true)
            })
        {
            return Err(InitrdNetworkError::InvalidInput);
        }
        Ok(config)
    }

    pub fn mac_addr(&self) -> &str {
        &self.mac_addr
    }

    pub fn ipv4_addr(&self) -> &str {
        &self.ipv4_addr
    }

    pub fn ipv4_gateway(&self) -> &str {
        &self.ipv4_gateway
    }

    pub fn ipv6_addr(&self) -> &str {
        &self.ipv6_addr
    }

    pub fn ipv6_gateway(&self) -> &str {
        &self.ipv6_gateway
    }

    pub fn ipv6_extra_addrs(&self) -> &str {
        &self.ipv6_extra_addrs
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkResult {
    pub interface: String,
    pub dhcpv4: bool,
    pub dhcpv6_or_slaac: bool,
    pub should_disable_dhcpv4: bool,
    pub should_disable_accept_ra: bool,
    pub should_disable_autoconf: bool,
    pub ipv4_has_internet: bool,
    pub ipv6_has_internet: bool,
}

#[derive(Debug)]
pub enum InitrdNetworkError {
    InvalidInput,
    Command(io::Error),
    CommandFailed {
        program: String,
        status: Option<i32>,
    },
    Filesystem(io::Error),
}

impl fmt::Display for InitrdNetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("Invalid initrd network configuration"),
            Self::Command(error) => write!(f, "Initrd network command failed to start: {error}"),
            Self::CommandFailed { program, status } => {
                write!(
                    f,
                    "Initrd network command {program} failed with status {status:?}"
                )
            }
            Self::Filesystem(error) => write!(f, "Initrd network configuration failed: {error}"),
        }
    }
}

impl std::error::Error for InitrdNetworkError {}

pub fn interface_for_mac(link_output: &str, mac_addr: &str) -> Option<String> {
    link_output.lines().find_map(|line| {
        if line
            .to_ascii_lowercase()
            .contains(&mac_addr.to_ascii_lowercase())
            && !line.contains("master")
        {
            line.split_whitespace()
                .nth(1)
                .and_then(|name| name.split(':').next())
                .filter(|name| valid_interface_name(name))
                .map(str::to_owned)
        } else {
            None
        }
    })
}

pub fn address_from_ip_output(output: &str, family: AddressFamily) -> Option<String> {
    let token = match family {
        AddressFamily::Ipv4 => "inet",
        AddressFamily::Ipv6 => "inet6",
    };
    output.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let index = fields.iter().position(|field| *field == token)?;
        let address = *fields.get(index + 1)?;
        valid_address(address, family, true).then(|| address.to_owned())
    })
}

pub fn gateway_from_route_output(output: &str) -> Option<String> {
    let fields: Vec<&str> = output.lines().next()?.split_whitespace().collect();
    if fields.first() != Some(&"default") || fields.get(1) != Some(&"via") {
        return None;
    }
    fields.get(2).map(|gateway| (*gateway).to_owned())
}

/// Run the Alpine initrd network setup. A missing NIC is non-fatal in the
/// legacy initrd helper; later Live-stage checks decide whether installation can continue.
pub fn run_initrd_network<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    config: &InitrdNetworkConfig,
) -> Result<Option<NetworkResult>, InitrdNetworkError> {
    run_initrd_network_with_proxy(runner, root, config, None)
}

/// The application supplies its explicitly configured HTTP transport. Probe
/// that endpoint instead of treating blocked direct Internet egress as a bad
/// interface. Credentials are discarded before constructing any command.
pub fn run_initrd_network_with_proxy<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    config: &InitrdNetworkConfig,
    proxy: Option<&str>,
) -> Result<Option<NetworkResult>, InitrdNetworkError> {
    let proxy = proxy.map(ProxyEndpoint::parse).transpose()?;
    let interface = wait_for_interface(runner, config.mac_addr())?;
    let Some(interface) = interface else {
        return Ok(None);
    };

    run_checked(
        runner,
        "ip",
        ["link", "set", "dev", "lo", "up"],
        "initrd-loopback-up",
    )?;
    run_checked(
        runner,
        "ip",
        ["link", "set", "dev", &interface, "up"],
        "initrd-interface-up",
    )?;
    sleep(runner, "1", "initrd-interface-settle")?;

    run_ignored(
        runner,
        "timeout",
        [DHCP_TIMEOUT, "udhcpc", "-i", &interface, "-f", "-q", "-n"],
        "initrd-dhcpv4",
    );
    run_ignored(
        runner,
        "timeout",
        [DHCP_TIMEOUT, "udhcpc6", "-i", &interface, "-f", "-q", "-n"],
        "initrd-dhcpv6",
    );
    sleep(runner, DNS_FILE_TIMEOUT, "initrd-dns-settle")?;

    for _ in 0..6 {
        if have_ipv6_address(runner, &interface) {
            break;
        }
        sleep(runner, "1", "initrd-wait-slaac")?;
    }

    let dhcpv4 = have_ipv4_address(runner, &interface);
    let dhcpv6_or_slaac = have_ipv6_address(runner, &interface);
    let ra_has_gateway = have_gateway(runner, AddressFamily::Ipv6, &interface);
    let mut should_disable_dhcpv4 = false;
    let mut should_disable_accept_ra = false;
    let mut should_disable_autoconf = false;

    if dhcpv4
        && !config.ipv4_addr().is_empty()
        && !config.ipv4_gateway().is_empty()
        && first_address(runner, AddressFamily::Ipv4, &interface)
            .as_deref()
            .is_none_or(|current| strip_prefix(config.ipv4_addr()) != strip_prefix(current))
    {
        should_disable_dhcpv4 = true;
        flush_ipv4(runner, root, &interface)?;
    }
    if dhcpv6_or_slaac
        && !config.ipv6_addr().is_empty()
        && !config.ipv6_gateway().is_empty()
        && first_address(runner, AddressFamily::Ipv6, &interface)
            .as_deref()
            .is_none_or(|current| strip_prefix(config.ipv6_addr()) != strip_prefix(current))
    {
        should_disable_accept_ra = true;
        should_disable_autoconf = true;
        flush_ipv6(
            runner,
            root,
            &interface,
            should_disable_accept_ra,
            should_disable_autoconf,
        )?;
    }

    add_missing_config(runner, &interface, config)?;

    let (mut ipv4_has_internet, mut ipv6_has_internet) =
        test_internet(runner, &interface, false, false, proxy.as_ref())?;
    if !ipv4_has_internet
        && dhcpv4
        && !config.ipv4_addr().is_empty()
        && !config.ipv4_gateway().is_empty()
        && !matches_ipv4_config(runner, &interface, config)
    {
        should_disable_dhcpv4 = true;
        flush_ipv4(runner, root, &interface)?;
        add_missing_ipv4_config(runner, &interface, config)?;
        (ipv4_has_internet, ipv6_has_internet) = test_internet(
            runner,
            &interface,
            ipv4_has_internet,
            ipv6_has_internet,
            proxy.as_ref(),
        )?;
    }
    if !ipv6_has_internet
        && (dhcpv6_or_slaac || ra_has_gateway)
        && !config.ipv6_addr().is_empty()
        && !config.ipv6_gateway().is_empty()
        && !matches_ipv6_config(runner, &interface, config)
    {
        should_disable_accept_ra = true;
        should_disable_autoconf = true;
        flush_ipv6(
            runner,
            root,
            &interface,
            should_disable_accept_ra,
            should_disable_autoconf,
        )?;
        add_missing_ipv6_config(runner, &interface, config)?;
        (ipv4_has_internet, ipv6_has_internet) = test_internet(
            runner,
            &interface,
            ipv4_has_internet,
            ipv6_has_internet,
            proxy.as_ref(),
        )?;
    }

    if !ipv4_has_internet {
        if dhcpv4 {
            should_disable_dhcpv4 = true;
        }
        flush_ipv4(runner, root, &interface)?;
    }
    if !ipv6_has_internet {
        if dhcpv6_or_slaac {
            should_disable_accept_ra = true;
            should_disable_autoconf = true;
        }
        flush_ipv6(
            runner,
            root,
            &interface,
            should_disable_accept_ra,
            should_disable_autoconf,
        )?;
    }

    ensure_dns(root, AddressFamily::Ipv4)?;
    ensure_dns(root, AddressFamily::Ipv6)?;

    let result = NetworkResult {
        interface: interface.clone(),
        dhcpv4,
        dhcpv6_or_slaac,
        should_disable_dhcpv4,
        should_disable_accept_ra,
        should_disable_autoconf,
        ipv4_has_internet,
        ipv6_has_internet,
    };
    write_netconf(root, config, &result)?;
    Ok(Some(result))
}

fn wait_for_interface<R: CommandRunner>(
    runner: &mut R,
    mac_addr: &str,
) -> Result<Option<String>, InitrdNetworkError> {
    for _ in 0..20 {
        let command = command(
            "ip",
            ["-o", "link"],
            "initrd-find-interface",
            OutputMode::Capture,
        );
        let output = runner.run(&command).map_err(InitrdNetworkError::Command)?;
        if output.status == Some(0)
            && let Ok(links) = std::str::from_utf8(&output.stdout)
            && let Some(interface) = interface_for_mac(links, mac_addr)
        {
            return Ok(Some(interface));
        }
        sleep(runner, "1", "initrd-wait-interface")?;
    }
    Ok(None)
}

fn add_missing_config<R: CommandRunner>(
    runner: &mut R,
    interface: &str,
    config: &InitrdNetworkConfig,
) -> Result<(), InitrdNetworkError> {
    add_missing_ipv4_config(runner, interface, config)?;
    add_missing_ipv6_config(runner, interface, config)
}

fn add_missing_ipv4_config<R: CommandRunner>(
    runner: &mut R,
    interface: &str,
    config: &InitrdNetworkConfig,
) -> Result<(), InitrdNetworkError> {
    if config.ipv4_addr().is_empty() || config.ipv4_gateway().is_empty() {
        return Ok(());
    }
    if !have_ipv4_address(runner, interface) {
        run_checked(
            runner,
            "ip",
            ["-4", "addr", "add", config.ipv4_addr(), "dev", interface],
            "initrd-add-static-ipv4",
        )?;
    }
    if !have_gateway(runner, AddressFamily::Ipv4, interface) {
        run_checked(
            runner,
            "ip",
            [
                "-4",
                "route",
                "add",
                config.ipv4_gateway(),
                "dev",
                interface,
            ],
            "initrd-add-ipv4-onlink-gateway",
        )?;
        run_checked(
            runner,
            "ip",
            [
                "-4",
                "route",
                "add",
                "default",
                "via",
                config.ipv4_gateway(),
                "dev",
                interface,
            ],
            "initrd-add-ipv4-default-route",
        )?;
    }
    Ok(())
}

fn add_missing_ipv6_config<R: CommandRunner>(
    runner: &mut R,
    interface: &str,
    config: &InitrdNetworkConfig,
) -> Result<(), InitrdNetworkError> {
    if config.ipv6_addr().is_empty() || config.ipv6_gateway().is_empty() {
        return Ok(());
    }
    if !have_ipv6_address(runner, interface) {
        run_checked(
            runner,
            "ip",
            ["-6", "addr", "add", config.ipv6_addr(), "dev", interface],
            "initrd-add-static-ipv6",
        )?;
    }
    if !have_gateway(runner, AddressFamily::Ipv6, interface) {
        run_checked(
            runner,
            "ip",
            [
                "-6",
                "route",
                "add",
                config.ipv6_gateway(),
                "dev",
                interface,
            ],
            "initrd-add-ipv6-onlink-gateway",
        )?;
        run_checked(
            runner,
            "ip",
            [
                "-6",
                "route",
                "add",
                "default",
                "via",
                config.ipv6_gateway(),
                "dev",
                interface,
            ],
            "initrd-add-ipv6-default-route",
        )?;
    }
    Ok(())
}

fn matches_ipv4_config<R: CommandRunner>(
    runner: &mut R,
    interface: &str,
    config: &InitrdNetworkConfig,
) -> bool {
    first_address(runner, AddressFamily::Ipv4, interface).as_deref() == Some(config.ipv4_addr())
        && first_gateway(runner, AddressFamily::Ipv4, interface).as_deref()
            == Some(config.ipv4_gateway())
}

fn matches_ipv6_config<R: CommandRunner>(
    runner: &mut R,
    interface: &str,
    config: &InitrdNetworkConfig,
) -> bool {
    first_address(runner, AddressFamily::Ipv6, interface).as_deref() == Some(config.ipv6_addr())
        && first_gateway(runner, AddressFamily::Ipv6, interface).as_deref()
            == Some(config.ipv6_gateway())
}

fn test_internet<R: CommandRunner>(
    runner: &mut R,
    interface: &str,
    mut ipv4_has_internet: bool,
    mut ipv6_has_internet: bool,
    proxy: Option<&ProxyEndpoint>,
) -> Result<(bool, bool), InitrdNetworkError> {
    for _ in 0..5 {
        if need_internet_test(runner, AddressFamily::Ipv4, interface, ipv4_has_internet)
            && let Some(address) = first_address(runner, AddressFamily::Ipv4, interface)
            && test_transport(runner, strip_prefix(&address), &IPV4_DNS, proxy)?
        {
            ipv4_has_internet = true;
        }
        if need_internet_test(runner, AddressFamily::Ipv6, interface, ipv6_has_internet)
            && let Some(address) = first_address(runner, AddressFamily::Ipv6, interface)
            && test_transport(runner, strip_prefix(&address), &IPV6_DNS, proxy)?
        {
            ipv6_has_internet = true;
        }
        if !need_internet_test(runner, AddressFamily::Ipv4, interface, ipv4_has_internet)
            && !need_internet_test(runner, AddressFamily::Ipv6, interface, ipv6_has_internet)
        {
            break;
        }
    }
    Ok((ipv4_has_internet, ipv6_has_internet))
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

fn test_transport<R: CommandRunner>(
    runner: &mut R,
    source: &str,
    destinations: &[&str; 2],
    proxy: Option<&ProxyEndpoint>,
) -> Result<bool, InitrdNetworkError> {
    let Some(proxy) = proxy else {
        return test_connect(runner, source, destinations);
    };
    let port = proxy.port.to_string();
    let command = command(
        "nc",
        [
            "-z",
            "-v",
            "-w",
            TEST_TIMEOUT,
            "-s",
            source,
            &proxy.host,
            &port,
        ],
        "initrd-test-proxy-transport",
        OutputMode::Discard,
    );
    Ok(runner
        .run(&command)
        .is_ok_and(|result| result.status == Some(0)))
}

fn need_internet_test<R: CommandRunner>(
    runner: &mut R,
    family: AddressFamily,
    interface: &str,
    has_internet: bool,
) -> bool {
    if has_internet {
        return false;
    }
    let has_address = match family {
        AddressFamily::Ipv4 => have_ipv4_address(runner, interface),
        AddressFamily::Ipv6 => have_ipv6_address(runner, interface),
    };
    has_address && have_gateway(runner, family, interface)
}

fn test_connect<R: CommandRunner>(
    runner: &mut R,
    source: &str,
    destinations: &[&str; 2],
) -> Result<bool, InitrdNetworkError> {
    for destination in destinations {
        let command = command(
            "nc",
            [
                "-z",
                "-v",
                "-w",
                TEST_TIMEOUT,
                "-s",
                source,
                destination,
                "443",
            ],
            "initrd-test-connectivity",
            OutputMode::Discard,
        );
        if runner
            .run(&command)
            .is_ok_and(|output| output.status == Some(0))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn have_ipv4_address<R: CommandRunner>(runner: &mut R, interface: &str) -> bool {
    ip_probe(
        runner,
        ["-4", "addr", "show", "scope", "global", "dev", interface],
    )
    .contains("inet ")
}

fn have_ipv6_address<R: CommandRunner>(runner: &mut R, interface: &str) -> bool {
    ip_probe(
        runner,
        ["-6", "addr", "show", "scope", "global", "dev", interface],
    )
    .contains("inet6 ")
}

fn have_gateway<R: CommandRunner>(runner: &mut R, family: AddressFamily, interface: &str) -> bool {
    !route_output(runner, family, interface).trim().is_empty()
}

fn first_address<R: CommandRunner>(
    runner: &mut R,
    family: AddressFamily,
    interface: &str,
) -> Option<String> {
    let family_arg = match family {
        AddressFamily::Ipv4 => "-4",
        AddressFamily::Ipv6 => "-6",
    };
    let output = ip_probe(
        runner,
        [
            family_arg, "-o", "addr", "show", "scope", "global", "dev", interface,
        ],
    );
    address_from_ip_output(&output, family)
}

fn first_gateway<R: CommandRunner>(
    runner: &mut R,
    family: AddressFamily,
    interface: &str,
) -> Option<String> {
    gateway_from_route_output(&route_output(runner, family, interface))
}

fn route_output<R: CommandRunner>(
    runner: &mut R,
    family: AddressFamily,
    interface: &str,
) -> String {
    let family_arg = match family {
        AddressFamily::Ipv4 => "-4",
        AddressFamily::Ipv6 => "-6",
    };
    ip_probe(
        runner,
        [family_arg, "route", "show", "default", "dev", interface],
    )
}

fn ip_probe<R: CommandRunner, I, S>(runner: &mut R, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let command = command("ip", args, "initrd-network-probe", OutputMode::Capture);
    runner
        .run(&command)
        .ok()
        .filter(|output| output.status == Some(0))
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_default()
}

fn run_checked<R: CommandRunner, I, S>(
    runner: &mut R,
    program: &str,
    args: I,
    phase: &str,
) -> Result<(), InitrdNetworkError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let command = command(program, args, phase, OutputMode::Stream);
    let output = runner.run(&command).map_err(InitrdNetworkError::Command)?;
    if output.status == Some(0) {
        Ok(())
    } else {
        Err(InitrdNetworkError::CommandFailed {
            program: program.to_owned(),
            status: output.status,
        })
    }
}

fn run_ignored<R: CommandRunner, I, S>(runner: &mut R, program: &str, args: I, phase: &str)
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let command = command(program, args, phase, OutputMode::Stream);
    let _ = runner.run(&command);
}

fn sleep<R: CommandRunner>(
    runner: &mut R,
    seconds: &str,
    phase: &str,
) -> Result<(), InitrdNetworkError> {
    run_checked(runner, "sleep", [seconds], phase)
}

fn command<I, S>(program: &str, args: I, phase: &str, output: OutputMode) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    CommandSpec::new(program, args)
        .phase(phase)
        .stdin(CommandInput::Null)
        .stdout_mode(output)
        .stderr_mode(if output == OutputMode::Capture {
            OutputMode::Discard
        } else {
            output
        })
}

fn flush_ipv4<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    interface: &str,
) -> Result<(), InitrdNetworkError> {
    run_checked(
        runner,
        "ip",
        ["-4", "addr", "flush", "scope", "global", "dev", interface],
        "initrd-flush-ipv4-addresses",
    )?;
    run_checked(
        runner,
        "ip",
        ["-4", "route", "flush", "dev", interface],
        "initrd-flush-ipv4-routes",
    )?;
    filter_resolv_conf(root, '.')
}

fn flush_ipv6<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    interface: &str,
    disable_accept_ra: bool,
    disable_autoconf: bool,
) -> Result<(), InitrdNetworkError> {
    if disable_accept_ra {
        write_proc_value(
            &root.join(format!("proc/sys/net/ipv6/conf/{interface}/accept_ra")),
            "0\n",
        )?;
    }
    if disable_autoconf {
        write_proc_value(
            &root.join(format!("proc/sys/net/ipv6/conf/{interface}/autoconf")),
            "0\n",
        )?;
    }
    run_checked(
        runner,
        "ip",
        ["-6", "addr", "flush", "scope", "global", "dev", interface],
        "initrd-flush-ipv6-addresses",
    )?;
    run_checked(
        runner,
        "ip",
        ["-6", "route", "flush", "dev", interface],
        "initrd-flush-ipv6-routes",
    )?;
    filter_resolv_conf(root, ':')
}

fn filter_resolv_conf(root: &Path, character: char) -> Result<(), InitrdNetworkError> {
    let path = root.join("etc/resolv.conf");
    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(InitrdNetworkError::Filesystem(error)),
    };
    let filtered = contents
        .split_inclusive(|byte| *byte == b'\n')
        .filter(|line| !line.contains(&(character as u8)))
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    atomic_replace(&path, &filtered).map_err(InitrdNetworkError::Filesystem)
}

fn ensure_dns(root: &Path, family: AddressFamily) -> Result<(), InitrdNetworkError> {
    let path = root.join("etc/resolv.conf");
    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(InitrdNetworkError::Filesystem(error)),
    };
    let expected = match family {
        AddressFamily::Ipv4 => &IPV4_DNS,
        AddressFamily::Ipv6 => &IPV6_DNS,
    };
    let has_dns = contents.split(|byte| *byte == b'\n').any(|line| {
        let line = String::from_utf8_lossy(line);
        line.strip_prefix("nameserver ")
            .is_some_and(|address| match family {
                AddressFamily::Ipv4 => address.contains('.'),
                AddressFamily::Ipv6 => address.contains(':'),
            })
    });
    if !has_dns {
        let mut updated = contents;
        for server in expected {
            updated.extend_from_slice(format!("nameserver {server}\n").as_bytes());
        }
        atomic_replace(&path, &updated).map_err(InitrdNetworkError::Filesystem)?;
    }
    Ok(())
}

fn write_netconf(
    root: &Path,
    config: &InitrdNetworkConfig,
    result: &NetworkResult,
) -> Result<(), InitrdNetworkError> {
    let directory = root.join("dev/netconf").join(&result.interface);
    fs::create_dir_all(&directory).map_err(InitrdNetworkError::Filesystem)?;
    let fields = [
        ("dhcpv4", bool_digit(result.dhcpv4)),
        ("dhcpv6_or_slaac", bool_digit(result.dhcpv6_or_slaac)),
        (
            "should_disable_dhcpv4",
            bool_digit(result.should_disable_dhcpv4),
        ),
        (
            "should_disable_accept_ra",
            bool_digit(result.should_disable_accept_ra),
        ),
        (
            "should_disable_autoconf",
            bool_digit(result.should_disable_autoconf),
        ),
        ("ethx", result.interface.as_str()),
        ("mac_addr", config.mac_addr()),
        ("ipv4_addr", config.ipv4_addr()),
        ("ipv4_gateway", config.ipv4_gateway()),
        ("ipv6_addr", config.ipv6_addr()),
        ("ipv6_gateway", config.ipv6_gateway()),
        ("ipv6_extra_addrs", config.ipv6_extra_addrs()),
        ("ipv4_has_internet", bool_digit(result.ipv4_has_internet)),
        ("ipv6_has_internet", bool_digit(result.ipv6_has_internet)),
    ];
    for (name, value) in fields {
        atomic_replace(&directory.join(name), format!("{value}\n").as_bytes())
            .map_err(InitrdNetworkError::Filesystem)?;
    }
    Ok(())
}

fn bool_digit(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

fn write_proc_value(path: &Path, value: &str) -> Result<(), InitrdNetworkError> {
    fs::write(path, value).map_err(InitrdNetworkError::Filesystem)
}

fn atomic_replace(path: &Path, contents: &[u8]) -> io::Result<()> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".tmp.{}.{nonce}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let mode = fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o777)
        .unwrap_or(0o666);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(contents)?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn valid_mac(value: &str) -> bool {
    let mut octets = value.split(':');
    (0..6).all(|_| {
        octets.next().is_some_and(|octet| {
            octet.len() == 2 && octet.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && octets.next().is_none()
}

fn valid_address(value: &str, family: AddressFamily, allow_prefix: bool) -> bool {
    if value.is_empty() {
        return true;
    }
    let (address, prefix) = match value.split_once('/') {
        Some((address, prefix)) if allow_prefix => {
            let Ok(prefix) = prefix.parse::<u8>() else {
                return false;
            };
            let maximum = match family {
                AddressFamily::Ipv4 => 32,
                AddressFamily::Ipv6 => 128,
            };
            if prefix > maximum {
                return false;
            }
            (address, true)
        }
        Some(_) => return false,
        None => (value, false),
    };
    let Ok(parsed) = address.parse::<IpAddr>() else {
        return false;
    };
    let family_matches = matches!(
        (family, parsed),
        (AddressFamily::Ipv4, IpAddr::V4(_)) | (AddressFamily::Ipv6, IpAddr::V6(_))
    );
    family_matches && (!allow_prefix || !prefix || value.contains('/'))
}

fn valid_interface_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 15
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:@-".contains(&byte))
}

fn strip_prefix(value: &str) -> &str {
    value.split('/').next().unwrap_or(value)
}
