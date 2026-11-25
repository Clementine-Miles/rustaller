//! Read-only source network observation. Live and target networking are separate.
use super::handoff::{HandoffError, MAX_TEXT, capture, spec};
use crate::command::{CommandRunner, OutputMode};
use crate::deployment::{NetworkInterface, parse_interface_address, valid_interface_name};
use crate::fsutil::effective_uid;
use crate::worker::initrd_network::{AddressFamily, gateway_from_route_output};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NetworkOwner {
    Networkd,
    NetworkManager,
    Ifupdown,
    Dhcpcd,
}

#[derive(Debug)]
struct SourcePolicy {
    owner: NetworkOwner,
    dhcp4: bool,
    dhcp6: bool,
    accept_ra: bool,
    // None means the adapter verified policy but has no exact static
    // configuration to compare. Some(empty) explicitly disables addresses.
    static_addresses: [Option<Vec<String>>; 2],
    static_gateways: [Option<Option<String>>; 2],
}

impl SourcePolicy {
    fn new(owner: NetworkOwner, [dhcp4, dhcp6, accept_ra]: [bool; 3]) -> Self {
        Self {
            owner,
            dhcp4,
            dhcp6,
            accept_ra,
            static_addresses: [None, None],
            static_gateways: [None, None],
        }
    }

    fn check_observations(
        &self,
        observed: [&[String]; 2],
        raw: [&str; 2],
        gateways: [&Option<String>; 2],
    ) -> Result<(), HandoffError> {
        for (i, automatic) in [self.dhcp4, self.dhcp6 || self.accept_ra]
            .into_iter()
            .enumerate()
        {
            let dynamic = |line: &str| line.split_whitespace().any(|part| part == "dynamic");
            let lines = raw[i]
                .lines()
                .filter(|line| !line.split_whitespace().any(|part| part == "temporary"));
            // Linux DHCPv4 clients can use addresses with infinite kernel
            // lifetimes. Legacy IPv4 ownership comes from applied state or a
            // matching active lease, not from the kernel's dynamic marker.
            if (self.owner == NetworkOwner::Networkd || !automatic || i == 1)
                && lines.clone().any(|line| automatic != dynamic(line))
                || self.static_addresses[i].as_ref().is_some_and(|configured| {
                    configured.len() != observed[i].len()
                        || !configured
                            .iter()
                            .all(|address| observed[i].contains(address))
                })
                || self.static_gateways[i]
                    .as_ref()
                    .is_some_and(|gateway| gateway != gateways[i])
            {
                return Err(network_error(
                    "observed addresses or gateways disagree with the applied source profile; use explicit networking",
                ));
            }
        }
        Ok(())
    }
}

struct SourceLink<'a> {
    index: &'a str,
    name: &'a str,
    mac: &'a str,
    supported: bool,
}

fn source_links(output: &str) -> impl Iterator<Item = SourceLink<'_>> {
    output.lines().filter_map(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        let qualified_name = fields.get(1)?.trim_end_matches(':');
        let name = qualified_name.split('@').next()?;
        let hw = fields.iter().position(|word| *word == "link/ether")?;
        let supported = !qualified_name.contains('@')
            && !fields.iter().any(|word| {
                matches!(
                    *word,
                    "master"
                        | "vrf"
                        | "bridge"
                        | "bond"
                        | "vlan"
                        | "vxlan"
                        | "veth"
                        | "macvlan"
                        | "ipvlan"
                        | "tun"
                        | "wireguard"
                )
            });
        Some(SourceLink {
            index: fields[0].trim_end_matches(':'),
            name,
            mac: fields.get(hw + 1)?,
            supported,
        })
    })
}

fn network_error(reason: &'static str) -> HandoffError {
    HandoffError::Invalid(reason)
}

pub fn collect_network<R: CommandRunner>(
    runner: &mut R,
) -> Result<Vec<NetworkInterface>, HandoffError> {
    collect_network_at(runner, Path::new("/"))
}

fn collect_network_at<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
) -> Result<Vec<NetworkInterface>, HandoffError> {
    let networkd = service_active(runner, "systemd-networkd.service")?;
    let nm = active_nm_connections(runner, root)?;
    let links = capture(
        runner,
        &spec("ip", ["-o", "-d", "link", "show"], "handoff-network-links"),
    )?;
    let mut result = Vec::new();
    let mut legacy_dhcp4 = BTreeSet::new();
    for link in source_links(&links) {
        let name = link.name;
        if !valid_interface_name(name) {
            return Err(HandoffError::Invalid("unsupported interface name"));
        }
        let mac = link.mac.to_owned();
        let v4 = capture(
            runner,
            &spec(
                "ip",
                ["-o", "-4", "addr", "show", "dev", name, "scope", "global"],
                "handoff-ipv4-address",
            ),
        )?;
        let v6 = capture(
            runner,
            &spec(
                "ip",
                ["-o", "-6", "addr", "show", "dev", name, "scope", "global"],
                "handoff-ipv6-address",
            ),
        )?;
        let ipv4_addresses = parse_addresses(&v4, AddressFamily::Ipv4)?;
        let ipv6_addresses = parse_addresses(&v6, AddressFamily::Ipv6)?;
        if ipv4_addresses.is_empty() && ipv6_addresses.is_empty() {
            continue;
        }
        if !link.supported {
            return Err(HandoffError::Invalid(
                "addressed bridges, bonds, VRFs, VLANs and virtual links cannot be inherited",
            ));
        }
        if wireless_link(root, name) || nm.get(name).is_some_and(|profile| profile.wireless) {
            return Err(wireless_requirement(
                runner,
                root,
                name,
                nm.contains_key(name),
            ));
        }
        let index = link
            .index
            .parse::<u32>()
            .map_err(|_| network_error("invalid interface index"))?;
        let policy = applied_policy(
            runner,
            root,
            name,
            index,
            networkd,
            nm.get(name),
            &ipv4_addresses,
        )?;
        let route4 = capture(
            runner,
            &spec(
                "ip",
                ["-4", "route", "show", "default", "dev", name],
                "handoff-ipv4-gateway",
            ),
        )?;
        let route6 = capture(
            runner,
            &spec(
                "ip",
                ["-6", "route", "show", "default", "dev", name],
                "handoff-ipv6-gateway",
            ),
        )?;
        let ipv4_gateway = parse_default_gateway(&route4, AddressFamily::Ipv4)?;
        let ipv6_gateway = parse_default_gateway(&route6, AddressFamily::Ipv6)?;
        policy.check_observations(
            [&ipv4_addresses, &ipv6_addresses],
            [&v4, &v6],
            [&ipv4_gateway, &ipv6_gateway],
        )?;
        if matches!(policy.owner, NetworkOwner::Ifupdown | NetworkOwner::Dhcpcd) && policy.dhcp4 {
            legacy_dhcp4.insert(name);
        }
        result.push(NetworkInterface {
            name: name.into(),
            mac,
            ipv4_addresses,
            ipv6_addresses,
            ipv4_gateway,
            ipv6_gateway,
            dhcp4: policy.dhcp4,
            dhcp6: policy.dhcp6,
            accept_ra: policy.accept_ra,
        });
    }
    if result.is_empty() || result.len() > 16 {
        return Err(HandoffError::Invalid(
            "no supported inherited network interface",
        ));
    }
    for family in [AddressFamily::Ipv4, AddressFamily::Ipv6] {
        let version = if family == AddressFamily::Ipv4 {
            "-4"
        } else {
            "-6"
        };
        let rules = capture(
            runner,
            &spec("ip", [version, "rule", "show"], "handoff-routing-rules"),
        )?;
        let routes = capture(
            runner,
            &spec(
                "ip",
                [version, "route", "show", "table", "all"],
                "handoff-routing-tables",
            ),
        )?;
        validate_inherited_routes(&rules, &routes, family, &result, &legacy_dhcp4)?;
    }
    Ok(result)
}

