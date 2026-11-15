//! Read-only source network observation. Live and target networking are separate.
use super::handoff::{HandoffError, MAX_TEXT, capture, spec};
use crate::command::{CommandRunner, OutputMode};
use crate::deployment::NetworkInterface;
use crate::fsutil::effective_uid;
use crate::worker::initrd_network::{AddressFamily, gateway_from_route_output};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};
pub fn collect_network<R: CommandRunner>(
    runner: &mut R,
) -> Result<Vec<NetworkInterface>, HandoffError> {
    collect_network_at(runner, Path::new("/"))
}

fn collect_network_at<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
) -> Result<Vec<NetworkInterface>, HandoffError> {
    let active = capture(runner, &spec("systemctl", ["is-active", "systemd-networkd.service"], "handoff-network-owner"))
        .map_err(|_| HandoffError::Invalid("applied network policy capture requires active systemd-networkd; other source network managers are unsupported"))?;
    if active.trim() != "active" {
        return Err(HandoffError::Invalid(
            "source systemd-networkd is not active",
        ));
    }
    let other = runner.run(
        &spec(
            "systemctl",
            ["is-active", "NetworkManager.service"],
            "handoff-network-owner",
        )
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Capture),
    )?;
    if !matches!(other.status, Some(3 | 4)) {
        return Err(HandoffError::Invalid(
            "cannot exclude competing NetworkManager ownership; use one supported active source network manager",
        ));
    }
    let links = capture(
        runner,
        &spec("ip", ["-o", "-d", "link", "show"], "handoff-network-links"),
    )?;
    let mut result = Vec::new();
    for line in links.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let Some(name) = fields.get(1).map(|value| {
            value
                .trim_end_matches(':')
                .split('@')
                .next()
                .unwrap_or_default()
        }) else {
            continue;
        };
        let Some(index) = fields.iter().position(|value| *value == "link/ether") else {
            continue;
        };
        if name == "lo" {
            continue;
        }
        if !crate::deployment::valid_interface_name(name) {
            return Err(HandoffError::Invalid("unsupported interface name"));
        }
        let mac = fields
            .get(index + 1)
            .ok_or(HandoffError::Invalid("missing interface MAC"))?
            .to_string();
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
        if fields.contains(&"master")
            || fields.iter().any(|word| {
                matches!(
                    *word,
                    "vrf"
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
            })
            || fields[1].contains('@')
        {
            return Err(HandoffError::Invalid(
                "addressed bridges, bonds, VRFs, VLANs and virtual links cannot be inherited",
            ));
        }
        let index = fields[0]
            .trim_end_matches(':')
            .parse::<u32>()
            .map_err(|_| HandoffError::Invalid("invalid interface index"))?;
        let [dhcp4, dhcp6, accept_ra] = source_network_policy(root, index)?;
        // Observations are a Live bootstrap snapshot, not policy. The current
        // manifest cannot represent static addresses mixed with automatic ones.
        for (automatic, output) in [(dhcp4, &v4), (dhcp6 || accept_ra, &v6)] {
            for line in output
                .lines()
                .filter(|line| !line.split_whitespace().any(|v| v == "temporary"))
            {
                if automatic != line.split_whitespace().any(|v| v == "dynamic") {
                    return Err(HandoffError::Invalid(
                        "observed addresses disagree with the applied profile, or mix static and automatic addressing",
                    ));
                }
            }
        }
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
        result.push(NetworkInterface {
            name: name.into(),
            mac,
            ipv4_addresses,
            ipv6_addresses,
            ipv4_gateway: parse_default_gateway(&route4, AddressFamily::Ipv4)?,
            ipv6_gateway: parse_default_gateway(&route6, AddressFamily::Ipv6)?,
            dhcp4,
            dhcp6,
            accept_ra,
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
        validate_inherited_routes(&rules, &routes, family, &result)?;
    }
    Ok(result)
}

/// Only an applied, flat networkd profile currently supplies authoritative
/// automatic-address policy. Unknown managers and drop-ins fail before erase.
fn source_network_policy(root: &Path, index: u32) -> Result<[bool; 3], HandoffError> {
    let unknown = || {
        HandoffError::Invalid(
            "cannot determine applied DHCP/RA policy: require an active flat systemd-networkd profile with explicit DHCP= and IPv6AcceptRA=; other managers and drop-ins are unsupported",
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
            if (!v6 && nic.dhcp4 && field("proto") != Some("dhcp"))
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
                out(3, "inactive\n"),
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
                    .all(|c| c.program == "ip" || c.program == "systemctl")
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
        assert!(validate_inherited_routes(RULES, routes, AddressFamily::Ipv4, &network).is_ok());
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
                    &network
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
                validate_inherited_routes(RULES, bad, AddressFamily::Ipv4, &network).is_err(),
                "{bad}"
            );
        }
        let mut dynamic = network.clone();
        dynamic[0].dhcp4 = true;
        assert!(validate_inherited_routes(RULES, routes, AddressFamily::Ipv4, &dynamic).is_err());
        assert!(
            validate_inherited_routes(
                RULES,
                &routes.replace("proto static", "proto dhcp"),
                AddressFamily::Ipv4,
                &dynamic
            )
            .is_ok()
        );
    }
}
