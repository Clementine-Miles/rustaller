//! Authenticated host preparation and init-aware kexec handoff.
//! All mutations require a disk-identity-bound confirmation. Tests use only mocks.
use super::{disk, initrd, preflight, sources};
use crate::codec::{decode_plan, encode_plan};
use crate::command::{
    CommandError, CommandInput, CommandOutput, CommandRunner, CommandSpec, OutputMode,
    PipelineOutput, run_checked,
};
use crate::deployment::{DeploymentManifest, DiskIdentity, NetworkInterface, write_manifest};
use crate::fsutil::effective_uid;
use crate::model::{BootMode, InstallPlan, Target};
use crate::worker::initrd_network::{
    AddressFamily, InitrdNetworkConfig, ProxyEndpoint, gateway_from_route_output,
};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{
    DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink,
};
use std::path::{Component, Path, PathBuf};

/// Published by https://alpinelinux.org/downloads/ and its /keys/ncopa.asc link.
pub const ALPINE_RELEASE_FINGERPRINT: &str = "0482D84022F52DF1C4E7CD43293ACD0907D9495A";
const ALPINE_RELEASE_KEY: &[u8] = include_bytes!("alpine-release.asc");
const MAX_TEXT: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostInit {
    Systemd,
    OpenRcInit,
}

pub struct HostDeployment {
    pub manifest: DeploymentManifest,
    pub confirmation: String,
    pub init: HostInit,
    pub alpine: sources::AlpineNetboot,
}

pub struct PreparedHandoff {
    workspace: PathBuf,
    kernel: PathBuf,
    initrd: PathBuf,
    cmdline: String,
    disk: DiskIdentity,
    init: HostInit,
    boot_mode: BootMode,
}
impl PreparedHandoff {
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
    pub fn kernel(&self) -> &Path {
        &self.kernel
    }
    pub fn initrd(&self) -> &Path {
        &self.initrd
    }
    pub fn command_line(&self) -> &str {
        &self.cmdline
    }
}

#[derive(Debug)]
pub enum HandoffError {
    Invalid(&'static str),
    Filesystem(io::Error),
    Command(CommandError),
    MissingTool(&'static str),
    Preflight(preflight::PreflightError),
    Kexec(preflight::KexecError),
    ArmedImage(Box<HandoffError>),
}
impl fmt::Display for HandoffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "Cannot prepare installer handoff: {reason}"),
            Self::Filesystem(error) => write!(f, "Installer staging failed: {error}"),
            Self::Command(error) => error.fmt(f),
            Self::Preflight(error) => error.fmt(f),
            Self::Kexec(error) => error.fmt(f),
            Self::ArmedImage(error) => write!(
                f,
                "{error}; the installer kexec image could not be unloaded, so another kexec request must not be issued until it is unloaded"
            ),
            Self::MissingTool(tool) => write!(f, "Required host tool is unavailable: {tool}"),
        }
    }
}
impl std::error::Error for HandoffError {}
impl From<io::Error> for HandoffError {
    fn from(error: io::Error) -> Self {
        Self::Filesystem(error)
    }
}
impl From<CommandError> for HandoffError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