/// Select a real, currently visible interface. The explicit policy owns
/// addresses/routes and therefore does not import the host's route tables.
pub fn collect_explicit_network<R: CommandRunner>(
    runner: &mut R,
    explicit: &crate::config::ExplicitNetwork,
) -> Result<Vec<NetworkInterface>, HandoffError> {
    explicit.validate().map_err(network_error)?;
    let links = capture(
        runner,
        &spec("ip", ["-o", "-d", "link", "show"], "handoff-network-links"),
    )?;
    let mut selected =
        source_links(&links).filter(|link| link.mac.eq_ignore_ascii_case(&explicit.mac));
    let link = selected
        .next()
        .ok_or_else(|| network_error("explicit network MAC is not present"))?;
    if selected.next().is_some() || !link.supported || !valid_interface_name(link.name) {
        return Err(network_error(
            "explicit network MAC is ambiguous or belongs to an unsupported link",
        ));
    }
    if wireless_link(Path::new("/"), link.name) {
        return Err(wireless_requirement(
            runner,
            Path::new("/"),
            link.name,
            false,
        ));
    }
    Ok(vec![NetworkInterface {
        name: link.name.into(),
        mac: link.mac.to_ascii_lowercase(),
        ipv4_addresses: explicit.ipv4_address.iter().cloned().collect(),
        ipv4_gateway: explicit.ipv4_gateway.clone(),
        ipv6_addresses: explicit.ipv6_address.iter().cloned().collect(),
        ipv6_gateway: explicit.ipv6_gateway.clone(),
        dhcp4: explicit.dhcp4,
        dhcp6: explicit.dhcp6,
        accept_ra: explicit.accept_ra,
    }])
}

fn local_output<R: CommandRunner, const N: usize>(
    runner: &mut R,
    program: &str,
    args: [&str; N],
) -> Result<Option<crate::command::CommandOutput>, HandoffError> {
    let command = spec(program, args, "handoff-network-owner")
        .env("LC_ALL", "C")
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Capture)
        .inspection_limits();
    match runner.run(&command) {
        Ok(output) if output.stdout.len() <= MAX_TEXT => Ok(Some(output)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(_) => Err(network_error("oversized network manager response")),
    }
}

fn service_active<R: CommandRunner>(runner: &mut R, unit: &str) -> Result<bool, HandoffError> {
    match local_output(runner, "systemctl", ["is-active", unit])? {
        Some(result) if result.status == Some(0) && result.stdout == b"active\n" => Ok(true),
        Some(result) if matches!(result.status, Some(1 | 3 | 4)) => Ok(false),
        None => Ok(false),
        _ => Err(network_error(
            "cannot determine running source network services",
        )),
    }
}

fn applied_policy<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    name: &str,
    index: u32,
    networkd: bool,
    nm: Option<&NmConnection>,
    ipv4: &[String],
) -> Result<SourcePolicy, HandoffError> {
    let conflict = || {
        network_error(
            "more than one source manager can configure this interface; use explicit networking",
        )
    };
    let networkd = networkd && networkd_owns_link(root, index)?;
    let ifupdown = ifupdown_policy(root, name)?;
    let dhcpcd = active_dhcpcd(root, name)?;
    if usize::from(networkd) + usize::from(nm.is_some()) + usize::from(ifupdown.is_some()) > 1 {
        return Err(conflict());
    }
    if let Some(lease_client) = dhcpcd {
        if networkd
            || nm.is_some()
            || ifupdown
                .as_ref()
                .is_some_and(|policy| lease_client.global || !policy.dhcp4)
        {
            return Err(conflict());
        }
        let policy = dhcpcd_policy(runner, root, name, ipv4, lease_client)?;
        if let Some(ifupdown) = ifupdown {
            if [policy.dhcp4, policy.dhcp6, policy.accept_ra]
                != [ifupdown.dhcp4, ifupdown.dhcp6, ifupdown.accept_ra]
            {
                return Err(conflict());
            }
            return Ok(ifupdown);
        }
        return Ok(policy);
    }
    if let Some(nm) = nm {
        return nm_policy(runner, name, &nm.uuid);
    }
    if networkd {
        return source_network_policy(root, index)
            .map(|flags| SourcePolicy::new(NetworkOwner::Networkd, flags));
    }
    if let Some(ifupdown) = ifupdown {
        return Ok(ifupdown);
    }
    Err(network_error(
        "no supported applied profile owns this Ethernet interface; use explicit networking",
    ))
}

fn networkd_owns_link(root: &Path, index: u32) -> Result<bool, HandoffError> {
    let netif = root.join("run/systemd/netif");
    let path = netif.join("links").join(index.to_string());
    let owner = match fs::symlink_metadata(&netif) {
        Ok(metadata) if metadata.is_dir() => metadata.uid(),
        Ok(_) => return Err(network_error("unsafe networkd state directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    crate::shared::configure::validate_planned_parent(&path)
        .map_err(|_| network_error("unsafe networkd state directory"))?;
    let text = match read_network_file(&path, owner) {
        Ok(text) => text,
        Err(HandoffError::Filesystem(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    match text
        .lines()
        .find_map(|line| line.strip_prefix("ADMIN_STATE="))
    {
        Some("configured") => Ok(true),
        Some("unmanaged") => Ok(false),
        _ => Err(network_error(
            "networkd is still configuring an addressed interface",
        )),
    }
}

fn ifupdown_policy(root: &Path, name: &str) -> Result<Option<SourcePolicy>, HandoffError> {
    let invalid =
        || network_error("unsupported or stale ifupdown configuration; use explicit networking");
    let mut state_path = None;
    for relative in ["run/network/ifstate", "run/ifstate"] {
        let path = root.join(relative);
        if let Some(state) = optional_network_file(&path)? {
            for line in state.lines() {
                let mut words = line.split_whitespace();
                let Some(first) = words.next() else { continue };
                let (physical, logical) = first.split_once('=').unwrap_or((first, first));
                if physical == name
                    && (logical != name || state_path.replace(path.clone()).is_some())
                {
                    return Err(invalid());
                }
            }
        }
    }
    let Some(state_path) = state_path else {
        return Ok(None);
    };
    let path = root.join("etc/network/interfaces");
    let mut text = optional_network_file(&path)?.ok_or_else(invalid)?;
    let mut newest = fs::metadata(&path)?.modified()?;
    // Handle the distro's standard flat include directory. Other globs and
    // recursive includes cannot be interpreted without running a manager.
    let includes = text.lines().any(|line| {
        matches!(
            line.split_whitespace().next(),
            Some("source" | "source-directory")
        )
    });
    if includes {
        let mut remaining = String::new();
        let mut sources = None;
        for line in text.lines() {
            let words: Vec<_> = line.split_whitespace().collect();
            match words.as_slice() {
                ["source", "/etc/network/interfaces.d/*"]
                | ["source-directory", "/etc/network/interfaces.d"] => {
                    if sources.replace(words[0]).is_some() {
                        return Err(invalid());
                    }
                }
                [directive, ..] if matches!(*directive, "source" | "source-directory") => {
                    return Err(invalid());
                }
                _ => {
                    remaining.push_str(line);
                    remaining.push('\n');
                }
            }
        }
        let directory = root.join("etc/network/interfaces.d");
        if directory.exists() {
            crate::shared::configure::validate_directory_tree(&directory).map_err(|_| invalid())?;
            let entries = fs::read_dir(&directory)?
                .take(65)
                .collect::<Result<Vec<_>, _>>()?;
            if entries.len() > 64 {
                return Err(invalid());
            }
            for entry in entries {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    return Err(invalid());
                };
                if name.starts_with('.')
                    || (sources == Some("source-directory")
                        && !name
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)))
                {
                    continue;
                }
                let path = entry.path();
                let contents = optional_network_file(&path)?.ok_or_else(invalid)?;
                newest = newest.max(fs::metadata(path)?.modified()?);
                remaining.push('\n');
                remaining.push_str(&contents);
                if remaining.len() > MAX_TEXT {
                    return Err(invalid());
                }
            }
        }
        text = remaining;
    }
    if newest > fs::metadata(state_path)?.modified()? {
        return Err(invalid());
    }
    parse_ifupdown_profile(&text, name).map(Some)
}

#[derive(Default)]
struct IfupdownFamily {
    method: Option<&'static str>,
    addresses: Vec<String>,
    gateway: Option<String>,
    netmask: Option<u8>,
}

fn parse_ifupdown_profile(text: &str, device: &str) -> Result<SourcePolicy, HandoffError> {
    let invalid = || {
        network_error(
            "unsupported ifupdown address policy or custom interface hooks; use explicit networking",
        )
    };
    let mut families = [IfupdownFamily::default(), IfupdownFamily::default()];
    let mut section = None;
    let mut native_ra = None;
    let mut native_dhcp = None;
    let mut seen_native = false;
    for line in text.lines() {
        let words: Vec<_> = line
            .split_whitespace()
            .take_while(|word| !word.starts_with('#'))
            .collect();
        match words.as_slice() {
            [] => continue,
            [directive, ..] if matches!(*directive, "source" | "source-directory" | "mapping") => {
                return Err(invalid());
            }
            [directive, ..] if *directive == "auto" || directive.starts_with("allow-") => {
                section = None;
                continue;
            }
            ["iface", name, tail @ ..] => {
                section = None;
                if *name != device {
                    continue;
                }
                let (family, method) = match tail {
                    [] if !seen_native && families.iter().all(|f| f.method.is_none()) => {
                        seen_native = true;
                        section = Some(2);
                        continue;
                    }
                    [family, method] if !seen_native => (*family, *method),
                    _ => return Err(invalid()),
                };
                let index = match family {
                    "inet" => 0,
                    "inet6" => 1,
                    _ => return Err(invalid()),
                };
                let method = match method {
                    "dhcp" => "dhcp",
                    "static" => "static",
                    "manual" => "manual",
                    "auto" if index == 1 => "ra",
                    _ => return Err(invalid()),
                };
                if families[index].method.replace(method).is_some() {
                    return Err(invalid());
                }
                section = Some(index);
                continue;
            }
            _ => {}
        }
        let Some(index) = section else { continue };
        match words.as_slice() {
            ["address", address] => {
                let index = if index == 2 {
                    usize::from(address.contains(':'))
                } else {
                    index
                };
                families[index].addresses.push((*address).into());
            }
            ["gateway", gateway] => {
                let index = if index == 2 {
                    usize::from(gateway.contains(':'))
                } else {
                    index
                };
                if families[index].gateway.replace((*gateway).into()).is_some() {
                    return Err(invalid());
                }
            }
            ["netmask", mask] if index == 0 => {
                let mask = u32::from(mask.parse::<Ipv4Addr>().map_err(|_| invalid())?);
                let bits = mask.leading_ones();
                if mask != u32::MAX.checked_shl(32 - bits).unwrap_or(0)
                    || families[0].netmask.replace(bits as u8).is_some()
                {
                    return Err(invalid());
                }
            }
            ["use", "static" | "ipv6"] if index == 2 => {}
            ["use", "dhcp"] if index == 2 && native_dhcp.is_none() => {
                native_dhcp = Some([true, false]);
            }
            ["dhcp-opts", "-4"] if index == 2 && native_dhcp.is_some() => {}
            ["dhcp-opts", "-6", "--noipv6rs", "--ia_na"] if index == 2 && native_dhcp.is_some() => {
                native_dhcp = Some([false, true]);
            }
            ["ipv6-accept-ra", value] if index == 2 && native_ra.is_none() => {
                native_ra = Some(match *value {
                    "0" => false,
                    "1" => true,
                    _ => return Err(invalid()),
                });
            }
            ["mtu" | "dns-nameservers" | "dns-search", ..] => {}
            _ => return Err(invalid()),
        }
    }
    let [dhcp4, dhcp6] = native_dhcp.unwrap_or([
        families[0].method == Some("dhcp"),
        families[1].method == Some("dhcp"),
    ]);
    let accept_ra = native_ra.unwrap_or(families[1].method == Some("ra"));
    if !seen_native && families.iter().all(|f| f.method.is_none()) {
        return Err(invalid());
    }
    let mut policy = SourcePolicy::new(NetworkOwner::Ifupdown, [dhcp4, dhcp6, accept_ra]);
    for (i, family) in families.into_iter().enumerate() {
        let automatic = [dhcp4, dhcp6 || accept_ra][i];
        if automatic {
            if !family.addresses.is_empty() || family.gateway.is_some() || family.netmask.is_some()
            {
                return Err(invalid());
            }
            continue;
        }
        let addresses = family
            .addresses
            .iter()
            .map(|address| {
                let cidr = if !address.contains('/') {
                    format!("{address}/{}", family.netmask.ok_or_else(invalid)?)
                } else {
                    address.clone()
                };
                let (ip, prefix) = parse_interface_address(&cidr, i == 1).map_err(|_| invalid())?;
                Ok(format!("{ip}/{prefix}"))
            })
            .collect::<Result<Vec<_>, HandoffError>>()?;
        if (family.method == Some("static") && addresses.is_empty())
            || (family.gateway.is_some() && addresses.is_empty())
            || (matches!(family.method, None | Some("manual"))
                && !seen_native
                && !addresses.is_empty())
        {
            return Err(invalid());
        }
        policy.static_addresses[i] = Some(addresses);
        policy.static_gateways[i] = Some(configured_gateway(
            family.gateway.as_deref().unwrap_or(""),
            i == 1,
        )?);
    }
    Ok(policy)
}

#[derive(Clone, Copy)]
struct DhcpcdOwner {
    global: bool,
    ipv4_only: bool,
}

fn active_dhcpcd(root: &Path, name: &str) -> Result<Option<DhcpcdOwner>, HandoffError> {
    let invalid =
        || network_error("unsupported or competing dhcpcd runtime state; use explicit networking");
    let mut owner = None;
    for dir in ["run", "run/dhcpcd"] {
        for (file, global, ipv4_only) in [
            ("dhcpcd.pid".to_owned(), true, false),
            (format!("dhcpcd.{name}.pid"), false, false),
            (format!("dhcpcd.{name}-4.pid"), false, true),
            (format!("dhcpcd.{name}-6.pid"), false, false),
        ] {
            let Some(raw_pid) = optional_network_file(&root.join(dir).join(&file))? else {
                continue;
            };
            let pid = raw_pid
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|pid| *pid > 1)
                .ok_or_else(invalid)?;
            let proc = root.join("proc").join(pid.to_string());
            let exe = match fs::read_link(proc.join("exe")) {
                Ok(exe) => exe,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Err(invalid());
                }
                Err(error) => return Err(error.into()),
            };
            if exe.file_name() != Some(OsStr::new("dhcpcd")) {
                continue;
            }
            if file.ends_with("-6.pid") {
                return Err(invalid());
            }
            if owner.replace(DhcpcdOwner { global, ipv4_only }).is_some() {
                return Err(invalid());
            }
        }
    }
    Ok(owner)
}

fn dhcpcd_policy<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    name: &str,
    addresses: &[String],
    owner: DhcpcdOwner,
) -> Result<SourcePolicy, HandoffError> {
    let invalid = || {
        network_error("dhcpcd policy or active lease cannot be verified; use explicit networking")
    };
    let config = optional_network_file(&root.join("etc/dhcpcd.conf"))?.unwrap_or_default();
    let mut noipv6 = owner.ipv4_only;
    let mut nora = false;
    let mut nodhcp6 = false;
    let mut applicable = true;
    for line in config.lines() {
        let words: Vec<_> = line
            .split_whitespace()
            .take_while(|word| !word.starts_with('#'))
            .collect();
        match words.as_slice() {
            [] => {}
            ["interface", interface] => applicable = *interface == name,
            [
                "include" | "profile" | "ssid" | "allowinterfaces" | "denyinterfaces",
                ..,
            ] => return Err(invalid()),
            _ if !applicable => {}
            ["noipv6" | "ipv4only"] => noipv6 = true,
            ["noipv6rs"] => nora = true,
            ["nodhcp6"] => nodhcp6 = true,
            [
                "hostname" | "persistent" | "rapid_commit" | "require" | "option" | "slaac"
                | "nohook" | "timeout" | "waitip",
                ..,
            ] => {}
            _ => return Err(invalid()),
        }
    }
    let lease = local_output(runner, "dhcpcd", ["-4", "--dumplease", name])?
        .filter(|output| output.status == Some(0))
        .ok_or_else(invalid)?;
    let text = std::str::from_utf8(&lease.stdout).map_err(|_| invalid())?;
    if !lease_matches_ipv4(text, addresses) {
        return Err(invalid());
    }
    // Automatic IPv6 without RA cannot be recovered unambiguously from a
    // legacy daemon's lease view. Accept the normal RA-driven policy.
    if !noipv6 && nora && !nodhcp6 {
        return Err(invalid());
    }
    Ok(SourcePolicy::new(
        NetworkOwner::Dhcpcd,
        [true, !noipv6 && !nodhcp6, !noipv6 && !nora],
    ))
}