/// Read-only discovery. Downloads only bounded release metadata into memory.
pub fn discover<R: CommandRunner>(
    runner: &mut R,
    plan: &InstallPlan,
) -> Result<HostDeployment, HandoffError> {
    plan.validate()
        .map_err(|_| HandoffError::Invalid("invalid installation plan"))?;
    if !plan.target.is_automatic() {
        return Err(HandoffError::Invalid(
            "kexec handoff requires an automatic target or Alpine rescue",
        ));
    }
    preflight::collect_host_facts(runner, plan.target).map_err(HandoffError::Preflight)?;
    let boot_mode =
        preflight::validate_source_boot(Path::new("/")).map_err(HandoffError::Invalid)?;
    validate_boot_mode(plan, boot_mode)?;
    let init_path = fs::read_link("/proc/1/exe")?;
    let init = detect_init(&init_path)?;
    for tool in [
        "curl",
        "gpg",
        "tar",
        "gzip",
        "zcat",
        "cpio",
        "kexec",
        "ip",
        "lsblk",
        "sha256sum",
        "ssh-keygen",
    ] {
        if !program_available(tool) {
            return Err(HandoffError::MissingTool(tool));
        }
    }
    if !program_available(match init {
        HostInit::Systemd => "systemctl",
        HostInit::OpenRcInit => "openrc-shutdown",
    }) {
        return Err(HandoffError::Invalid(
            "native init shutdown tool is missing",
        ));
    }
    let disabled = read_text(Path::new("/proc/sys/kernel/kexec_load_disabled"))?;
    let lockdown = match read_text(Path::new("/sys/kernel/security/lockdown")) {
        Ok(text) => Some(text),
        Err(HandoffError::Filesystem(error)) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let kernel_config = running_kernel_config(runner)?;
    preflight::validate_one_reboot_support(&kernel_config, &disabled, lockdown.as_deref(), true)
        .map_err(HandoffError::Kexec)?;
    let roots = disk::discover_root_disks(runner)
        .map_err(|_| HandoffError::Invalid("cannot bind a unique source root disk"))?;
    let selected = plan
        .target_disk
        .as_deref()
        .map(|path| path.strip_prefix("/dev/").unwrap_or(path));
    let name = disk::select_root_disk(&roots, selected)
        .map_err(|_| HandoffError::Invalid("select one whole source root disk explicitly"))?;
    let device = format!("/dev/{name}");
    disk::validate_target_disk_path(Path::new(&device))
        .map_err(|_| HandoffError::Invalid("selected disk is not a block device"))?;
    let identity = read_disk_identity(runner, &device)?;
    crate::shared::storage::validate_disk_geometry(runner, &device, boot_mode).map_err(|_| {
        HandoffError::Invalid(
            "disk is read-only or its logical sector size is unsupported for this firmware",
        )
    })?;
    let network = collect_network(runner)?;
    let alpine =
        sources::parse_alpine_netboot(&fetch_text(runner, sources::ALPINE_RELEASES_URL)?, "x86_64")
            .map_err(|_| HandoffError::Invalid("invalid official Alpine netboot metadata"))?;
    let debian_codename = if plan.target == Target::Debian {
        match sources::parse_debian_release_metadata(&fetch_text(
            runner,
            sources::DEBIAN_RELEASE_URL,
        )?)
        .map_err(|_| HandoffError::Invalid("invalid Debian stable metadata"))?
        {
            sources::StableRelease::Debian { codename, .. } => codename,
            _ => unreachable!(),
        }
    } else {
        String::new()
    };
    let gentoo_stage3 = if plan.target == Target::Gentoo {
        Some(
            sources::parse_gentoo_stage3(
                &fetch_text(runner, sources::GENTOO_STAGE3_INDEX_URL)?,
                "x86_64",
            )
            .map_err(|_| HandoffError::Invalid("invalid Gentoo systemd stage3 metadata"))?,
        )
    } else {
        None
    };
    let manifest = DeploymentManifest {
        disk: identity,
        boot_mode,
        alpine_branch: alpine.branch.clone(),
        debian_codename,
        gentoo_stage3,
        network,
    };
    manifest
        .validate()
        .map_err(|_| HandoffError::Invalid("unsupported disk or network inventory"))?;
    if plan.hold != Some(1) {
        crate::shared::network::validate_network_policy(plan, &manifest).map_err(|_| {
            HandoffError::Invalid("incompatible inherited network and target policy")
        })?;
    }
    let confirmation = confirmation_for_plan(plan, &manifest.disk);
    Ok(HostDeployment {
        manifest,
        confirmation,
        init,
        alpine,
    })
}

pub fn detect_init(executable: &Path) -> Result<HostInit, HandoffError> {
    match executable.file_name().and_then(OsStr::to_str) {
        Some("systemd") => Ok(HostInit::Systemd),
        Some("openrc-init") => Ok(HostInit::OpenRcInit),
        _ => Err(HandoffError::Invalid(
            "one-reboot handoff currently requires systemd or openrc-init PID 1; BusyBox/SysV shutdown has no verified kexec adapter",
        )),
    }
}

pub fn confirmation_for(disk: &DiskIdentity) -> String {
    format!(
        "ERASE {} {} SERIAL={} WWN={} PTUUID={}",
        disk.device, disk.size_bytes, disk.serial, disk.wwn, disk.partition_table_id
    )
}
pub fn confirmation_for_plan(plan: &InstallPlan, disk: &DiskIdentity) -> String {
    if plan.hold == Some(1) {
        confirmation_for(disk).replacen("ERASE ", "RESCUE ", 1)
    } else {
        confirmation_for(disk)
    }
}

fn validate_boot_mode(plan: &InstallPlan, running: BootMode) -> Result<(), HandoffError> {
    if plan.force_boot_mode.is_some_and(|mode| mode != running) {
        return Err(HandoffError::Invalid(
            "one-reboot installation cannot change the running BIOS/UEFI firmware mode",
        ));
    }
    Ok(())
}

pub fn read_disk_identity<R: CommandRunner>(
    runner: &mut R,
    device: &str,
) -> Result<DiskIdentity, HandoffError> {
    if !valid_device(device) {
        return Err(HandoffError::Invalid("unsafe disk device path"));
    }
    let mut values = Vec::new();
    for column in ["SIZE", "SERIAL", "WWN", "PTUUID"] {
        let text = capture(
            runner,
            &spec(
                "lsblk",
                ["-dnbro", column, "--", device],
                "handoff-disk-identity",
            ),
        )?;
        if text.lines().count() > 1 || text.trim().len() > 256 {
            return Err(HandoffError::Invalid("ambiguous disk identity"));
        }
        values.push(text.trim().to_owned());
    }
    let size_bytes = values[0]
        .parse()
        .map_err(|_| HandoffError::Invalid("invalid disk byte size"))?;
    Ok(DiskIdentity {
        device: device.into(),
        size_bytes,
        serial: values[1].clone(),
        wwn: values[2].clone(),
        partition_table_id: values[3].clone(),
    })
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

/// Start preparation only after the user reviewed and typed the exact token.
/// Workspace must be a new absolute directory; failure never modifies boot state.
pub fn prepare<R: CommandRunner>(
    runner: &mut R,
    plan: &InstallPlan,
    deployment: &HostDeployment,
    confirmation: &str,
    workspace: &Path,
    worker: &[u8],
) -> Result<PreparedHandoff, HandoffError> {
    plan.validate_install_credentials()
        .map_err(|_| HandoffError::Invalid("invalid installation plan"))?;
    deployment
        .manifest
        .validate()
        .map_err(|_| HandoffError::Invalid("invalid deployment inventory"))?;
    validate_boot_mode(plan, deployment.manifest.boot_mode)?;
    validate_selected_source(deployment)?;
    if !plan.target.is_automatic() {
        return Err(HandoffError::Invalid(
            "automatic target or Alpine rescue required",
        ));
    }
    if confirmation != deployment.confirmation
        || confirmation != confirmation_for_plan(plan, &deployment.manifest.disk)
    {
        return Err(HandoffError::Invalid(
            "handoff confirmation does not match this action and device identity",
        ));
    }
    if plan.password.is_none() && plan.ssh_keys.is_empty() {
        return Err(HandoffError::Invalid(
            "configure a login password or public key before handoff",
        ));
    }
    if read_disk_identity(runner, &deployment.manifest.disk.device)? != deployment.manifest.disk {
        return Err(HandoffError::Invalid(
            "disk identity changed after confirmation",
        ));
    }
    validate_new_workspace(workspace)?;
    DirBuilder::new().mode(0o700).create(workspace)?;
    let outcome = prepare_owned(runner, plan, deployment, workspace, worker);
    if outcome.is_err() {
        let _ = fs::remove_dir_all(workspace);
    }
    outcome
}

fn prepare_owned<R: CommandRunner>(
    runner: &mut R,
    plan: &InstallPlan,
    deployment: &HostDeployment,
    workspace: &Path,
    worker: &[u8],
) -> Result<PreparedHandoff, HandoffError> {
    let transport = Transport::from_environment()?;
    prepare_with_transport(runner, plan, deployment, workspace, worker, &transport)
}

fn prepare_with_transport<R: CommandRunner>(
    runner: &mut R,
    plan: &InstallPlan,
    deployment: &HostDeployment,
    workspace: &Path,
    worker: &[u8],
    transport: &Transport,
) -> Result<PreparedHandoff, HandoffError> {
    let archive = workspace.join(&deployment.alpine.filename);
    download(runner, &deployment.alpine.url, &archive, 1024 * 1024 * 1024)?;
    let signature = workspace.join("alpine-netboot.asc");
    download(
        runner,
        &deployment.alpine.signature_url,
        &signature,
        32 * 1024,
    )?;
    verify_alpine_archive(
        runner,
        workspace,
        &archive,
        &signature,
        &deployment.alpine.sha256,
    )?;
    let extracted = workspace.join("netboot");
    fs::create_dir(&extracted)?;
    let flavor = if plan.no_cloud_kernel { "lts" } else { "virt" };
    let kernel_member = format!("boot/vmlinuz-{flavor}");
    let initrd_member = format!("boot/initramfs-{flavor}");
    let modloop_member = format!("boot/modloop-{flavor}");
    run_checked(
        runner,
        &spec(
            "tar",
            [
                OsStr::new("-xzf"),
                archive.as_os_str(),
                "--no-same-owner".as_ref(),
                "-C".as_ref(),
                extracted.as_os_str(),
                kernel_member.as_ref(),
                initrd_member.as_ref(),
                modloop_member.as_ref(),
            ],
            "handoff-extract-verified-netboot",
        ),
    )?;
    let kernel = extracted.join(kernel_member);
    let original_initrd = extracted.join(initrd_member);
    let modloop = extracted.join(modloop_member);
    for path in [&kernel, &original_initrd, &modloop] {
        validate_payload(path)?;
    }
    let mut encoded = encode_plan(plan).map_err(|_| HandoffError::Invalid("cannot encode plan"))?;
    let decoded = decode_plan(&encoded);
    encoded.fill(0);
    let mut resolved = decoded.map_err(|_| HandoffError::Invalid("cannot copy validated plan"))?;
    resolved.target_disk = Some(deployment.manifest.disk.device.clone());
    resolved.ssh_keys = resolve_public_keys(runner, workspace, &plan.ssh_keys)?;
    let overlay = workspace.join("overlay");
    fs::create_dir(&overlay)?;
    for path in ["configs", "etc/apk", "etc/init.d", "etc/runlevels/default"] {
        fs::create_dir_all(overlay.join(path))?;
    }
    transport.stage(&overlay)?;
    if !transport.environment.is_empty() {
        fs::create_dir(overlay.join("etc/conf.d"))?;
        private_file(
            &overlay.join("etc/conf.d/rustaller"),
            transport.environment.as_bytes(),
            0o600,
        )?;
    }
    if let Some(source) = &plan.frpc_config {
        let contents = source_text(runner, workspace, "frpc-source", source)?;
        private_file(
            &overlay.join("configs/frpc.toml"),
            contents.as_bytes(),
            0o600,
        )?;
        resolved.frpc_config = Some("/configs/frpc.toml".into());
    }
    initrd::install_worker_and_plan(&overlay, &resolved, worker)
        .map_err(|_| HandoffError::Invalid("cannot stage matching worker and private plan"))?;
    write_manifest(
        &overlay.join("configs/deployment.manifest"),
        &deployment.manifest,
    )
    .map_err(|_| HandoffError::Invalid("cannot stage deployment manifest"))?;
    fs::copy(&modloop, overlay.join("modloop-virt"))?;
    private_file(&overlay.join("etc/.default_boot_services"), b"", 0o600)?;
    // A provider's attached cidata disk must not inject tiny-cloud services or
    // configuration into the private installer environment.
    private_file(&overlay.join("etc/tiny-cloud.disabled"), b"", 0o600)?;
    let live_world = live_package_world(&resolved);
    private_file(&overlay.join("etc/apk/world"), live_world.as_bytes(), 0o644)?;
    let repos = format!(
        "{}/{}/main\n{}/{}/community\n",
        sources::ALPINE_ARCHIVE,
        deployment.alpine.branch,
        sources::ALPINE_ARCHIVE,
        deployment.alpine.branch
    );
    private_file(
        &overlay.join("etc/apk/repositories"),
        repos.as_bytes(),
        0o644,
    )?;
    private_file(
        &overlay.join("etc/init.d/rustaller"),
        LIVE_SERVICE.as_bytes(),
        0o755,
    )?;
    private_file(
        &overlay.join("etc/init.d/modloop"),
        LIVE_MODULE_SERVICE.as_bytes(),
        0o755,
    )?;
    symlink(
        "/etc/init.d/rustaller",
        overlay.join("etc/runlevels/default/rustaller"),
    )?;
    let overlay_archive = workspace.join("rustaller.apkovl.tar.gz");
    run_checked(
        runner,
        &spec(
            "tar",
            [
                OsStr::new("-czf"),
                overlay_archive.as_os_str(),
                "--numeric-owner".as_ref(),
                "--owner=0".as_ref(),
                "--group=0".as_ref(),
                "-C".as_ref(),
                overlay.as_os_str(),
                ".".as_ref(),
            ],
            "handoff-pack-private-overlay",
        ),
    )?;
    fs::set_permissions(&overlay_archive, fs::Permissions::from_mode(0o600))?;
    let initrd_root = workspace.join("initrd-root");
    let configs = deployment
        .manifest
        .network
        .iter()
        .map(|nic| {
            InitrdNetworkConfig::new(
                &nic.mac,
                nic.ipv4_addresses
                    .first()
                    .map(String::as_str)
                    .unwrap_or_default(),
                nic.ipv4_gateway.as_deref().unwrap_or_default(),
                nic.ipv6_addresses
                    .first()
                    .map(String::as_str)
                    .unwrap_or_default(),
                nic.ipv6_gateway.as_deref().unwrap_or_default(),
                nic.ipv6_addresses
                    .iter()
                    .skip(1)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(","),
            )
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| HandoffError::Invalid("invalid inherited initrd network"))?;
    let mut checked = CheckedPipelines(runner);
    initrd::unpack_initrd(&mut checked, &original_initrd, &initrd_root)
        .map_err(|_| HandoffError::Invalid("verified initrd could not be unpacked"))?;
    initrd::stage_alpine_initrd(&initrd_root, &resolved, worker, &configs)
        .map_err(|_| HandoffError::Invalid("unsupported Alpine initrd network anchor"))?;
    transport.stage(&initrd_root)?;
    if !transport.environment.is_empty() {
        let init_path = initrd_root.join("init");
        let contents = read_text(&init_path)?;
        let (shebang, remainder) = contents.split_once('\n').ok_or(HandoffError::Invalid(
            "Alpine initrd init lacks its interpreter line",
        ))?;
        if !shebang.starts_with("#!") {
            return Err(HandoffError::Invalid(
                "Alpine initrd init lacks its interpreter line",
            ));
        }
        let script = format!(
            "{shebang}\n# rustaller private transport settings; never placed on the kernel command line\n. /configs/transport.env\n{remainder}"
        );
        crate::fsutil::atomic_replace(&init_path, script.as_bytes(), 0o755)?;
    }
    fs::copy(
        &overlay_archive,
        initrd_root.join("rustaller.apkovl.tar.gz"),
    )?;
    fs::set_permissions(
        initrd_root.join("rustaller.apkovl.tar.gz"),
        fs::Permissions::from_mode(0o600),
    )?;
    let prepared_initrd = workspace.join("initramfs-rustaller");
    initrd::repack_initrd(&mut checked, &initrd_root, &prepared_initrd)
        .map_err(|_| HandoffError::Invalid("prepared initrd pipeline failed"))?;
    fs::set_permissions(&prepared_initrd, fs::Permissions::from_mode(0o600))?;
    validate_payload(&prepared_initrd)?;
    let cmdline = format!(
        "alpine_repo={}/{}/main apkovl=/rustaller.apkovl.tar.gz modloop=/modloop-virt rustaller_live=1 console=tty0 console=ttyS0,115200n8 modules=loop,squashfs",
        sources::ALPINE_ARCHIVE,
        deployment.alpine.branch
    );
    Ok(PreparedHandoff {
        workspace: workspace.to_owned(),
        kernel,
        initrd: prepared_initrd,
        cmdline,
        disk: deployment.manifest.disk.clone(),
        init: deployment.init,
        boot_mode: deployment.manifest.boot_mode,
    })
}

const LIVE_SERVICE: &str = "#!/sbin/openrc-run\nname=\"rustaller Live installer\"\ndescription=\"Install the identity-bound target from the private Rust plan\"\ncommand=/rustaller-worker\ncommand_args=\"live-install /configs/install.plan /configs/deployment.manifest\"\ncommand_background=yes\npidfile=/run/rustaller-installer.pid\noutput_log=/var/log/rustaller.log\nerror_log=/var/log/rustaller.log\ndepend() {\n    need modloop\n}\n";
const LIVE_MODULE_SERVICE: &str = "#!/sbin/openrc-run\nname=\"verified rustaller Live modules\"\ndepend() {\n    before hwdrivers modules\n}\nstart() {\n    /rustaller-worker live-modules\n}\n";

fn live_package_world(plan: &InstallPlan) -> String {
    // Alpine 3.24 util-linux is an empty compatibility metapackage. The Live
    // guard runs findmnt/lsblk before it can install any further prerequisites.
    let mut world = "alpine-base\nca-certificates\ncurl\ngnupg\nopenssh\nfindmnt\nlsblk\nblkid\nmount\numount\nutil-linux-misc\n".to_owned();
    if plan.username.as_deref().is_some_and(|name| name != "root") {
        world.push_str("sudo\n");
    }
    world
}

/// Load using kexec_file_load, verify loaded state, then ask PID 1 to shut down
/// services and execute the loaded kernel. Rust never installs a GRUB fallback
/// or requests a legacy load. Native PID 1 may restart the original OS if the
/// actual kexec syscall fails; one final firmware reboot describes success.
pub fn execute<R: CommandRunner>(
    runner: &mut R,
    prepared: &PreparedHandoff,
) -> Result<(), HandoffError> {
    if preflight::validate_source_boot(Path::new("/")).map_err(HandoffError::Invalid)?
        != prepared.boot_mode
    {
        return Err(HandoffError::Invalid(
            "firmware mode changed during preparation",
        ));
    }
    validate_payload(&prepared.kernel)?;
    validate_payload(&prepared.initrd)?;
    if read_disk_identity(runner, &prepared.disk.device)? != prepared.disk {
        return Err(HandoffError::Invalid("disk identity changed before kexec"));
    }
    crate::shared::storage::validate_disk_geometry(
        runner,
        &prepared.disk.device,
        prepared.boot_mode,
    )
    .map_err(|_| HandoffError::Invalid("disk geometry or write access changed before kexec"))?;
    let memory = read_text(Path::new("/proc/meminfo"))?;
    let available = memory
        .lines()
        .find_map(|line| {
            line.strip_prefix("MemAvailable:")
                .and_then(|line| line.split_whitespace().next())
                .and_then(|number| number.parse::<u64>().ok())
        })
        .ok_or(HandoffError::Invalid(
            "cannot establish kexec memory headroom",
        ))?
        * 1024;
    let required = fs::metadata(&prepared.kernel)?
        .len()
        .saturating_add(fs::metadata(&prepared.initrd)?.len())
        .saturating_add(64 * 1024 * 1024);
    if available < required {
        return Err(HandoffError::Invalid(
            "not enough available RAM to load verified kernel/initrd safely",
        ));
    }
    let current_init = detect_init(&fs::read_link("/proc/1/exe")?)?;
    if current_init != prepared.init {
        return Err(HandoffError::Invalid("PID 1 changed during preparation"));
    }
    if current_init == HostInit::OpenRcInit
        && !fs::metadata("/run/openrc/init.fifo")
            .is_ok_and(|metadata| metadata.file_type().is_fifo())
    {
        return Err(HandoffError::Invalid(
            "openrc-init command FIFO is unavailable",
        ));
    }
    execute_with_facts(runner, prepared, available, current_init)
}

fn execute_with_facts<R: CommandRunner>(
    runner: &mut R,
    prepared: &PreparedHandoff,
    available: u64,
    current_init: HostInit,
) -> Result<(), HandoffError> {
    let required = fs::metadata(&prepared.kernel)?
        .len()
        .saturating_add(fs::metadata(&prepared.initrd)?.len())
        .saturating_add(64 * 1024 * 1024);
    if available < required || current_init != prepared.init {
        return Err(HandoffError::Invalid("kexec execution facts changed"));
    }
    let prior = capture(
        runner,
        &spec(
            "cat",
            ["/sys/kernel/kexec_loaded"],
            "handoff-check-existing-kernel",
        ),
    )?;
    if prior.trim() != "0" {
        return Err(HandoffError::Invalid(
            "a kexec image is already loaded; refusing to replace another operation",
        ));
    }
    let mut args = vec![
        OsStr::new("-s").to_owned(),
        "-l".into(),
        prepared.kernel.as_os_str().to_owned(),
    ];
    args.push(format!("--initrd={}", prepared.initrd.display()).into());
    args.push(format!("--append={}", prepared.cmdline).into());
    run_checked(runner, &spec("kexec", args, "handoff-kexec-file-load"))?;
    let handoff = (|| {
        let loaded = capture(
            runner,
            &spec(
                "cat",
                ["/sys/kernel/kexec_loaded"],
                "handoff-check-loaded-kernel",
            ),
        )?;
        if loaded.trim() != "1" {
            return Err(HandoffError::Invalid(
                "kernel did not report a loaded kexec image",
            ));
        }
        let shutdown = match prepared.init {
            HostInit::Systemd => spec("systemctl", ["kexec"], "handoff-init-kexec"),
            HostInit::OpenRcInit => {
                spec("openrc-shutdown", ["--kexec", "now"], "handoff-init-kexec")
            }
        };
        run_checked(runner, &shutdown)?;
        Ok(())
    })();
    if let Err(error) = handoff {
        if run_checked(
            runner,
            &spec("kexec", ["-s", "-u"], "handoff-unload-owned-kernel"),
        )
        .is_err()
        {
            return Err(HandoffError::ArmedImage(Box::new(error)));
        }
        return Err(error);
    }
    Ok(())
}

fn validate_selected_source(deployment: &HostDeployment) -> Result<(), HandoffError> {
    let source = &deployment.alpine;
    let parts: Vec<_> = source.version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
        || source.branch != format!("v{}.{}", parts[0], parts[1])
        || source.branch != deployment.manifest.alpine_branch
        || source.filename != format!("alpine-netboot-{}-x86_64.tar.gz", source.version)
        || source.url
            != format!(
                "{}/{}/releases/x86_64/{}",
                sources::ALPINE_ARCHIVE,
                source.branch,
                source.filename
            )
        || source.signature_url != format!("{}.asc", source.url)
        || source.checksum_url != format!("{}.sha256", source.url)
        || source.sha256.len() != 64
        || !source.sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(HandoffError::Invalid(
            "netboot source identity differs from official metadata",
        ));
    }
    Ok(())
}

pub fn verify_alpine_archive<R: CommandRunner>(
    runner: &mut R,
    workspace: &Path,
    archive: &Path,
    signature: &Path,
    sha256: &str,
) -> Result<(), HandoffError> {
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(HandoffError::Invalid("invalid expected Alpine SHA256"));
    }
    let key = workspace.join("alpine-release.asc");
    private_file(&key, ALPINE_RELEASE_KEY, 0o600)?;
    let home = workspace.join("gnupg");
    DirBuilder::new().mode(0o700).create(&home)?;
    let base = vec![
        "--no-options".into(),
        "--batch".into(),
        "--no-tty".into(),
        "--homedir".into(),
        home.as_os_str().to_owned(),
        "--no-auto-key-retrieve".into(),
    ];
    let mut inspect = base.clone();
    inspect.extend([
        "--with-colons".into(),
        "--import-options".into(),
        "show-only".into(),
        "--import".into(),
        key.as_os_str().to_owned(),
    ]);
    let public = capture(runner, &spec("gpg", inspect, "handoff-inspect-release-key"))?;
    if !pinned_primary_key(&public) {
        return Err(HandoffError::Invalid(
            "bundled Alpine release key fingerprint mismatch",
        ));
    }
    let mut import = base.clone();
    import.extend(["--import".into(), key.as_os_str().to_owned()]);
    run_checked(runner, &spec("gpg", import, "handoff-import-release-key"))?;
    let mut verify = base;
    verify.extend([
        "--status-fd".into(),
        "1".into(),
        "--verify".into(),
        signature.as_os_str().to_owned(),
        archive.as_os_str().to_owned(),
    ]);
    let status = capture(
        runner,
        &spec("gpg", verify, "handoff-verify-netboot-signature"),
    )?;
    if !valid_alpine_signature(&status) {
        return Err(HandoffError::Invalid(
            "Alpine archive signature was not made by the pinned release key",
        ));
    }
    let digest = capture(
        runner,
        &spec(
            "sha256sum",
            [OsStr::new("--"), archive.as_os_str()],
            "handoff-verify-netboot-sha256",
        ),
    )?;
    if digest
        .split_whitespace()
        .next()
        .map(|value| value.trim_start_matches('\\'))
        .is_none_or(|value| !value.eq_ignore_ascii_case(sha256))
    {
        return Err(HandoffError::Invalid("Alpine archive SHA256 mismatch"));
    }
    Ok(())
}

pub fn pinned_primary_key(colons: &str) -> bool {
    let mut primary = false;
    let mut found = None;
    let mut count = 0;
    for line in colons.lines() {
        let fields: Vec<_> = line.split(':').collect();
        match fields.first().copied() {
            Some("pub") => {
                primary = true;
                count += 1;
            }
            Some("sub") => primary = false,
            Some("fpr") if primary => {
                if found
                    .replace(fields.get(9).copied().unwrap_or_default())
                    .is_some()
                {
                    return false;
                }
                primary = false;
            }
            _ => {}
        }
    }
    count == 1 && found == Some(ALPINE_RELEASE_FINGERPRINT)
}
pub fn valid_alpine_signature(status: &str) -> bool {
    let mut valid = false;
    for line in status.lines() {
        let Some(line) = line.strip_prefix("[GNUPG:] ") else {
            continue;
        };
        let fields: Vec<_> = line.split_whitespace().collect();
        match fields.first().copied() {
            Some("BADSIG" | "ERRSIG" | "EXPSIG" | "EXPKEYSIG" | "REVKEYSIG" | "NO_PUBKEY") => {
                return false;
            }
            Some("VALIDSIG") => {
                if valid
                    || !(fields.get(1) == Some(&ALPINE_RELEASE_FINGERPRINT)
                        || fields.last() == Some(&ALPINE_RELEASE_FINGERPRINT))
                {
                    return false;
                }
                valid = true;
            }
            _ => {}
        }
    }
    valid
}

pub fn resolve_public_keys<R: CommandRunner>(
    runner: &mut R,
    workspace: &Path,
    sources: &[String],
) -> Result<Vec<String>, HandoffError> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, source) in sources.iter().enumerate() {
        let text = if source.trim_start().starts_with("ssh-")
            || source.trim_start().starts_with("ecdsa-")
            || source.trim_start().starts_with("sk-")
        {
            source.clone()
        } else if let Some(user) = source.strip_prefix("github:") {
            source_text(
                runner,
                workspace,
                &format!("key-source-{index}"),
                &account_key_url("https://github.com", user)?,
            )?
        } else if let Some(user) = source.strip_prefix("gitlab:") {
            source_text(
                runner,
                workspace,
                &format!("key-source-{index}"),
                &account_key_url("https://gitlab.com", user)?,
            )?
        } else {
            source_text(runner, workspace, &format!("key-source-{index}"), source)?
        };
        for line in text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let fields: Vec<_> = line.split_whitespace().collect();
            if line.len() > 8192
                || line.chars().any(char::is_control)
                || fields.len() < 2
                || !matches!(
                    fields[0],
                    "ssh-ed25519"
                        | "ssh-rsa"
                        | "ecdsa-sha2-nistp256"
                        | "ecdsa-sha2-nistp384"
                        | "ecdsa-sha2-nistp521"
                        | "sk-ssh-ed25519@openssh.com"
                        | "sk-ecdsa-sha2-nistp256@openssh.com"
                )
                || !fields[1]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte))
            {
                return Err(HandoffError::Invalid(
                    "SSH source did not contain a supported public key",
                ));
            }
            if seen.insert(line.to_owned()) {
                result.push(line.to_owned());
            }
            if result.len() > 128 {
                return Err(HandoffError::Invalid(
                    "SSH sources contain more than 128 unique keys",
                ));
            }
        }
    }
    if !sources.is_empty() && result.is_empty() {
        return Err(HandoffError::Invalid("SSH sources returned no public keys"));
    }
    for (index, key) in result.iter().enumerate() {
        let path = workspace.join(format!("resolved-public-key-{index}"));
        private_file(&path, format!("{key}\n").as_bytes(), 0o600)?;
        run_checked(
            runner,
            &spec(
                "ssh-keygen",
                [OsStr::new("-l"), "-f".as_ref(), path.as_os_str()],
                "handoff-validate-public-key",
            )
            .stdout_mode(OutputMode::Capture)
            .stderr_mode(OutputMode::Capture),
        )?;
    }
    if !result.is_empty() {
        let path = workspace.join("resolved-authorized-keys");
        private_file(&path, format!("{}\n", result.join("\n")).as_bytes(), 0o600)?;
    }
    Ok(result)
}