fn lease_matches_ipv4(text: &str, addresses: &[String]) -> bool {
    let mut ips = text.lines().filter_map(|line| {
        let (key, value) = line.split_once('=')?;
        matches!(key, "ip_address" | "new_ip_address").then_some(value.trim_matches(['\'', '"']))
    });
    let Some(lease) = ips.next().and_then(|value| value.parse::<Ipv4Addr>().ok()) else {
        return false;
    };
    ips.next().is_none()
        && addresses.len() == 1
        && parse_interface_address(&addresses[0], false)
            .is_ok_and(|(ip, _)| ip == IpAddr::V4(lease))
}

struct NmConnection {
    uuid: String,
    wireless: bool,
}

fn active_nm_connections<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
) -> Result<BTreeMap<String, NmConnection>, HandoffError> {
    let response = local_output(
        runner,
        "nmcli",
        [
            "--terse",
            "--escape",
            "no",
            "--fields",
            "UUID,TYPE,DEVICE",
            "connection",
            "show",
            "--active",
        ],
    )?;
    match response {
        Some(output) if output.status == Some(0) => parse_nm_active(
            &String::from_utf8(output.stdout)
                .map_err(|_| network_error("invalid active NetworkManager connection response"))?,
        ),
        Some(output) if output.status == Some(8) => Ok(BTreeMap::new()),
        // Running nmcli can fail without an initialized system bus. Only a
        // successful independent service check permits treating it as inactive.
        _ if !service_active(runner, "NetworkManager.service")?
            && !root.join("run/NetworkManager/NetworkManager.pid").exists() =>
        {
            Ok(BTreeMap::new())
        }
        _ => Err(network_error(
            "cannot read active NetworkManager connections; use explicit networking",
        )),
    }
}

fn parse_nm_active(text: &str) -> Result<BTreeMap<String, NmConnection>, HandoffError> {
    let invalid = || {
        network_error(
            "NetworkManager has an unsupported, incomplete or competing active connection; use explicit networking",
        )
    };
    let mut profiles = BTreeMap::new();
    for line in text.lines() {
        // Only the first two colons delimit fields. A Linux device name may
        // contain a colon. Active IDs and secrets are never requested.
        let mut fields = line.splitn(3, ':');
        let uuid = fields.next().ok_or_else(invalid)?;
        let kind = fields.next().ok_or_else(invalid)?;
        let name = fields.next().ok_or_else(invalid)?;
        if kind == "loopback" && name == "lo" {
            continue;
        }
        let wireless = match kind {
            "ethernet" | "802-3-ethernet" => false,
            "wifi" | "802-11-wireless" => true,
            _ => return Err(invalid()),
        };
        if !valid_interface_name(name)
            || uuid.len() != 36
            || !uuid.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
            || profiles
                .insert(
                    name.into(),
                    NmConnection {
                        uuid: uuid.into(),
                        wireless,
                    },
                )
                .is_some()
        {
            return Err(invalid());
        }
    }
    Ok(profiles)
}

const NM_FIELDS: &str = "connection.interface-name,connection.master,802-3-ethernet.cloned-mac-address,ipv4.method,ipv4.addresses,ipv4.gateway,ipv4.routes,ipv4.routing-rules,ipv4.route-table,ipv4.dhcp-client-id,ipv6.method,ipv6.addresses,ipv6.gateway,ipv6.routes,ipv6.routing-rules,ipv6.route-table,ipv6.dhcp-duid";

fn nm_policy<R: CommandRunner>(
    runner: &mut R,
    name: &str,
    uuid: &str,
) -> Result<SourcePolicy, HandoffError> {
    let command = spec(
        "nmcli",
        [
            "--terse",
            "--escape",
            "no",
            "--fields",
            NM_FIELDS,
            "connection",
            "show",
            "--active",
            "uuid",
            uuid,
        ],
        "handoff-networkmanager-profile",
    )
    .env("LC_ALL", "C")
    .inspection_limits();
    let output = capture(runner, &command).map_err(|_| {
        network_error("cannot read active NetworkManager Ethernet profile; use explicit networking")
    })?;
    parse_nm_profile(&output, name)
}

fn parse_nm_profile(text: &str, device: &str) -> Result<SourcePolicy, HandoffError> {
    let invalid = || {
        network_error(
            "unsupported NetworkManager profile settings or mixed address policies; use explicit networking",
        )
    };
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let (setting, value) = line.split_once(':').ok_or_else(invalid)?;
        if !NM_FIELDS.split(',').any(|name| name == setting)
            || fields.insert(setting, value.trim()).is_some()
        {
            return Err(invalid());
        }
    }
    let field = |name| match fields.get(name).copied() {
        Some("--") => Ok(""),
        Some(v) => Ok(v),
        None => Err(invalid()),
    };
    if !matches!(field("connection.interface-name")?, "" | "--")
        && field("connection.interface-name")? != device
        || !field("connection.master")?.is_empty()
        || !matches!(
            field("802-3-ethernet.cloned-mac-address")?,
            "" | "preserve" | "permanent"
        )
        || [
            "ipv4.routes",
            "ipv4.routing-rules",
            "ipv4.dhcp-client-id",
            "ipv6.routes",
            "ipv6.routing-rules",
            "ipv6.dhcp-duid",
        ]
        .into_iter()
        .any(|key| !field(key).is_ok_and(str::is_empty))
        || ["ipv4.route-table", "ipv6.route-table"]
            .into_iter()
            .any(|key| {
                !matches!(
                    field(key),
                    Ok("" | "0" | "0 (unspec)" | "254" | "254 (main)")
                )
            })
    {
        return Err(invalid());
    }
    let method4 = field("ipv4.method")?;
    let method6 = field("ipv6.method")?;
    if !matches!(method4, "auto" | "manual" | "disabled")
        || !matches!(method6, "auto" | "dhcp" | "manual" | "disabled")
    {
        return Err(invalid());
    }
    // NM's 'auto' follows RA, including its DHCPv6 managed/other flags.
    let mut policy = SourcePolicy::new(
        NetworkOwner::NetworkManager,
        [
            method4 == "auto",
            matches!(method6, "auto" | "dhcp"),
            method6 == "auto",
        ],
    );
    for (i, (method, field_address, field_gateway)) in [
        (method4, "ipv4.addresses", "ipv4.gateway"),
        (method6, "ipv6.addresses", "ipv6.gateway"),
    ]
    .into_iter()
    .enumerate()
    {
        let addresses = field(field_address)?;
        let gateway = field(field_gateway)?;
        if method == "manual" {
            if addresses.is_empty() {
                return Err(invalid());
            }
            let addresses = addresses
                .split(',')
                .map(|addr| {
                    let (ip, prefix) =
                        parse_interface_address(addr.trim(), i == 1).map_err(|_| invalid())?;
                    Ok(format!("{ip}/{prefix}"))
                })
                .collect::<Result<Vec<_>, HandoffError>>()?;
            policy.static_addresses[i] = Some(addresses);
            policy.static_gateways[i] = Some(configured_gateway(gateway, i == 1)?);
        } else if !addresses.is_empty() || !gateway.is_empty() {
            return Err(invalid());
        } else if method == "disabled" {
            policy.static_addresses[i] = Some(vec![]);
            policy.static_gateways[i] = Some(None);
        }
    }
    Ok(policy)
}

fn configured_gateway(text: &str, ipv6: bool) -> Result<Option<String>, HandoffError> {
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<IpAddr>() {
        Ok(ip)
            if ip.is_ipv6() == ipv6
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_loopback() =>
        {
            Ok(Some(ip.to_string()))
        }
        _ => Err(network_error("invalid gateway in source network profile")),
    }
}

fn wireless_link(root: &Path, name: &str) -> bool {
    let sys = root.join("sys/class/net").join(name);
    sys.join("wireless").is_dir() || sys.join("phy80211").exists()
}

fn wireless_requirement<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    name: &str,
    network_manager: bool,
) -> HandoffError {
    // iwctl is a read-only client here. Request device state, ignore the SSID
    // and never query the profile directory or any saved credentials.
    let connected = local_output(runner, "iwctl", ["station", name, "show"])
        .ok()
        .flatten()
        .filter(|response| response.status == Some(0))
        .is_some_and(|response| {
            String::from_utf8_lossy(&response.stdout)
                .lines()
                .any(|line| {
                    let mut words = line.split_whitespace();
                    words.next() == Some("State") && words.next() == Some("connected")
                })
        });
    let configured_by_iwd = iwd_ip_configuration(root).ok().flatten();
    network_error(match (network_manager, connected, configured_by_iwd) {
        (true, true, Some(false)) => {
            "NetworkManager owns addressing and iwd reports the station associated; Wi-Fi cannot be re-established in Live without verified firmware and private credentials"
        }
        (true, true, Some(true)) => {
            "NetworkManager and iwd may both configure addressing on this station; use supported wired networking"
        }
        (_, true, Some(true)) => {
            "iwd owns the Wi-Fi association and is configured for addressing; Live requires verified firmware and private credentials before reinstalling over Wi-Fi"
        }
        (_, true, Some(false)) => {
            "iwd reports the station associated and does not configure addressing; Live requires verified firmware and private credentials before reinstalling over Wi-Fi"
        }
        _ => {
            "source Wi-Fi cannot be inherited without verified Live firmware, association and private credentials; use a supported wired interface"
        }
    })
}

fn iwd_ip_configuration(root: &Path) -> Result<Option<bool>, HandoffError> {
    let Some(text) = optional_network_file(&root.join("etc/iwd/main.conf"))? else {
        return Ok(Some(false)); // iwd's documented default
    };
    let mut general = false;
    let mut result = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            general = line == "[General]";
        } else if general
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "EnableNetworkConfiguration"
        {
            if result.is_some() {
                return Ok(None);
            }
            result = match value.trim() {
                "true" => Some(true),
                "false" => Some(false),
                _ => return Ok(None),
            };
        }
    }
    Ok(Some(result.unwrap_or(false)))
}

fn optional_network_file(path: &Path) -> Result<Option<String>, HandoffError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(_) => {
            crate::shared::configure::validate_planned_parent(path)
                .map_err(|_| network_error("unsafe source network policy directory"))?;
            read_network_file(path, effective_uid()).map(Some)
        }
    }
}

/// An applied, flat networkd profile supplies authoritative automatic policy.
fn source_network_policy(root: &Path, index: u32) -> Result<[bool; 3], HandoffError> {
    let unknown = || {
        HandoffError::Invalid(
            "cannot determine applied networkd policy: require a flat profile with explicit DHCP= and IPv6AcceptRA=, or use explicit networking",
        )
    };
    let state_dir = root.join("run/systemd/netif");
    crate::shared::configure::validate_directory_tree(&root.join("run/systemd"))
        .map_err(|_| unknown())?;
    let owner = fs::symlink_metadata(&state_dir)
        .map_err(|_| unknown())?
        .uid();
    for directory in [&state_dir, &state_dir.join("links")] {
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir()
            || ![0, owner].contains(&metadata.uid())
            || metadata.mode() & 0o022 != 0
        {
            return Err(unknown());
        }
    }
    let state_path = state_dir.join("links").join(index.to_string());
    let state = read_network_file(&state_path, owner).map_err(|_| unknown())?;
    let mut profile = None;
    let mut configured = false;
    for line in state.lines() {
        if line == "ADMIN_STATE=configured" {
            configured = true;
        }
        if let Some(path) = line.strip_prefix("NETWORK_FILE=")
            && profile.replace(path).is_some()
        {
            return Err(unknown());
        }
    }
    let profile = profile.ok_or_else(unknown)?;
    let path = Path::new(profile);
    if !configured
        || ![
            "/etc/systemd/network",
            "/run/systemd/network",
            "/usr/lib/systemd/network",
        ]
        .iter()
        .any(|parent| path.parent() == Some(Path::new(parent)))
        || path.extension() != Some(OsStr::new("network"))
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(unknown());
    }
    // Merge semantics for hierarchical drop-ins are deliberately unsupported.
    let name = path
        .file_name()
        .ok_or_else(unknown)?
        .to_str()
        .ok_or_else(unknown)?;
    for parent in ["etc/systemd", "run/systemd", "usr/lib/systemd"] {
        let directory = root.join(parent);
        if fs::symlink_metadata(directory.join("networkd.conf.d")).is_ok() {
            return Err(unknown());
        }
        let global = directory.join("networkd.conf");
        if fs::symlink_metadata(&global).is_ok() {
            crate::shared::configure::validate_planned_file(&global).map_err(|_| unknown())?;
            let text = read_network_file(&global, effective_uid())?;
            if text
                .lines()
                .map(str::trim)
                .filter(|line| !line.starts_with(['#', ';']))
                .filter_map(|line| line.split_once('='))
                .any(|(_, value)| !value.trim().is_empty())
            {
                return Err(unknown());
            }
        }
    }
    for parent in [
        "etc/systemd/network",
        "run/systemd/network",
        "usr/lib/systemd/network",
    ] {
        let directory = root.join(parent);
        if fs::symlink_metadata(directory.join(format!("{name}.d"))).is_ok() {
            return Err(unknown());
        }
        if let Ok(entries) = fs::read_dir(&directory) {
            for entry in entries {
                let entry = entry?;
                // Also refuse prefix and global .network drop-ins.
                if entry.file_name().to_string_lossy().ends_with(".network.d") {
                    return Err(unknown());
                }
            }
        }
    }
    let path = root.join(path.strip_prefix("/").map_err(|_| unknown())?);
    crate::shared::configure::validate_planned_file(&path).map_err(|_| unknown())?;
    if fs::metadata(&path)?.modified()? > fs::metadata(&state_path)?.modified()? {
        return Err(HandoffError::Invalid(
            "network profile changed since it was applied; apply it before retrying",
        ));
    }
    let text = read_network_file(&path, effective_uid())?;
    parse_networkd_policy(&text)
}

fn read_network_file(path: &Path, owner: u32) -> Result<String, HandoffError> {
    // Linux O_NOFOLLOW | O_NONBLOCK: never block on a substituted FIFO.
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(0x20000 | 0x800)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() > MAX_TEXT as u64
        || ![0, owner].contains(&metadata.uid())
        || metadata.mode() & 0o022 != 0
    {
        return Err(HandoffError::Invalid("unsafe network policy file"));
    }
    let mut text = String::new();
    (&mut file)
        .take((MAX_TEXT + 1) as u64)
        .read_to_string(&mut text)?;
    if text.len() > MAX_TEXT || text.contains('\0') {
        return Err(HandoffError::Invalid("invalid network policy text"));
    }
    Ok(text)
}

fn parse_networkd_policy(text: &str) -> Result<[bool; 3], HandoffError> {
    let invalid = || {
        HandoffError::Invalid(
            "unsupported networkd policy: set DHCP=no|yes|ipv4|ipv6 and IPv6AcceptRA=yes|no explicitly in the applied flat profile; DHCPv6 without RA requires WithoutRA=solicit",
        )
    };
    let mut section = "";
    let mut dhcp = None;
    let mut ra = None;
    let mut without_ra = None;
    for line in text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with(['#', ';']))
    {
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(invalid)?;
        let (key, value) = (key.trim(), value.trim());
        // These change address ownership/topology beyond the three booleans.
        if matches!(
            key,
            "Include"
                | "Bridge"
                | "Bond"
                | "VRF"
                | "VLAN"
                | "MACVLAN"
                | "IPVLAN"
                | "Tunnel"
                | "IPv6SendRA"
                | "UseAutonomousPrefix"
                | "DHCPv6Client"
                | "KeepConfiguration"
        ) || value.ends_with('\\')
        {
            return Err(invalid());
        }
        if matches!(section, "DHCP" | "DHCPv4" | "DHCPv6" | "IPv6AcceptRA")
            && !matches!(key, "UseDNS" | "UseDomains" | "WithoutRA")
        {
            return Err(invalid());
        }
        match (section, key) {
            ("Network", "DHCP") => {
                let flags = match value {
                    "no" | "false" | "0" => [false, false],
                    "yes" | "true" | "1" => [true, true],
                    "ipv4" => [true, false],
                    "ipv6" => [false, true],
                    _ => return Err(invalid()),
                };
                if dhcp.replace(flags).is_some() {
                    return Err(invalid());
                }
            }
            ("Network", "IPv6AcceptRA") => {
                let yes = match value {
                    "yes" | "true" | "1" => true,
                    "no" | "false" | "0" => false,
                    _ => return Err(invalid()),
                };
                if ra.replace(yes).is_some() {
                    return Err(invalid());
                }
            }
            ("DHCPv6", "WithoutRA") => {
                without_ra = match without_ra {
                    None => Some(value),
                    Some(_) => return Err(invalid()),
                };
            }
            _ => {}
        }
    }
    let [dhcp4, dhcp6] = dhcp.ok_or_else(invalid)?;
    let accept_ra = ra.ok_or_else(invalid)?;
    if without_ra.is_some_and(|v| !matches!(v, "no" | "solicit"))
        || dhcp6 && !accept_ra && without_ra != Some("solicit")
    {
        return Err(invalid());
    }
    Ok([dhcp4, dhcp6, accept_ra])
}