fn account_key_url(base: &str, user: &str) -> Result<String, HandoffError> {
    if user.is_empty()
        || user.len() > 255
        || !user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(HandoffError::Invalid(
            "invalid SSH public-key account source",
        ));
    }
    Ok(format!("{base}/{user}.keys"))
}
fn source_text<R: CommandRunner>(
    runner: &mut R,
    workspace: &Path,
    name: &str,
    source: &str,
) -> Result<String, HandoffError> {
    let path = if source.starts_with("https://") {
        let path = workspace.join(name);
        download(runner, source, &path, MAX_TEXT as u64)?;
        path
    } else {
        let path = PathBuf::from(source);
        if !path.is_absolute() {
            return Err(HandoffError::Invalid(
                "configuration/public-key files must be absolute paths or HTTPS URLs",
            ));
        }
        path
    };
    let text = read_text(&path)?;
    if text.contains("PRIVATE KEY") || text.starts_with("openssh-key-v1") {
        let mut bytes = text.into_bytes();
        bytes.fill(0);
        return Err(HandoffError::Invalid(
            "private keys must not enter an installation plan",
        ));
    }
    Ok(text)
}

/// A fixed allowlist of inherited native network-client settings. This is
/// private environment data, never a kernel argument or executable hook.
#[derive(Default)]
struct Transport {
    environment: String,
    ca: Option<Vec<u8>>,
}
impl Transport {
    fn from_environment() -> Result<Self, HandoffError> {
        Self::from_settings(optional_environment)
    }
    fn from_settings(
        mut get: impl FnMut(&str) -> Result<Option<String>, HandoffError>,
    ) -> Result<Self, HandoffError> {
        let mut values = Vec::new();
        for (lower, upper) in [
            ("http_proxy", "HTTP_PROXY"),
            ("https_proxy", "HTTPS_PROXY"),
            ("no_proxy", "NO_PROXY"),
        ] {
            let low = get(lower)?;
            let high = get(upper)?;
            if let Some(value) = low.as_ref().or(high.as_ref()) {
                if lower != "no_proxy" {
                    validate_proxy(value)?;
                }
                values.push((lower, value.clone()));
            }
            if let Some(value) = high.as_ref().or(low.as_ref()) {
                if upper != "NO_PROXY" {
                    validate_proxy(value)?;
                }
                values.push((upper, value.clone()));
            }
        }
        let has_proxy = values
            .iter()
            .any(|(name, _)| *name != "no_proxy" && *name != "NO_PROXY");
        if !has_proxy && get("ALL_PROXY")?.or(get("all_proxy")?).is_some() {
            return Err(HandoffError::Invalid(
                "Alpine Live requires an explicit HTTP/HTTPS proxy; ALL_PROXY alone is unsupported",
            ));
        }
        let mut explicit_ca = None;
        for name in ["SSL_CERT_FILE", "SSL_CA_CERT_FILE", "CURL_CA_BUNDLE"] {
            if let Some(path) = get(name)? {
                explicit_ca = Some(PathBuf::from(path));
                break;
            }
        }
        let ca = if explicit_ca.is_some() || has_proxy {
            let path =
                explicit_ca.unwrap_or_else(|| PathBuf::from("/etc/ssl/certs/ca-certificates.crt"));
            if !path.is_absolute() {
                return Err(HandoffError::Invalid(
                    "transport CA must use an absolute certificate-bundle path",
                ));
            }
            let text = read_text(&path)?;
            if !text.contains("-----BEGIN CERTIFICATE-----")
                || !text.contains("-----END CERTIFICATE-----")
                || text.contains("PRIVATE KEY")
            {
                return Err(HandoffError::Invalid(
                    "transport CA input must contain only a public PEM certificate bundle",
                ));
            }
            for name in ["SSL_CERT_FILE", "SSL_CA_CERT_FILE", "CURL_CA_BUNDLE"] {
                values.push((name, "/configs/transport-ca.pem".into()));
            }
            Some(text.into_bytes())
        } else {
            None
        };
        let environment = values
            .into_iter()
            .map(|(name, value)| format!("export {name}={}\n", shell_quote(&value)))
            .collect();
        Ok(Self { environment, ca })
    }
    fn stage(&self, root: &Path) -> Result<(), HandoffError> {
        if self.environment.is_empty() {
            return Ok(());
        }
        let directory = root.join("configs");
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(HandoffError::Invalid(
                    "transport configuration directory is unsafe",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                DirBuilder::new().mode(0o700).create(&directory)?
            }
            Err(error) => return Err(error.into()),
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        private_file(
            &directory.join("transport.env"),
            self.environment.as_bytes(),
            0o600,
        )?;
        if let Some(ca) = &self.ca {
            private_file(&directory.join("transport-ca.pem"), ca, 0o600)?;
        }
        Ok(())
    }
}
fn optional_environment(name: &str) -> Result<Option<String>, HandoffError> {
    let value = match std::env::var(name) {
        Ok(value) if !value.is_empty() => value,
        Ok(_) | Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(_) => {
            return Err(HandoffError::Invalid(
                "transport setting is not valid UTF-8",
            ));
        }
    };
    if value.len() > 8192 || value.chars().any(char::is_control) {
        return Err(HandoffError::Invalid("invalid transport environment value"));
    }
    Ok(Some(value))
}
fn validate_proxy(value: &str) -> Result<(), HandoffError> {
    ProxyEndpoint::parse(value).map(|_|()).map_err(|_|HandoffError::Invalid("transport proxy must identify a reachable HTTP/HTTPS host outside the current loopback stack"))
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn running_kernel_config<R: CommandRunner>(runner: &mut R) -> Result<String, HandoffError> {
    if Path::new("/proc/config.gz").is_file() {
        return capture(
            runner,
            &spec("zcat", ["/proc/config.gz"], "handoff-running-kernel-config"),
        );
    }
    let release = capture(
        runner,
        &spec("uname", ["-r"], "handoff-running-kernel-version"),
    )?;
    if release.trim().is_empty()
        || !release
            .trim()
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
    {
        return Err(HandoffError::Invalid("unsafe running kernel version"));
    }
    read_text(&Path::new("/boot").join(format!("config-{}", release.trim())))
}
fn fetch_text<R: CommandRunner>(runner: &mut R, url: &str) -> Result<String, HandoffError> {
    validate_https(url)?;
    capture(
        runner,
        &spec(
            "curl",
            [
                "--disable",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "15",
                "--max-time",
                "60",
                "--max-filesize",
                "1048576",
                url,
            ],
            "handoff-release-metadata",
        ),
    )
}
fn download<R: CommandRunner>(
    runner: &mut R,
    url: &str,
    path: &Path,
    limit: u64,
) -> Result<(), HandoffError> {
    validate_https(url)?;
    if fs::symlink_metadata(path).is_ok() {
        return Err(HandoffError::Invalid("download destination already exists"));
    }
    let mut args: Vec<_> = [
        "--disable",
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "15",
        "--max-time",
        "1800",
        "--retry",
        "2",
        "--max-filesize",
    ]
    .map(Into::into)
    .to_vec();
    args.extend([
        limit.to_string().into(),
        "--output".into(),
        path.as_os_str().to_owned(),
        "--config".into(),
        "-".into(),
    ]);
    // User-selected key/frpc URLs can carry private path/query tokens. Keep
    // them off process argv as well as redacted diagnostics.
    let configuration = format!(
        "url = \"{}\"\n",
        url.replace('\\', "\\\\").replace('"', "\\\"")
    );
    run_checked(
        runner,
        &spec("curl", args, "handoff-https-download")
            .stdin(CommandInput::Bytes(configuration.into_bytes()))
            .stderr_mode(OutputMode::Capture),
    )?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > limit {
        return Err(HandoffError::Invalid(
            "downloaded payload is empty, oversized or not a regular file",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}
fn validate_https(url: &str) -> Result<(), HandoffError> {
    let Some(value) = url.strip_prefix("https://") else {
        return Err(HandoffError::Invalid("downloads require HTTPS"));
    };
    let host = value.split('/').next().unwrap_or_default();
    if host.is_empty()
        || host.contains('@')
        || url.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(HandoffError::Invalid("unsafe HTTPS URL"));
    }
    Ok(())
}
fn read_text(path: &Path) -> Result<String, HandoffError> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_TEXT as u64 {
        return Err(HandoffError::Invalid(
            "oversized or nonregular configuration input",
        ));
    }
    let mut text = String::new();
    File::open(path)?
        .take((MAX_TEXT + 1) as u64)
        .read_to_string(&mut text)?;
    if text.len() > MAX_TEXT || text.contains('\0') {
        return Err(HandoffError::Invalid("invalid bounded configuration text"));
    }
    Ok(text)
}
fn private_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), HandoffError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(mode)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn validate_payload(path: &Path) -> Result<(), HandoffError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(HandoffError::Invalid(
            "verified payload is missing or not regular",
        ));
    }
    Ok(())
}
fn valid_device(device: &str) -> bool {
    device.strip_prefix("/dev/").is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    })
}
fn validate_new_workspace(path: &Path) -> Result<(), HandoffError> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || fs::symlink_metadata(path).is_ok()
    {
        return Err(HandoffError::Invalid(
            "workspace must be a new absolute directory",
        ));
    }
    let mut prefix = PathBuf::new();
    for part in path
        .parent()
        .ok_or(HandoffError::Invalid("workspace parent missing"))?
        .components()
    {
        prefix.push(part);
        let metadata = fs::symlink_metadata(&prefix)?;
        if !metadata.file_type().is_dir()
            || ![0, effective_uid()].contains(&metadata.uid())
            || (metadata.permissions().mode() & 0o022 != 0
                && metadata.permissions().mode() & 0o1000 == 0)
        {
            return Err(HandoffError::Invalid(
                "workspace ancestors must be trusted real directories",
            ));
        }
    }
    Ok(())
}
fn program_available(program: &str) -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|directory| {
        fs::metadata(directory.join(program))
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
}
fn spec<I, S>(program: &str, args: I, phase: &str) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    CommandSpec::new(program, args)
        .phase(phase)
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream)
}
fn capture<R: CommandRunner>(
    runner: &mut R,
    command: &CommandSpec,
) -> Result<String, HandoffError> {
    let command = command
        .clone()
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Capture);
    let bytes = crate::command::capture_checked(runner, &command)?;
    if bytes.len() > MAX_TEXT {
        return Err(HandoffError::Invalid("oversized command output"));
    }
    String::from_utf8(bytes).map_err(|_| HandoffError::Invalid("command returned invalid UTF-8"))
}
struct CheckedPipelines<'a, R>(&'a mut R);
impl<R: CommandRunner> CommandRunner for CheckedPipelines<'_, R> {
    fn run(&mut self, spec: &CommandSpec) -> io::Result<CommandOutput> {
        self.0.run(spec)
    }
    fn run_pipeline(&mut self, commands: &[CommandSpec]) -> io::Result<PipelineOutput> {
        let output = self.0.run_pipeline(commands)?;
        if output.statuses.len() != commands.len()
            || output.statuses.iter().any(|status| *status != Some(0))
        {
            return Err(io::Error::other("installer archive pipeline failed"));
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::MockRunner;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "rustaller-handoff-internal-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn prepared(&self, init: HostInit) -> PreparedHandoff {
            let kernel = self.0.join("kernel");
            let initrd = self.0.join("private-initrd");
            fs::write(&kernel, b"kernel-fixture").unwrap();
            fs::write(&initrd, b"private-plan-fixture").unwrap();
            PreparedHandoff {
                workspace: self.0.clone(),
                kernel,
                initrd,
                cmdline: "rustaller_live=1 console=ttyS0".into(),
                disk: DiskIdentity {
                    device: "/dev/vda".into(),
                    size_bytes: 8 * 1024_u64.pow(3),
                    serial: "test-disk".into(),
                    wwn: String::new(),
                    partition_table_id: "01234567".into(),
                },
                init,
                boot_mode: BootMode::Bios,
            }
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
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
            let root = Scratch::new();
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
        let root = Scratch::new();
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

    #[test]
    fn preexisting_loaded_image_is_never_replaced_or_unloaded() {
        let scratch = Scratch::new();
        let prepared = scratch.prepared(HostInit::Systemd);
        let mut runner = MockRunner::new([out(0, "1\n")]);
        assert!(
            execute_with_facts(&mut runner, &prepared, 1024_u64.pow(3), HostInit::Systemd).is_err()
        );
        assert_eq!(runner.calls().len(), 1);
        assert_eq!(runner.calls()[0].program, "cat");
    }
    #[test]
    fn live_overlay_world_installs_real_storage_guards_before_worker_start() {
        let mut plan = InstallPlan::new(Target::Alpine);
        let world = live_package_world(&plan);
        for package in [
            "findmnt",
            "lsblk",
            "blkid",
            "mount",
            "umount",
            "util-linux-misc",
        ] {
            assert!(world.lines().any(|line| line == package));
        }
        assert!(
            !world
                .lines()
                .any(|line| line == "util-linux" || line == "sudo")
        );
        plan.username = Some("admin".into());
        assert!(live_package_world(&plan).lines().any(|line| line == "sudo"));
    }
    #[test]
    fn native_kexec_handoff_checks_loaded_state_and_preserves_init_adapter() {
        for init in [HostInit::Systemd, HostInit::OpenRcInit] {
            let scratch = Scratch::new();
            let prepared = scratch.prepared(init);
            let mut runner =
                MockRunner::new([out(0, "0\n"), out(0, ""), out(0, "1\n"), out(0, "")]);
            execute_with_facts(&mut runner, &prepared, 1024_u64.pow(3), init).unwrap();
            assert_eq!(runner.calls().len(), 4);
            assert_eq!(runner.calls()[1].program, "kexec");
            assert_eq!(&runner.calls()[1].args[..2], ["-s", "-l"]);
            assert!(
                runner.calls()[1]
                    .args
                    .iter()
                    .any(|arg| arg == "--append=rustaller_live=1 console=ttyS0")
            );
            let command = &runner.calls()[3];
            match init {
                HostInit::Systemd => {
                    assert_eq!(command.program, "systemctl");
                    assert_eq!(command.args, ["kexec"]);
                }
                HostInit::OpenRcInit => {
                    assert_eq!(command.program, "openrc-shutdown");
                    assert_eq!(command.args, ["--kexec", "now"]);
                }
            }
            assert!(
                !runner
                    .calls()
                    .iter()
                    .any(|call| call.program == "reboot" || call.program == "grub-reboot")
            );
        }
    }
    #[test]
    fn every_failure_after_our_load_attempts_file_syscall_unload() {
        for outputs in [
            vec![out(0, "0\n"), out(0, ""), out(0, "0\n"), out(0, "")],
            vec![out(0, "0\n"), out(0, ""), out(1, ""), out(0, "")],
            vec![
                out(0, "0\n"),
                out(0, ""),
                out(0, "1\n"),
                out(1, ""),
                out(0, ""),
            ],
        ] {
            let scratch = Scratch::new();
            let prepared = scratch.prepared(HostInit::Systemd);
            let mut runner = MockRunner::new(outputs);
            assert!(
                execute_with_facts(&mut runner, &prepared, 1024_u64.pow(3), HostInit::Systemd)
                    .is_err()
            );
            let last = runner.calls().last().unwrap();
            assert_eq!(last.program, "kexec");
            assert_eq!(last.args, ["-s", "-u"]);
        }
    }
    #[test]
    fn load_failure_does_not_unload_or_request_shutdown_and_cleanup_failure_is_explicit() {
        let scratch = Scratch::new();
        let prepared = scratch.prepared(HostInit::Systemd);
        let mut runner = MockRunner::new([out(0, "0\n"), out(1, "")]);
        assert!(
            execute_with_facts(&mut runner, &prepared, 1024_u64.pow(3), HostInit::Systemd).is_err()
        );
        assert_eq!(runner.calls().len(), 2);
        let mut runner = MockRunner::new([
            out(0, "0\n"),
            out(0, ""),
            out(0, "1\n"),
            out(1, ""),
            out(1, ""),
        ]);
        let error = execute_with_facts(&mut runner, &prepared, 1024_u64.pow(3), HostInit::Systemd)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("could not be unloaded"));
        let mut empty = MockRunner::empty();
        assert!(execute_with_facts(&mut empty, &prepared, 1, HostInit::Systemd).is_err());
        assert!(
            execute_with_facts(&mut empty, &prepared, 1024_u64.pow(3), HostInit::OpenRcInit)
                .is_err()
        );
        assert!(empty.calls().is_empty());
    }
    #[test]
    fn archive_wrapper_rejects_failure_in_any_pipeline_stage() {
        let mut runner =
            MockRunner::new([out(9, ""), out(0, ""), out(0, "compressed-but-invalid")]);
        let mut checked = CheckedPipelines(&mut runner);
        let commands = [
            spec("find", ["."], "test-only"),
            spec("cpio", ["-o"], "test-only"),
            spec("gzip", ["-c"], "test-only"),
        ];
        assert!(checked.run_pipeline(&commands).is_err());
    }
    #[test]
    fn private_transport_is_allowlisted_quoted_and_staged_without_changing_command_line() {
        let scratch = Scratch::new();
        let ca = scratch.0.join("public-ca.pem");
        fs::write(
            &ca,
            "-----BEGIN CERTIFICATE-----\npublic-test-fixture\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let settings = BTreeMap::from([
            (
                "HTTPS_PROXY",
                "http://user:pa'ss@proxy.example:8080".to_owned(),
            ),
            ("SSL_CERT_FILE", ca.to_string_lossy().into_owned()),
            ("NO_PROXY", "localhost,127.0.0.1".to_owned()),
        ]);
        let transport = Transport::from_settings(|name| Ok(settings.get(name).cloned())).unwrap();
        assert!(
            transport
                .environment
                .contains("export HTTPS_PROXY='http://user:pa'\\''ss@proxy.example:8080'")
        );
        assert!(transport.environment.contains("export https_proxy="));
        assert!(
            transport
                .environment
                .contains("export SSL_CA_CERT_FILE='/configs/transport-ca.pem'")
        );
        assert!(!transport.environment.contains("PATH="));
        let root = scratch.0.join("staged");
        fs::create_dir(&root).unwrap();
        transport.stage(&root).unwrap();
        assert_eq!(
            fs::metadata(root.join("configs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for name in ["transport.env", "transport-ca.pem"] {
            assert_eq!(
                fs::metadata(root.join("configs").join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let other = scratch.0.join("unsafe-root");
        fs::create_dir(&other).unwrap();
        symlink(&root, other.join("configs")).unwrap();
        assert!(transport.stage(&other).is_err());
        assert!(
            Transport::from_settings(|name| Ok(
                (name == "ALL_PROXY").then(|| "socks5://proxy.example:1080".into())
            ))
            .is_err()
        );
    }
    #[test]
    fn downloaded_source_tokens_are_stdin_configuration_and_not_process_arguments() {
        struct FileDownload {
            output: PathBuf,
            call: Option<CommandSpec>,
        }
        impl CommandRunner for FileDownload {
            fn run(&mut self, spec: &CommandSpec) -> io::Result<CommandOutput> {
                self.call = Some(spec.clone());
                fs::write(&self.output, b"test-payload")?;
                Ok(out(0, ""))
            }
            fn run_pipeline(&mut self, _: &[CommandSpec]) -> io::Result<PipelineOutput> {
                panic!("no download pipeline")
            }
        }
        let scratch = Scratch::new();
        let path = scratch.0.join("download");
        let mut runner = FileDownload {
            output: path.clone(),
            call: None,
        };
        download(
            &mut runner,
            "https://keys.example/PRIVATE-URL-SENTINEL?token=secret",
            &path,
            1024,
        )
        .unwrap();
        let command = runner.call.unwrap();
        assert_eq!(command.program, "curl");
        assert!(
            !command
                .args
                .iter()
                .any(|arg| arg.to_string_lossy().contains("PRIVATE-URL-SENTINEL"))
        );
        let CommandInput::Bytes(input) = &command.stdin else {
            panic!("private curl config")
        };
        assert!(String::from_utf8_lossy(input).contains("PRIVATE-URL-SENTINEL"));
        assert!(!format!("{command:?}").contains("PRIVATE-URL-SENTINEL"));
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