fn validate_inherited_routes(
    rules: &str,
    routes: &str,
    family: AddressFamily,
    network: &[NetworkInterface],
    legacy_dhcp4: &BTreeSet<&str>,
) -> Result<(), HandoffError> {
    let invalid = || {
        HandoffError::Invalid(
            "unsupported inherited routing: require only local/main/default rules, one gateway default per family and connected routes; policy tables, VRFs and extra routes cannot be preserved",
        )
    };
    if rules.trim().is_empty()
        && routes.trim().is_empty()
        && family == AddressFamily::Ipv6
        && network.iter().all(|nic| {
            nic.ipv6_addresses.is_empty()
                && nic.ipv6_gateway.is_none()
                && !nic.dhcp6
                && !nic.accept_ra
        })
    {
        return Ok(());
    }
    let mut seen = BTreeSet::new();
    for line in rules.lines().filter(|line| !line.trim().is_empty()) {
        let words: Vec<_> = line.split_whitespace().collect();
        if words.len() != 5 || words[1..4] != ["from", "all", "lookup"] {
            return Err(invalid());
        }
        let expected = match words[0] {
            "0:" => ["local", "255"],
            "32766:" => ["main", "254"],
            "32767:" => ["default", "253"],
            _ => return Err(invalid()),
        };
        if !expected.contains(&words[4]) || !seen.insert(words[0]) {
            return Err(invalid());
        }
    }
    if !seen.contains("0:") || !seen.contains("32766:") {
        return Err(invalid());
    }
    let v6 = family == AddressFamily::Ipv6;
    let mut defaults = 0;
    for line in routes.lines().filter(|line| !line.trim().is_empty()) {
        let words: Vec<_> = line.split_whitespace().collect();
        let field = |key| words.windows(2).find(|p| p[0] == key).map(|p| p[1]);
        if words
            .iter()
            .any(|w| matches!(*w, "nexthop" | "nhid" | "encap" | "vrf"))
        {
            return Err(invalid());
        }
        if let Some(table) = field("table") {
            if matches!(table, "local" | "255") {
                if matches!(words[0], "local" | "broadcast" | "multicast")
                    && field("proto") == Some("kernel")
                    && field("via").is_none()
                {
                    continue;
                }
                return Err(invalid());
            }
            if !matches!(table, "main" | "254") {
                return Err(invalid());
            }
        }
        let nic = network
            .iter()
            .find(|nic| Some(nic.name.as_str()) == field("dev"))
            .ok_or_else(invalid)?;
        let gateway = if v6 {
            &nic.ipv6_gateway
        } else {
            &nic.ipv4_gateway
        };
        if words[0] == "default" {
            defaults += 1;
            if defaults > 1 || parse_default_gateway(line, family)?.as_ref() != gateway.as_ref() {
                return Err(invalid());
            }
            if (!v6
                && nic.dhcp4
                && field("proto") != Some("dhcp")
                && !(legacy_dhcp4.contains(nic.name.as_str())
                    && matches!(field("proto"), None | Some("boot"))))
                || (v6 && nic.accept_ra && field("proto") != Some("ra"))
                || (v6 && nic.dhcp6 && !nic.accept_ra)
            {
                return Err(HandoffError::Invalid(
                    "automatic address policy has a static or unsupported default gateway that cannot be preserved",
                ));
            }
            let addresses = if v6 {
                &nic.ipv6_addresses
            } else {
                &nic.ipv4_addresses
            };
            if field("src").is_some_and(|src| {
                addresses
                    .first()
                    .and_then(|value| value.split_once('/'))
                    .map(|(ip, _)| ip)
                    != Some(src)
            }) {
                return Err(HandoffError::Invalid(
                    "default route selects a non-primary source address that cannot be preserved",
                ));
            }
            continue;
        }
        if field("via").is_some() {
            return Err(invalid());
        }
        let destination = words[0];
        // Kernel connected routes, link-local IPv6, or an on-link provider
        // gateway host route are recreated by addresses/GatewayOnLink.
        let addresses = if v6 {
            &nic.ipv6_addresses
        } else {
            &nic.ipv4_addresses
        };
        if destination == "fe80::/64" && v6 && field("proto") == Some("kernel") {
            continue;
        }
        if gateway
            .as_deref()
            .is_some_and(|g| destination.trim_end_matches(if v6 { "/128" } else { "/32" }) == g)
        {
            continue;
        }
        if !addresses
            .iter()
            .any(|cidr| same_network_prefix(destination, cidr, v6))
        {
            return Err(invalid());
        }
    }
    if defaults
        != network
            .iter()
            .filter(|nic| {
                if v6 {
                    nic.ipv6_gateway.is_some()
                } else {
                    nic.ipv4_gateway.is_some()
                }
            })
            .count()
    {
        return Err(invalid());
    }
    Ok(())
}

fn same_network_prefix(route: &str, address: &str, v6: bool) -> bool {
    let parse = |value: &str| -> Option<(u128, u32)> {
        let (ip, prefix) = value
            .split_once('/')
            .unwrap_or((value, if v6 { "128" } else { "32" }));
        let prefix: u32 = prefix.parse().ok()?;
        let (number, bits) = match ip.parse::<std::net::IpAddr>().ok()? {
            std::net::IpAddr::V4(ip) if !v6 => (u32::from(ip) as u128, 32),
            std::net::IpAddr::V6(ip) if v6 => (u128::from(ip), 128),
            _ => return None,
        };
        if prefix > bits {
            return None;
        }
        let mask = if prefix == 0 {
            0
        } else {
            u128::MAX << (bits - prefix)
        };
        Some((number & mask, prefix))
    };
    parse(route)
        .zip(parse(address))
        .is_some_and(|(route, address)| route == address)
}

/// The manifest represents one simple default gateway per family and link.
/// Refuse multipath/ambiguous routes rather than silently selecting one.
pub fn parse_default_gateway(
    output: &str,
    family: AddressFamily,
) -> Result<Option<String>, HandoffError> {
    let lines: Vec<_> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return Ok(None);
    }
    if lines.len() != 1
        || lines[0].split_whitespace().next() != Some("default")
        || lines[0]
            .split_whitespace()
            .any(|word| matches!(word, "nexthop" | "nhid" | "encap"))
    {
        return Err(HandoffError::Invalid(
            "unsupported multipath/ambiguous default route",
        ));
    }
    let gateway = gateway_from_route_output(output).ok_or(HandoffError::Invalid(
        "default route requires a simple gateway",
    ))?;
    let ip = gateway
        .parse::<std::net::IpAddr>()
        .map_err(|_| HandoffError::Invalid("invalid default gateway"))?;
    if ip.is_ipv6() != (family == AddressFamily::Ipv6)
        || ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
    {
        return Err(HandoffError::Invalid(
            "invalid default gateway family/address",
        ));
    }
    Ok(Some(ip.to_string()))
}

pub fn parse_addresses(output: &str, family: AddressFamily) -> Result<Vec<String>, HandoffError> {
    let token = if family == AddressFamily::Ipv4 {
        "inet"
    } else {
        "inet6"
    };
    let mut result = Vec::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let index = fields
            .iter()
            .position(|value| *value == token)
            .ok_or(HandoffError::Invalid("malformed address inventory"))?;
        let value = fields
            .get(index + 1)
            .ok_or(HandoffError::Invalid("missing CIDR"))?
            .to_string();
        if fields
            .iter()
            .any(|field| matches!(*field, "tentative" | "dadfailed"))
        {
            return Err(HandoffError::Invalid(
                "inherited address has not passed duplicate address detection",
            ));
        }
        if fields.contains(&"temporary") {
            continue;
        }
        crate::deployment::parse_interface_address(&value, family == AddressFamily::Ipv6)
            .map_err(|_| HandoffError::Invalid("invalid inherited CIDR address"))?;
        if !result.contains(&value) {
            result.push(value);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandOutput, MockRunner};
    use crate::test_support::Scratch;
    use std::fs::File;
    use std::os::unix::fs::symlink;
    fn out(status: i32, text: &str) -> CommandOutput {
        CommandOutput::new(Some(status), text.as_bytes().to_vec(), vec![])
    }
    fn network_profile(root: &Path, policy: &str) {
        let file = root.join("etc/systemd/network/10-wan.network");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            file,
            format!("[Match]\nMACAddress=52:54:00:12:34:56\n{policy}"),
        )
        .unwrap();
        let state = root.join("run/systemd/netif/links/2");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(
            state,
            "ADMIN_STATE=configured\nNETWORK_FILE=/etc/systemd/network/10-wan.network\n",
        )
        .unwrap();
    }

    const RULES: &str =
        "0: from all lookup local\n32766: from all lookup main\n32767: from all lookup default\n";

    #[test]
    fn capture_separates_slaac_dhcpv6_static_and_independent_dhcpv4_policy() {
        for (dhcp, ra, v6, expected) in [
            ("no", true, true, [false, false, true]),
            ("ipv6", false, true, [false, true, false]),
            ("ipv6", true, true, [false, true, true]),
            ("no", false, true, [false, false, false]),
            ("ipv4", true, true, [true, false, true]),
            ("ipv4", false, false, [true, false, false]),
        ] {
            let root = Scratch::new("source-network");
            network_profile(
                &root.0,
                &format!(
                    "[Network]\nDHCP={dhcp}\nIPv6AcceptRA={}\n[DHCPv6]\nWithoutRA=solicit\n",
                    if ra { "yes" } else { "no" }
                ),
            );
            let v4 = format!(
                "2: eth0 inet 192.0.2.2/24 scope global {} eth0\n",
                if expected[0] { "dynamic" } else { "" }
            );
            let v6_address = if v6 {
                format!(
                    "2: eth0 inet6 2001:db8::2/64 scope global {}\n",
                    if expected[1] || ra { "dynamic" } else { "" }
                )
            } else {
                String::new()
            };
            let r4 = format!(
                "default via 192.0.2.1 dev eth0 proto {}\n",
                if expected[0] { "dhcp" } else { "static" }
            );
            let r6 = if ra {
                "default via fe80::1 dev eth0 proto ra\n"
            } else if v6 && !expected[1] {
                "default via fe80::1 dev eth0 proto static\n"
            } else {
                ""
            };
            let routes4 =
                format!("{r4}192.0.2.0/24 dev eth0 proto kernel scope link src 192.0.2.2\n");
            let routes6 = if v6 {
                format!(
                    "{r6}2001:db8::/64 dev eth0 proto kernel\nfe80::/64 dev eth0 proto kernel\n"
                )
            } else {
                String::new()
            };
            let mut runner = MockRunner::new([
                out(0, "active\n"),
                out(8, ""),
                out(0, "2: eth0: <UP> mtu 1500 link/ether 52:54:00:12:34:56\n"),
                out(0, &v4),
                out(0, &v6_address),
                out(0, &r4),
                out(0, r6),
                out(0, RULES),
                out(0, &routes4),
                out(0, if v6 { RULES } else { "" }),
                out(0, &routes6),
            ]);
            let network = collect_network_at(&mut runner, &root.0).unwrap();
            assert_eq!(
                [network[0].dhcp4, network[0].dhcp6, network[0].accept_ra],
                expected
            );
            assert_eq!(network[0].ipv4_addresses, ["192.0.2.2/24"]);
            assert_eq!(network[0].ipv6_addresses.len(), usize::from(v6));
            assert!(
                runner
                    .calls()
                    .iter()
                    .all(|c| c.program == "ip" || c.program == "systemctl" || c.program == "nmcli")
            );
        }
    }

    #[test]
    fn unknown_competing_stale_and_unsafe_policy_sources_fail_before_handoff() {
        let root = Scratch::new("source-network");
        assert!(source_network_policy(&root.0, 2).is_err());
        let mut runner = MockRunner::new([out(3, "inactive\n")]);
        assert!(collect_network_at(&mut runner, &root.0).is_err());
        let mut runner = MockRunner::new([out(0, "active\n"), out(0, "active\n")]);
        assert!(collect_network_at(&mut runner, &root.0).is_err());
        network_profile(&root.0, "[Network]\nDHCP=ipv4\nIPv6AcceptRA=yes\n");
        assert_eq!(
            source_network_policy(&root.0, 2).unwrap(),
            [true, false, true]
        );
        let file = root.0.join("etc/systemd/network/10-wan.network");
        let state = root.0.join("run/systemd/netif/links/2");
        File::open(&state)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
        assert!(source_network_policy(&root.0, 2).is_err());
        network_profile(&root.0, "[Network]\nDHCP=no\nIPv6AcceptRA=no\n");
        fs::create_dir(root.0.join("etc/systemd/network/10-wan.network.d")).unwrap();
        assert!(source_network_policy(&root.0, 2).is_err());
        fs::remove_dir(root.0.join("etc/systemd/network/10-wan.network.d")).unwrap();
        fs::rename(&file, file.with_extension("outside")).unwrap();
        symlink(file.with_extension("outside"), &file).unwrap();
        assert!(source_network_policy(&root.0, 2).is_err());
        fs::remove_file(&file).unwrap();
        fs::hard_link(file.with_extension("outside"), &file).unwrap();
        assert!(source_network_policy(&root.0, 2).is_err());
    }

    #[test]
    fn policy_parser_refuses_implicit_conflicting_or_extended_ipv6_semantics() {
        for policy in [
            "[Network]\nDHCP=ipv6\n",
            "[Network]\nDHCP=no\nIPv6AcceptRA=maybe\n",
            "[Network]\nDHCP=no\nDHCP=ipv6\nIPv6AcceptRA=yes\n",
            "[Network]\nDHCP=ipv6\nIPv6AcceptRA=no\n",
            "[Network]\nDHCP=ipv6\nIPv6AcceptRA=yes\n[IPv6AcceptRA]\nDHCPv6Client=no\n",
            "[Network]\nDHCP=no\nIPv6AcceptRA=yes\nUseAutonomousPrefix=no\n",
            "[Network]\nDHCP=ipv4\nIPv6AcceptRA=no\n[DHCPv4]\nClientIdentifier=mac\n",
        ] {
            assert!(parse_networkd_policy(policy).is_err(), "{policy}");
        }
    }

    #[test]
    fn inherited_routing_rejects_nonrepresentable_tables_rules_defaults_and_routes() {
        let nic = NetworkInterface {
            name: "eth0".into(),
            mac: "52:54:00:12:34:56".into(),
            ipv4_addresses: vec!["192.0.2.2/24".into()],
            ipv4_gateway: Some("192.0.2.1".into()),
            ipv6_addresses: vec![],
            ipv6_gateway: None,
            dhcp4: false,
            dhcp6: false,
            accept_ra: false,
        };
        let network = [nic];
        let routes = "default via 192.0.2.1 dev eth0 proto static\n192.0.2.0/24 dev eth0 proto kernel\nlocal 192.0.2.2 dev eth0 table local proto kernel\n";
        let no_legacy = BTreeSet::new();
        assert!(
            validate_inherited_routes(RULES, routes, AddressFamily::Ipv4, &network, &no_legacy)
                .is_ok()
        );
        for extra in [
            "100: from 192.0.2.2 lookup 100\n",
            "100: from all fwmark 1 lookup main\n",
            "1000: from all lookup [l3mdev-table]\n",
            "32766: from all lookup main\n",
        ] {
            assert!(
                validate_inherited_routes(
                    &format!("{RULES}{extra}"),
                    routes,
                    AddressFamily::Ipv4,
                    &network,
                    &no_legacy,
                )
                .is_err()
            );
        }
        for bad in [
            "default dev eth0\n",
            "default via 192.0.2.1 dev eth0\ndefault via 192.0.2.3 dev eth0\n",
            "default via 192.0.2.1 dev eth0 table 100\n",
            "default nexthop via 192.0.2.1 dev eth0 weight 1\n",
            "default nhid 3\n",
            "default via 192.0.2.1 dev eth0\n198.51.100.0/24 via 192.0.2.9 dev eth0\n",
            "default via 192.0.2.1 dev eth0\nblackhole 198.51.100.0/24\n",
            "default via 192.0.2.1 dev eth0\n192.0.2.0/24 dev eth0 table 100\n",
            "default via 192.0.2.1 dev eth0 table local proto static\n",
            "default via 192.0.2.1 dev eth0 src 192.0.2.99\n",
        ] {
            assert!(
                validate_inherited_routes(RULES, bad, AddressFamily::Ipv4, &network, &no_legacy)
                    .is_err(),
                "{bad}"
            );
        }
        let mut dynamic = network.clone();
        dynamic[0].dhcp4 = true;
        assert!(
            validate_inherited_routes(RULES, routes, AddressFamily::Ipv4, &dynamic, &no_legacy)
                .is_err()
        );
        assert!(
            validate_inherited_routes(
                RULES,
                &routes.replace("proto static", "proto dhcp"),
                AddressFamily::Ipv4,
                &dynamic,
                &no_legacy,
            )
            .is_ok()
        );
    }

    fn nm_fixture() -> String {
        NM_FIELDS
            .split(',')
            .map(|name| {
                let value = match name {
                    "connection.interface-name" => "ens3",
                    "ipv4.method" => "manual",
                    "ipv4.addresses" => "192.0.2.20/24",
                    "ipv4.gateway" => "192.0.2.1",
                    "ipv6.method" => "disabled",
                    _ => "--",
                };
                format!("{name}:{value}\n")
            })
            .collect()
    }

    #[test]
    fn networkmanager_uses_the_active_profile_and_checks_observed_policy() {
        let root = Scratch::new("source-nm");
        let uuid = "12345678-90ab-cdef-1234-0123456789ab";
        let routes = "default via 192.0.2.1 dev ens3 proto static\n192.0.2.0/24 dev ens3 proto kernel scope link src 192.0.2.20\n";
        let mut runner = MockRunner::new([
            out(3, ""),
            out(0, &format!("{uuid}:802-3-ethernet:ens3\n")),
            out(0, "2: ens3: <UP> mtu 1500 link/ether 52:54:00:12:34:56\n"),
            out(0, "2: ens3 inet 192.0.2.20/24 scope global ens3\n"),
            out(0, ""),
            out(0, &nm_fixture()),
            out(0, routes.lines().next().unwrap()),
            out(0, ""),
            out(0, RULES),
            out(0, routes),
            out(0, ""),
            out(0, ""),
        ]);
        let network = collect_network_at(&mut runner, &root.0).unwrap();
        assert_eq!(network[0].ipv4_addresses, ["192.0.2.20/24"]);
        assert_eq!(network[0].ipv4_gateway.as_deref(), Some("192.0.2.1"));
        assert!(!network[0].dhcp4);
        let profile = &runner.calls()[5];
        assert_eq!(profile.program, "nmcli");
        assert!(profile.args.iter().any(|arg| arg == uuid));
        assert!(profile.args.iter().all(|arg| arg != "--show-secrets"));
        assert!(
            parse_nm_profile(
                &nm_fixture().replace("ipv4.routes:--", "ipv4.routes:198.51.100.0/24"),
                "ens3"
            )
            .is_err()
        );
        assert!(parse_nm_active(&format!("{uuid}:vpn:ens3")).is_err());
    }

    #[test]
    fn ifupdown_checks_the_applied_flat_profile_and_native_alpine_syntax() {
        let root = Scratch::new("source-ifupdown");
        let path = root.0.join("etc/network/interfaces");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "source /etc/network/interfaces.d/*\n").unwrap();
        let include = path.parent().unwrap().join("interfaces.d/10-wan");
        fs::create_dir_all(include.parent().unwrap()).unwrap();
        fs::write(&include, "auto eth0\niface eth0 inet static\n  address 192.0.2.2\n  netmask 255.255.255.0\n  gateway 192.0.2.1\n").unwrap();
        let state = root.0.join("run/network/ifstate");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(&state, "lo=lo\neth0=eth0\n").unwrap();
        let policy = ifupdown_policy(&root.0, "eth0").unwrap().unwrap();
        assert_eq!(
            policy.static_addresses[0],
            Some(vec!["192.0.2.2/24".into()])
        );
        assert_eq!(policy.static_gateways[0], Some(Some("192.0.2.1".into())));
        assert!(ifupdown_policy(&root.0, "eth1").unwrap().is_none());
        let policy = parse_ifupdown_profile(
            "auto eth0\niface eth0\n  use dhcp\n  dhcp-opts -4\n  ipv6-accept-ra 0\n",
            "eth0",
        )
        .unwrap();
        assert!(policy.dhcp4 && !policy.dhcp6 && !policy.accept_ra);
        assert!(
            parse_ifupdown_profile("iface eth0 inet dhcp\n  pre-up /usr/bin/anything\n", "eth0")
                .is_err()
        );
        fs::write(&state, "eth0=other 1 explicit\n").unwrap();
        assert!(ifupdown_policy(&root.0, "eth0").is_err());
    }

    #[test]
    fn dhcpcd_requires_active_matching_lease_and_rejects_competing_owner() {
        let root = Scratch::new("source-dhcpcd");
        let proc = root.0.join("proc/25");
        fs::create_dir_all(&proc).unwrap();
        symlink("/usr/sbin/dhcpcd", proc.join("exe")).unwrap();
        let pid = root.0.join("run/dhcpcd.eth0-4.pid");
        fs::create_dir_all(pid.parent().unwrap()).unwrap();
        fs::write(&pid, "25\n").unwrap();
        let lease = active_dhcpcd(&root.0, "eth0").unwrap().unwrap();
        let addresses = vec!["192.0.2.2/24".to_owned()];
        let mut runner = MockRunner::new([out(0, "new_ip_address='192.0.2.2'\n")]);
        let policy = dhcpcd_policy(&mut runner, &root.0, "eth0", &addresses, lease).unwrap();
        assert!(policy.dhcp4 && !policy.dhcp6 && !policy.accept_ra);
        assert!(!lease_matches_ipv4(
            "new_ip_address=192.0.2.3\n",
            &addresses
        ));
        let nm = NmConnection {
            uuid: "active-profile".into(),
            wireless: false,
        };
        assert!(
            applied_policy(
                &mut runner,
                &root.0,
                "eth0",
                2,
                false,
                Some(&nm),
                &addresses
            )
            .is_err()
        );
        assert_eq!(runner.calls()[0].args, ["-4", "--dumplease", "eth0"]);
    }

    #[test]
    fn iwd_association_reports_address_ownership_without_reading_profiles() {
        let root = Scratch::new("source-iwd");
        let path = root.0.join("etc/iwd/main.conf");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "[General]\nEnableNetworkConfiguration=false\n").unwrap();
        let mut runner =
            MockRunner::new([out(0, "State connected\nConnected network example-wifi\n")]);
        let reason = wireless_requirement(&mut runner, &root.0, "wlan0", true).to_string();
        assert!(
            reason
                .contains("NetworkManager owns addressing and iwd reports the station associated")
        );
        assert!(!reason.contains("example-wifi"));
        assert_eq!(runner.calls()[0].args, ["station", "wlan0", "show"]);
        fs::write(&path, "[General]\nEnableNetworkConfiguration=true\n").unwrap();
        assert_eq!(iwd_ip_configuration(&root.0).unwrap(), Some(true));
    }
}
