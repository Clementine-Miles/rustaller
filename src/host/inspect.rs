//! Read-only inspection shared by CLI and TUI; renderers never probe the host.
use crate::{
    command::{CommandRunner, CommandSpec, capture_checked},
    deployment::DeploymentManifest,
    model::{BootMode, InstallPlan},
};
use std::{collections::BTreeMap, fs, io::Read, path::Path};
#[path = "inspect_cpu.rs"]
pub mod cpu;
#[derive(Clone, Default)]
pub struct PreflightReport {
    pub current: String,
    pub target: String,
    pub warning: String,
    /// Diagnostic fallback: installation preflight did not authorize continuation.
    pub blocked: bool,
}
impl From<String> for PreflightReport {
    fn from(current: String) -> Self {
        Self {
            current,
            target: "Planned configuration unavailable".into(),
            warning: String::new(),
            blocked: false,
        }
    }
}
impl PreflightReport {
    pub fn text(&self) -> String {
        format!(
            "CURRENT SYSTEM — detected\n{}\nTARGET SYSTEM — planned\n{}\n{}",
            self.current, self.target, self.warning
        )
    }
}
const UNKNOWN: &str = "Unknown";
const MAX_TEXT: u64 = 1024 * 1024;
const MAX_DEVICES: usize = 256;

// Never source a host configuration file or execute a downloaded collector.
fn read(root: &Path, name: &str) -> String {
    fs::File::open(root.join(name))
        .ok()
        .and_then(|f| {
            let mut text = String::new();
            f.take(MAX_TEXT + 1).read_to_string(&mut text).ok()?;
            (text.len() as u64 <= MAX_TEXT).then(|| clean(&text))
        })
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| UNKNOWN.into())
}
fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}
fn line(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(512)
        .collect::<String>()
        .trim()
        .to_owned()
}
fn fact(runner: &mut impl CommandRunner, program: &str, args: &[&str]) -> String {
    capture_checked(
        runner,
        &CommandSpec::new(program, args)
            .phase("host-inspection")
            .env("LC_ALL", "C")
            .inspection_limits(),
    )
    .ok()
    .filter(|v| v.len() <= 65536)
    .and_then(|v| String::from_utf8(v).ok())
    .map(|s| clean(s.trim()))
    .filter(|v| !v.is_empty())
    .unwrap_or_else(|| UNKNOWN.into())
}
fn names(root: &Path, name: &str) -> Vec<String> {
    let mut result = fs::read_dir(root.join(name))
        .ok()
        .into_iter()
        .flatten()
        .take(MAX_DEVICES)
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|s| safe_name(s))
        .collect::<Vec<_>>();
    result.sort();
    result
}
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
fn number(root: &Path, name: &str) -> Option<u64> {
    read(root, name).trim().parse().ok()
}
fn display_bytes(value: Option<u64>) -> String {
    value
        .map(|n| format!("{:.1} MiB", n as f64 / 1048576.0))
        .unwrap_or_else(|| UNKNOWN.into())
}
fn nonempty(value: &str) -> &str {
    if value.is_empty() { UNKNOWN } else { value }
}
fn property(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|v| {
        v.split_once('=')
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| line(v.trim_matches(['"', '\''])))
    })
}

#[derive(Clone, Debug)]
pub struct SystemFacts {
    pub distribution: String,
    pub kernel: String,
    pub architecture: String,
    pub hostname: String,
    pub uptime_seconds: Option<u64>,
    pub pid1: String,
    pub libc: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryFacts {
    pub total: Option<u64>,
    pub available: Option<u64>,
    /// Total minus MemAvailable, not the misleading total-minus-MemFree.
    pub used: Option<u64>,
    pub swap_total: Option<u64>,
    pub swap_used: Option<u64>,
    pub zram: Vec<ZramFacts>,
    pub balloon_drivers: Vec<String>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ZramFacts {
    pub name: String,
    pub capacity: Option<u64>,
    pub compressed_bytes: Option<u64>,
    pub memory_bytes: Option<u64>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountFacts {
    pub path: String,
    pub source: String,
    pub filesystem: String,
    pub options: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiskFacts {
    pub name: String,
    pub kind: String,
    pub bytes: Option<u64>,
    pub model: String,
    pub identity: String,
    pub transport: String,
    pub logical_sector: Option<u64>,
    pub physical_sector: Option<u64>,
    pub partition_table: String,
    pub filesystem: String,
    pub parent: String,
    pub rotational: Option<bool>,
    pub discard_max: Option<u64>,
    pub slaves: Vec<String>,
    pub holders: Vec<String>,
}
#[derive(Clone, Debug, Default)]
pub struct StorageFacts {
    pub devices: Vec<DiskFacts>,
    pub mounts: Vec<MountFacts>,
    pub utilization: String,
    pub raid: String,
}
impl StorageFacts {
    pub fn mounted_partition(&self, path: &str) -> Option<&DiskFacts> {
        let src = self
            .mounts
            .iter()
            .find(|m| m.path == path)?
            .source
            .strip_prefix("/dev/")?;
        self.devices
            .iter()
            .find(|d| d.kind == "part" && d.name == src)
    }

    /// Return only directly mounted partitions on the same whole disk. Device
    /// mapper, RAID and ambiguous bind mounts cannot provide reusable geometry.
    pub fn partition_sizes(&self, selected: &str) -> Option<(&str, u64, u64)> {
        let part = |path: &str| {
            let mount = self.mounts.iter().find(|m| m.path == path)?;
            if path != "/" && !matches!(mount.filesystem.as_str(), "vfat" | "fat" | "fat32") {
                return None;
            }
            self.mounted_partition(path)
        };
        let root = part("/")?;
        let boot = ["/boot", "/boot/efi", "/efi"].into_iter().find_map(part)?;
        if root.name == boot.name
            || root.parent.is_empty()
            || root.parent != boot.parent
            || (!selected.is_empty() && selected.trim_start_matches("/dev/") != root.parent)
        {
            return None;
        }
        Some((&root.parent, boot.bytes?, root.bytes?))
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InterfaceFacts {
    pub name: String,
    pub mac: String,
    pub addresses: Vec<String>,
    pub mtu: Option<u64>,
    pub state: String,
    pub speed_mbps: Option<u64>,
    pub policy: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteFacts {
    pub ipv6: bool,
    pub interface: String,
    pub gateway: String,
    pub metric: Option<u64>,
}
#[derive(Clone, Debug, Default)]
pub struct NetworkFacts {
    pub interfaces: Vec<InterfaceFacts>,
    pub defaults: Vec<RouteFacts>,
    pub dns: Vec<String>,
    pub ssh_policy: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct FirmwareFacts {
    pub mode: String,
    pub bitness: String,
    pub secure_boot: String,
    pub vendor: String,
}
#[derive(Clone, Debug)]
pub struct VirtualizationFacts {
    pub environment: String,
    pub container: bool,
    pub manufacturer: String,
    pub product: String,
    pub compatibility: String,
}
/// Optional observations are independent of the strict installation manifest.
#[derive(Clone, Debug)]
pub struct HostInspection {
    pub system: SystemFacts,
    pub cpu: cpu::CpuFacts,
    pub virtualization: VirtualizationFacts,
    pub memory: MemoryFacts,
    pub storage: StorageFacts,
    pub firmware: FirmwareFacts,
    pub network: NetworkFacts,
}

fn memory(root: &Path) -> MemoryFacts {
    let fields = read(root, "proc/meminfo")
        .lines()
        .filter_map(|v| {
            let (name, value) = v.split_once(':')?;
            let mut parts = value.split_whitespace();
            let kb = parts.next()?.parse::<u64>().ok()?;
            (parts.next() == Some("kB") && parts.next().is_none())
                .then(|| kb.checked_mul(1024).map(|n| (name.to_owned(), n)))
                .flatten()
        })
        .collect::<BTreeMap<_, _>>();
    let total = fields.get("MemTotal").copied();
    let available = fields
        .get("MemAvailable")
        .copied()
        .filter(|n| total.is_none_or(|t| *n <= t));
    let swap_total = fields.get("SwapTotal").copied();
    let mut result = MemoryFacts {
        total,
        available,
        used: total.zip(available).map(|(t, a)| t - a),
        swap_total,
        swap_used: swap_total
            .zip(fields.get("SwapFree").copied())
            .and_then(|(t, f)| t.checked_sub(f)),
        ..MemoryFacts::default()
    };
    for name in names(root, "sys/block")
        .into_iter()
        .filter(|n| n.starts_with("zram"))
    {
        let stats = read(root, &format!("sys/block/{name}/mm_stat"));
        let stats = stats
            .split_whitespace()
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
            .ok();
        result.zram.push(ZramFacts {
            capacity: number(root, &format!("sys/block/{name}/disksize")),
            compressed_bytes: stats.as_ref().and_then(|s| s.get(1).copied()),
            memory_bytes: stats.as_ref().and_then(|s| s.get(2).copied()),
            name,
        });
    }
    result.balloon_drivers = ["virtio_balloon", "xen_balloon", "hv_balloon", "vmw_balloon"]
        .into_iter()
        .filter(|n| root.join(format!("sys/module/{n}")).is_dir())
        .map(str::to_owned)
        .collect();
    result
}
fn mount_options(value: &str) -> String {
    let filtered = value
        .split(',')
        .take(128)
        .map(|option| {
            let key = option
                .split('=')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if [
                "password",
                "passwd",
                "pass",
                "username",
                "user",
                "credentials",
                "cred",
                "secret",
                "token",
                "key",
                "keyfile",
            ]
            .iter()
            .any(|s| key == *s || key.ends_with(&format!("_{s}")))
            {
                format!("{key}=[redacted]")
            } else {
                let cleaned = line(option);
                if cleaned.chars().count() > 160 {
                    format!("{}…", cleaned.chars().take(160).collect::<String>())
                } else {
                    cleaned
                }
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    if filtered.chars().count() > 1024 {
        format!("{}…", filtered.chars().take(1024).collect::<String>())
    } else {
        filtered
    }
}
fn mounts(text: &str) -> Vec<MountFacts> {
    text.lines()
        .take(1024)
        .filter_map(|v| {
            let (before, after) = v.split_once(" - ")?;
            let b = before.split_whitespace().collect::<Vec<_>>();
            let a = after.split_whitespace().collect::<Vec<_>>();
            let path = *b.get(4)?;
            if !matches!(path, "/" | "/boot" | "/boot/efi" | "/efi") {
                return None;
            }
            Some(MountFacts {
                path: path.into(),
                source: line(a.get(1)?),
                filesystem: line(a.first()?),
                options: format!("{}; {}", mount_options(b.get(5)?), mount_options(a.get(2)?)),
            })
        })
        .collect()
}
// lsblk -P uses quoted fields and hexadecimal escaping, never shell parsing.
fn pairs(text: &str) -> Option<BTreeMap<String, String>> {
    let mut remaining = text.trim();
    let mut result = BTreeMap::new();
    while !remaining.is_empty() {
        let (key, rest) = remaining.split_once("=\"")?;
        if !key
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b"_-".contains(&b))
        {
            return None;
        }
        let (value, rest) = rest.split_once('"')?;
        let mut decoded = Vec::new();
        let mut bytes = value.as_bytes().iter().copied();
        while let Some(b) = bytes.next() {
            if b == b'\\' {
                if bytes.next()? != b'x' {
                    return None;
                }
                let hi = (bytes.next()? as char).to_digit(16)?;
                let lo = (bytes.next()? as char).to_digit(16)?;
                decoded.push((hi * 16 + lo) as u8);
            } else {
                decoded.push(b);
            }
        }
        if result.len() >= 32
            || result
                .insert(key.into(), line(&String::from_utf8(decoded).ok()?))
                .is_some()
        {
            return None;
        }
        remaining = rest.trim_start();
    }
    Some(result)
}
pub fn storage(runner: &mut impl CommandRunner, root: &Path) -> StorageFacts {
    let listing = fact(
        runner,
        "lsblk",
        &[
            "--bytes",
            "--pairs",
            "--output",
            "KNAME,TYPE,SIZE,MODEL,SERIAL,WWN,TRAN,LOG-SEC,PHY-SEC,PTTYPE,FSTYPE,UUID,PKNAME,ROTA,DISC-MAX",
        ],
    );
    let mut devices = Vec::new();
    for values in listing.lines().take(MAX_DEVICES).filter_map(pairs) {
        let get = |name: &str| values.get(name).cloned().unwrap_or_default();
        let name = get("KNAME");
        if !safe_name(&name) {
            continue;
        }
        let num = |name: &str| values.get(name).and_then(|n| n.parse::<u64>().ok());
        devices.push(DiskFacts {
            name: name.clone(),
            kind: get("TYPE"),
            bytes: num("SIZE"),
            model: get("MODEL"),
            identity: [get("SERIAL"), get("WWN"), get("UUID")]
                .into_iter()
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join(" / "),
            transport: get("TRAN"),
            logical_sector: num("LOG-SEC"),
            physical_sector: num("PHY-SEC"),
            partition_table: get("PTTYPE"),
            filesystem: get("FSTYPE"),
            parent: get("PKNAME"),
            rotational: num("ROTA").filter(|n| *n <= 1).map(|n| n == 1),
            discard_max: num("DISC-MAX"),
            slaves: names(root, &format!("sys/class/block/{name}/slaves")),
            holders: names(root, &format!("sys/class/block/{name}/holders")),
        });
    }
    if devices.is_empty() {
        for name in names(root, "sys/class/block") {
            let prefix = format!("sys/class/block/{name}");
            devices.push(DiskFacts {
                name: name.clone(),
                kind: if root.join(format!("{prefix}/partition")).exists() {
                    "partition"
                } else {
                    "block"
                }
                .into(),
                bytes: number(root, &format!("{prefix}/size")).and_then(|n| n.checked_mul(512)),
                model: line(&read(root, &format!("{prefix}/device/model"))),
                identity: line(&read(root, &format!("{prefix}/device/serial"))),
                logical_sector: number(root, &format!("{prefix}/queue/logical_block_size")),
                physical_sector: number(root, &format!("{prefix}/queue/physical_block_size")),
                rotational: number(root, &format!("{prefix}/queue/rotational"))
                    .filter(|n| *n <= 1)
                    .map(|n| n == 1),
                discard_max: number(root, &format!("{prefix}/queue/discard_max_bytes")),
                slaves: names(root, &format!("{prefix}/slaves")),
                holders: names(root, &format!("{prefix}/holders")),
                ..DiskFacts::default()
            });
        }
    }
    StorageFacts {
        devices,
        mounts: mounts(&read(root, "proc/self/mountinfo")),
        utilization: fact(runner, "df", &["-P", "-B1", "/", "/boot"]),
        raid: read(root, "proc/mdstat")
            .lines()
            .filter(|v| !v.starts_with("Personalities") && !v.starts_with("unused devices"))
            .take(16)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}
fn routes(text: &str, ipv6: bool) -> Vec<RouteFacts> {
    text.lines()
        .take(64)
        .filter_map(|v| {
            let words = v.split_whitespace().collect::<Vec<_>>();
            if words.first() != Some(&"default") {
                return None;
            }
            let arg = |key| {
                words
                    .iter()
                    .position(|s| *s == key)
                    .and_then(|i| words.get(i + 1).copied())
            };
            let interface = arg("dev")?;
            if !safe_name(interface) {
                return None;
            }
            let gateway = match arg("via") {
                Some(ip) => {
                    let address = ip.parse::<std::net::IpAddr>().ok()?;
                    if address.is_ipv6() != ipv6 {
                        return None;
                    }
                    ip
                }
                None => "on-link",
            };
            Some(RouteFacts {
                ipv6,
                interface: interface.into(),
                gateway: gateway.into(),
                metric: arg("metric").and_then(|n| n.parse().ok()),
            })
        })
        .collect()
}
fn network(runner: &mut impl CommandRunner, root: &Path) -> NetworkFacts {
    let listing = fact(runner, "ip", &["-o", "address", "show"]);
    let mut interface_names = names(root, "sys/class/net");
    let mut addresses: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for v in listing.lines().take(1024) {
        let words = v.split_whitespace().collect::<Vec<_>>();
        let Some(name) = words.get(1).and_then(|s| s.split('@').next()) else {
            continue;
        };
        if !safe_name(name) {
            continue;
        }
        let Some(i) = words.iter().position(|w| matches!(*w, "inet" | "inet6")) else {
            continue;
        };
        let Some(address) = words.get(i + 1) else {
            continue;
        };
        let Some((ip, prefix)) = address.split_once('/') else {
            continue;
        };
        let Ok(ip) = ip.parse::<std::net::IpAddr>() else {
            continue;
        };
        let Ok(prefix) = prefix.parse::<u8>() else {
            continue;
        };
        if prefix > if ip.is_ipv6() { 128 } else { 32 } || ip.is_ipv6() != (words[i] == "inet6") {
            continue;
        }
        addresses
            .entry(name.into())
            .or_default()
            .push(format!("{ip}/{prefix}"));
        if !interface_names.iter().any(|n| n == name) && interface_names.len() < MAX_DEVICES {
            interface_names.push(name.into());
        }
    }
    let interfaces = interface_names
        .into_iter()
        .map(|name| {
            let base = format!("sys/class/net/{name}");
            InterfaceFacts {
                mac: line(&read(root, &format!("{base}/address"))),
                addresses: addresses.remove(&name).unwrap_or_default(),
                mtu: number(root, &format!("{base}/mtu")),
                state: line(&read(root, &format!("{base}/operstate"))),
                speed_mbps: number(root, &format!("{base}/speed")),
                policy: "Unknown (not inferred from address)".into(),
                name,
            }
        })
        .collect();
    let mut defaults = routes(
        &fact(runner, "ip", &["-o", "-4", "route", "show", "default"]),
        false,
    );
    defaults.extend(routes(
        &fact(runner, "ip", &["-o", "-6", "route", "show", "default"]),
        true,
    ));
    let dns = read(root, "etc/resolv.conf")
        .lines()
        .filter(|v| {
            matches!(
                v.split_whitespace().next(),
                Some("nameserver" | "search" | "domain")
            )
        })
        .map(line)
        .take(32)
        .collect();
    let ssh_policy = fact(runner, "sshd", &["-T"])
        .lines()
        .filter(|v| {
            [
                "port ",
                "passwordauthentication ",
                "pubkeyauthentication ",
                "permitrootlogin ",
            ]
            .iter()
            .any(|prefix| v.starts_with(prefix))
        })
        .map(line)
        .take(8)
        .collect();
    NetworkFacts {
        interfaces,
        defaults,
        dns,
        ssh_policy,
    }
}
fn firmware(root: &Path, container: bool) -> FirmwareFacts {
    let efi = fs::symlink_metadata(root.join("sys/firmware/efi"));
    // Containers may expose an empty or filtered sysfs firmware directory.
    // Its missing EFI entry cannot establish the physical host boot mode.
    let mode = if container || !root.join("sys/firmware").is_dir() {
        UNKNOWN
    } else {
        match efi {
            Ok(ref m) if m.is_dir() => "UEFI",
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => "BIOS",
            _ => UNKNOWN,
        }
    };
    let bitness = if mode == "UEFI" {
        line(&read(root, "sys/firmware/efi/fw_platform_size"))
    } else if mode == "BIOS" {
        "Not applicable".into()
    } else {
        UNKNOWN.into()
    };
    let secure_boot = if mode != "UEFI" {
        if mode == "BIOS" {
            "Not applicable"
        } else {
            UNKNOWN
        }
        .into()
    } else {
        let path =
            root.join("sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c");
        let mut bytes = [0u8; 6];
        match fs::File::open(path).and_then(|mut f| f.read(&mut bytes)) {
            Ok(5) if bytes[4] == 0 => "Disabled".into(),
            Ok(5) if bytes[4] == 1 => "Enabled".into(),
            _ => UNKNOWN.into(),
        }
    };
    FirmwareFacts {
        mode: mode.into(),
        bitness,
        secure_boot,
        vendor: line(&read(root, "sys/class/dmi/id/bios_vendor")),
    }
}
fn virtualization(root: &Path, detected: &str) -> VirtualizationFacts {
    let manufacturer = line(&read(root, "sys/class/dmi/id/sys_vendor"));
    let product = line(&read(root, "sys/class/dmi/id/product_name"));
    let dmi = format!("{manufacturer} {product}").to_lowercase();
    let cgroup = read(root, "proc/1/cgroup");
    let container = root.join(".dockerenv").exists()
        || root.join("run/.containerenv").exists()
        || root.join("run/systemd/container").exists()
        || (root.join("proc/vz").exists() && !root.join("proc/bc").exists())
        || cgroup.contains("docker")
        || cgroup.contains("kubepods")
        || cgroup.contains("lxc")
        || matches!(
            detected,
            "docker" | "podman" | "lxc" | "lxc-libvirt" | "openvz" | "systemd-nspawn"
        );
    let environment = if detected == "openvz"
        || (root.join("proc/vz").exists() && !root.join("proc/bc").exists())
    {
        "OpenVZ (unsupported)".into()
    } else if container {
        "Container (unsupported)".into()
    } else if read(root, "sys/hypervisor/type").trim() == "xen" || root.join("proc/xen").exists() {
        format!(
            "Xen {}; guest type from sysfs",
            line(&read(root, "sys/hypervisor/guest_type"))
        )
    } else {
        match detected {
            "kvm" => "KVM/QEMU".into(),
            "qemu" => "QEMU".into(),
            "vmware" => "VMware".into(),
            "microsoft" => "Microsoft Hyper-V".into(),
            "oracle" => "VirtualBox".into(),
            "xen" => "Xen (guest type unavailable)".into(),
            "docker" | "podman" | "lxc" | "lxc-libvirt" | "openvz" | "systemd-nspawn" => {
                format!("{detected} (unsupported)")
            }
            "none" => "Physical / no virtualization detected".into(),
            _ if dmi.contains("kvm") || dmi.contains("qemu") => "KVM/QEMU (DMI indicator)".into(),
            _ if dmi.contains("vmware") => "VMware (DMI indicator)".into(),
            _ if dmi.contains("virtualbox") || dmi.contains("innotek") => {
                "VirtualBox (DMI indicator)".into()
            }
            _ if dmi.contains("microsoft") && dmi.contains("virtual machine") => {
                "Microsoft Hyper-V (DMI indicator)".into()
            }
            _ => "Unknown (physical host cannot be verified)".into(),
        }
    };
    let compatibility = crate::host::preflight::validate_virtualization(root)
        .map(|_| {
            "No unsupported virtualization indicators; firmware/disk checks remain required".into()
        })
        .unwrap_or_else(str::to_owned);
    VirtualizationFacts {
        environment,
        container,
        manufacturer,
        product,
        compatibility,
    }
}

/// Bounded local observations; no package installs, privilege escalation, benchmarks,
/// remote APIs, or authentication files. Called outside the TUI render loop.
pub fn collect(runner: &mut impl CommandRunner, root: &Path) -> HostInspection {
    let os = read(root, "etc/os-release");
    let mut libc = fact(runner, "getconf", &["GNU_LIBC_VERSION"]);
    if libc == UNKNOWN {
        let version = fact(runner, "ldd", &["--version"]);
        // Only version output, never ldd on an executable or shared library.
        libc = version
            .lines()
            .take(3)
            .filter(|s| {
                s.to_lowercase().contains("musl")
                    || s.contains("Version")
                    || s.contains("GLIBC")
                    || s.contains("GNU libc")
            })
            .map(line)
            .collect::<Vec<_>>()
            .join("; ");
        if libc.is_empty() {
            libc = UNKNOWN.into();
        }
    }
    let detected = runner
        .run(
            &CommandSpec::new("systemd-detect-virt", [] as [&str; 0])
                .phase("host-inspection")
                .env("LC_ALL", "C")
                .inspection_limits(),
        )
        .ok()
        .filter(|o| {
            o.stdout.len() <= 65536
                && (o.status == Some(0) || (o.status == Some(1) && o.stdout == b"none\n"))
        })
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| line(&s))
        .unwrap_or_else(|| UNKNOWN.into());
    let uptime_seconds = read(root, "proc/uptime")
        .split_whitespace()
        .next()
        .and_then(|n| n.split('.').next())
        .and_then(|n| n.parse().ok());
    let virtualization = virtualization(root, &detected);
    let firmware = firmware(root, virtualization.container);
    HostInspection {
        system: SystemFacts {
            distribution: property(&os, "PRETTY_NAME")
                .or_else(|| property(&os, "NAME"))
                .unwrap_or_else(|| UNKNOWN.into()),
            kernel: line(&read(root, "proc/sys/kernel/osrelease")),
            architecture: std::env::consts::ARCH.into(),
            hostname: line(&read(root, "proc/sys/kernel/hostname")),
            uptime_seconds,
            pid1: line(&read(root, "proc/1/comm")),
            libc,
        },
        cpu: cpu::inspect(root),
        virtualization,
        memory: memory(root),
        storage: storage(runner, root),
        firmware,
        network: network(runner, root),
    }
}
impl HostInspection {
    pub fn render_current(&self) -> String {
        let s = &self.system;
        let mut result = format!(
            "[System]\nOS: {}\nKernel: {}\nCPU Architecture: {}\nHostname: {}\nUptime: {}\nInit / PID 1: {}\nlibc: {}\n\n[CPU and ISA]\n{}\n[Virtualization]\nEnvironment: {}\nDMI manufacturer/product: {} / {}\nPhysical host CPU: Unverifiable; capabilities are guest-visible\n\n[Memory]\nRAM total / available / used: {} / {} / {}\nSwap total / used: {} / {}\nBalloon indicators: {}\n",
            s.distribution,
            s.kernel,
            s.architecture,
            s.hostname,
            s.uptime_seconds
                .map(|v| format!(
                    "{}d {}h {}m",
                    v / 86400,
                    (v % 86400) / 3600,
                    (v % 3600) / 60
                ))
                .unwrap_or_else(|| UNKNOWN.into()),
            s.pid1,
            s.libc,
            self.cpu.render(),
            self.virtualization.environment,
            self.virtualization.manufacturer,
            self.virtualization.product,
            display_bytes(self.memory.total),
            display_bytes(self.memory.available),
            display_bytes(self.memory.used),
            display_bytes(self.memory.swap_total),
            display_bytes(self.memory.swap_used),
            if self.memory.balloon_drivers.is_empty() {
                "No loaded known balloon driver detected (availability unknown)".into()
            } else {
                format!(
                    "{} loaded; active reclamation unknown",
                    self.memory.balloon_drivers.join(", ")
                )
            }
        );
        if self.memory.zram.is_empty() {
            result.push_str("zram: No devices detected\n");
        }
        for z in &self.memory.zram {
            result.push_str(&format!(
                "zram {}: capacity {}; compressed {}; RAM {}\n",
                z.name,
                display_bytes(z.capacity),
                display_bytes(z.compressed_bytes),
                display_bytes(z.memory_bytes)
            ));
        }
        result.push_str("\n[Storage]\n");
        if self.storage.devices.is_empty() {
            result.push_str("Device inventory: Unavailable\n");
        }
        for d in &self.storage.devices {
            result.push_str(&format!("/dev/{} ({}): {}; model {}\nIdentity: {}; transport {}; parent {}\nSectors logical/physical: {}/{}; table {}; FS {}\nRotational: {}; discard max: {} bytes\n",
                d.name, nonempty(&d.kind),display_bytes(d.bytes),nonempty(&d.model),nonempty(&d.identity),nonempty(&d.transport),nonempty(&d.parent),
                d.logical_sector.map(|v|v.to_string()).unwrap_or_else(|| UNKNOWN.into()),d.physical_sector.map(|v|v.to_string()).unwrap_or_else(|| UNKNOWN.into()),nonempty(&d.partition_table),nonempty(&d.filesystem),
                d.rotational.map(|v| if v {"yes"} else {"no"}).unwrap_or(UNKNOWN),d.discard_max.map(|v|v.to_string()).unwrap_or_else(|| UNKNOWN.into())));
            if !d.slaves.is_empty() || !d.holders.is_empty() {
                result.push_str(&format!(
                    "Relationships: slaves {:?}; holders {:?}\n",
                    d.slaves, d.holders
                ));
            }
        }
        if self.storage.mounts.is_empty() {
            result.push_str("Current root/boot mounts: Unavailable\n");
        }
        for m in &self.storage.mounts {
            result.push_str(&format!(
                "Mount {}: {} {}; options {}\n",
                m.path, m.source, m.filesystem, m.options
            ));
        }
        result.push_str(&format!("Disk utilization (bytes):\n{}\nRAID status:\n{}\n\n[Firmware]\nBoot mode: {}\nUEFI bitness: {}\nSecure Boot: {}\nFirmware vendor: {}\nCurrent bootloader: Unknown (not inferred from files)\n\n[Network]\n",
            self.storage.utilization,nonempty(&self.storage.raid),self.firmware.mode,self.firmware.bitness,self.firmware.secure_boot,self.firmware.vendor));
        if self.network.interfaces.is_empty() {
            result.push_str("Interfaces: Unavailable\n");
        }
        for n in &self.network.interfaces {
            result.push_str(&format!(
                "{}: MAC {}; MTU {}; state {}; speed {}\nAddresses: {}\nAddress policy: {}\n",
                n.name,
                n.mac,
                n.mtu
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| UNKNOWN.into()),
                n.state,
                n.speed_mbps
                    .map(|v| format!("{v} Mbit/s"))
                    .unwrap_or_else(|| "Unavailable".into()),
                if n.addresses.is_empty() {
                    UNKNOWN.into()
                } else {
                    n.addresses.join(", ")
                },
                n.policy
            ));
        }
        if self.network.defaults.is_empty() {
            result.push_str("Default routes / interfaces: Unavailable\n");
        }
        for r in &self.network.defaults {
            result.push_str(&format!(
                "Default IPv{}: via {} dev {} metric {}\n",
                if r.ipv6 { 6 } else { 4 },
                r.gateway,
                r.interface,
                r.metric
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| UNKNOWN.into())
            ));
        }
        result.push_str(&format!("DNS: {}\nSSH policy: {}\n\n[Installation compatibility]\n{}\nDiagnostics do not authorize installation; strict preflight and disk identity checks remain required.\n",
            if self.network.dns.is_empty() { UNKNOWN.into() } else { self.network.dns.join("; ") }, if self.network.ssh_policy.is_empty() { UNKNOWN.into() } else { self.network.ssh_policy.join("; ") },self.virtualization.compatibility));
        result
    }
}
pub fn report(
    runner: &mut impl CommandRunner,
    root: &Path,
    plan: &InstallPlan,
    manifest: &DeploymentManifest,
) -> PreflightReport {
    let mut observed = collect(runner, root);
    for nic in &manifest.network {
        if let Some(interface) = observed
            .network
            .interfaces
            .iter_mut()
            .find(|n| n.name == nic.name)
        {
            interface.policy = format!(
                "validated source policy: DHCPv4={}; DHCPv6={}; RA={}",
                nic.dhcp4, nic.dhcp6, nic.accept_ra
            );
        }
    }
    let mut current = observed.render_current();
    current.push_str(&format!("Selected disk (recorded preflight identity): {} ({} bytes)\nSerial: {} WWN: {} PTUUID: {}\n",manifest.disk.device,manifest.disk.size_bytes,manifest.disk.serial,manifest.disk.wwn,manifest.disk.partition_table_id));
    if plan.hold == Some(1) {
        return PreflightReport {
            current,
            target: "Alpine Live rescue; no target partitioning or bootloader installation".into(),
            blocked: false,
            warning: format!(
                "RESCUE: interrupts connectivity on {}",
                manifest.disk.device
            ),
        };
    }
    PreflightReport {
        current,
        target: planned_target(plan, Some(manifest)),
        blocked: false,
        warning: format!(
            "ERASE ALL PARTITIONS ON {} ({} bytes). Exact disk-identity confirmation required. Boot method: {}.",
            manifest.disk.device,
            manifest.disk.size_bytes,
            if manifest.boot_mode == BootMode::Efi {
                "UEFI fallback EFI/BOOT/BOOTX64.EFI; no NVRAM"
            } else if planned_count(plan, manifest) == Some(3) {
                "BIOS/GPT dedicated stage2 partition 1"
            } else {
                "BIOS/MBR with reserved embedding gap"
            }
        ),
    }
}

fn planned_count(plan: &InstallPlan, manifest: &DeploymentManifest) -> Option<usize> {
    crate::shared::storage::planned_partition_sizes(
        manifest.boot_mode,
        manifest.disk.size_bytes,
        plan.system.boot_size_mib.unwrap_or(512),
        plan.system.root_size_mib,
    )
    .ok()
    .map(|p| p.len())
}
/// The same target preview is available when optional host probes or strict
/// preflight fail. Detected firmware/layout/network are then explicitly unknown.
pub fn planned_target(plan: &InstallPlan, manifest: Option<&DeploymentManifest>) -> String {
    if plan.hold == Some(1) {
        return "Alpine Live rescue; no target partitioning or bootloader installation".into();
    }
    let count = manifest.and_then(|m| planned_count(plan, m));
    let table = manifest
        .filter(|_| count.is_some())
        .map(|m| {
            if m.boot_mode == BootMode::Efi || count == Some(3) {
                "GPT"
            } else {
                "MBR"
            }
        })
        .unwrap_or("Unknown (disk/firmware unverified)");
    let firmware = manifest
        .map(|m| format!("{} (detected)", m.boot_mode.as_str()))
        .unwrap_or_else(|| "Unknown (unverified)".into());
    let network = if manifest.is_some() {
        "inherited validated source policy"
    } else {
        "source policy unverified"
    };
    let kernel = crate::targets::packages::configuration_for_plan(plan, plan.target, false)
        .map(|v| v.kernel.package.to_owned())
        .unwrap_or_else(|_| UNKNOWN.into());
    let gentoo = if plan.target == crate::model::Target::Gentoo {
        plan.system
            .gentoo_init
            .map(|init| {
                format!(
                    "Init: {}\nStage3: {}\nABI: amd64, no-multilib (64-bit only)\n",
                    init.as_str(),
                    init.stage3_variant()
                )
            })
            .unwrap_or_else(|| {
                "Init / stage3: Unknown (Gentoo plan requires explicit init)\n".into()
            })
    } else {
        format!(
            "Init: {}\n",
            if plan.target == crate::model::Target::Alpine {
                "OpenRC"
            } else {
                "systemd"
            }
        )
    };
    let root_size = match plan.system.root_size_mib {
        Some(mib) => format!("{mib} MiB"),
        None => manifest
            .and_then(|m| {
                crate::shared::storage::planned_partitions(
                    m.boot_mode,
                    m.disk.size_bytes,
                    plan.system.boot_size_mib.unwrap_or(512),
                )
                .ok()
                .and_then(|parts| {
                    parts
                        .last()
                        .map(|root| m.disk.size_bytes / 1_048_576 - root.start_mib)
                })
            })
            .map_or_else(
                || "remaining disk".into(),
                |mib| format!("remaining disk (about {mib} MiB)"),
            ),
    };
    let target = format!(
        "Distribution: {}\n{gentoo}Kernel package/family: {kernel} (version determined after installation)\nBootloader: rustaller-boot schema {}\nFirmware: {}\nPartition table: {table}; count: {}\nFAT32: {} MiB at /boot\nRoot: {} at / ({root_size})\nBtrfs: {}\nSwapfile: {}; swappiness {}\nHostname: {}\nTimezone: {}\nNetwork: {network}; MTU {}\nIPv6: {}; address preference: {}\nTCP congestion / queue: {} / {}\nDNS: {} {:?}; search {:?}\nSSH port: {}; account: {}\nPublic keys: {} sources; credentials hidden\nProfile: {}; tools={}; retain docs={}; cache={}\n",
        plan.target,
        crate::targets::boot::VERSION,
        firmware,
        count
            .map(|n| n.to_string())
            .unwrap_or_else(|| UNKNOWN.into()),
        plan.system.boot_size_mib.unwrap_or(512),
        plan.system.root_filesystem.as_str(),
        if plan.system.root_filesystem.as_str() == "btrfs" {
            format!(
                "@; zstd level {} (0=off)",
                plan.system.btrfs_compression.unwrap_or(3)
            )
        } else {
            "not selected".into()
        },
        plan.system
            .swap
            .size_mib
            .map(|n| format!("/swapfile {n} MiB"))
            .unwrap_or("disabled".into()),
        plan.system
            .swap
            .swappiness
            .map(|n| n.to_string())
            .unwrap_or("kernel default".into()),
        plan.post_install
            .hostname
            .as_deref()
            .unwrap_or("distribution default"),
        plan.post_install
            .timezone
            .as_deref()
            .unwrap_or("unspecified"),
        plan.system
            .network
            .mtu
            .map(|n| n.to_string())
            .unwrap_or("target defaults; not inherited".into()),
        plan.system.network.ipv6.as_str(),
        plan.system.network.address_preference.as_str(),
        plan.system.network.congestion.as_str(),
        plan.system.network.qdisc.as_str(),
        plan.system.dns.mode.as_str(),
        plan.system.dns.servers,
        plan.system.dns.search_domains,
        plan.ssh_port.unwrap_or(22),
        plan.username.as_deref().unwrap_or("root"),
        plan.ssh_keys.len(),
        plan.system.profile.as_str(),
        plan.system.with_tools,
        plan.system.keep_docs,
        plan.system.keep_package_cache
    );

    if plan.system.root_filesystem == crate::config::RootFilesystem::Btrfs {
        format!(
            "{target}Warning: FAT32 /boot is outside root snapshots; restore matching kernel/initramfs separately.\n"
        )
    } else {
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        command::{CommandOutput, MockRunner},
        test_support::Scratch,
    };
    fn output(text: &str) -> CommandOutput {
        CommandOutput::new(Some(0), text.as_bytes().to_vec(), Vec::new())
    }

    #[test]
    fn missing_optional_facts_do_not_abort_diagnostics_or_authorize_installation() {
        let root = Scratch::new("inspect-missing");
        let mut runner = MockRunner::empty();
        let facts = collect(&mut runner, &root.0);
        let text = facts.render_current();
        for group in [
            "[System]",
            "[CPU and ISA]",
            "[Virtualization]",
            "[Memory]",
            "[Storage]",
            "[Firmware]",
            "[Network]",
            "[Installation compatibility]",
        ] {
            assert!(text.contains(group), "missing {group}");
        }
        assert_eq!(facts.system.distribution, UNKNOWN);
        assert_eq!(facts.firmware.mode, UNKNOWN);
        assert!(facts.storage.devices.is_empty());
        assert!(text.contains("Diagnostics do not authorize installation"));
        assert!(runner.calls().iter().all(|c| c.phase == "host-inspection"));
        assert!(!text.contains("Some("));
    }
    #[test]
    fn memory_uses_available_not_free_and_checks_malformed_counts() {
        let root = Scratch::new("inspect-memory");
        root.write("proc/meminfo", "MemTotal: 1000 kB\nMemFree: 20 kB\nMemAvailable: 250 kB\nSwapTotal: 500 kB\nSwapFree: 100 kB\n");
        let facts = memory(&root.0);
        assert_eq!(facts.used, Some(750 * 1024));
        assert_eq!(facts.swap_used, Some(400 * 1024));
        root.write("proc/meminfo", "MemTotal: 100 kB\nMemAvailable: 500 kB\nSwapTotal: 50 kB\nSwapFree: 80 kB\nMemFree: bad\n");
        let facts = memory(&root.0);
        assert_eq!(facts.available, None);
        assert_eq!(facts.swap_used, None);
    }
    #[test]
    fn zram_and_loaded_balloon_driver_are_only_indicators() {
        let root = Scratch::new("inspect-zram");
        root.write("sys/block/zram0/disksize", "1048576\n");
        root.write("sys/block/zram0/mm_stat", "8192 4096 6144 0 0 0 0 0\n");
        fs::create_dir_all(root.0.join("sys/module/virtio_balloon")).unwrap();
        let facts = memory(&root.0);
        assert_eq!(facts.zram[0].capacity, Some(1048576));
        assert_eq!(facts.zram[0].compressed_bytes, Some(4096));
        assert_eq!(facts.balloon_drivers, ["virtio_balloon"]);
    }
    #[test]
    fn quoted_lsblk_is_data_not_shell_and_malformed_lines_are_rejected() {
        let parsed = pairs(r#"KNAME="vda" MODEL="Virtual\x20Disk" SIZE="1234""#).unwrap();
        assert_eq!(parsed["MODEL"], "Virtual Disk");
        assert!(pairs("KNAME=unquoted").is_none());
        assert!(pairs(r#"KNAME="vda" KNAME="sda""#).is_none());
        assert!(pairs(r#"KNAME="vda" MODEL="bad\q""#).is_none());
        assert!(pairs("SIZE=\"\u{1b}[31m123\"").is_some());
    }
    #[test]
    fn mount_and_route_parsers_reject_truncation_and_invalid_addresses() {
        assert!(mounts("invalid\n1 2 - ext4 /dev/vda rw\n").is_empty());
        let found = mounts(
            "25 1 8:2 / / rw,relatime - ext4 /dev/vda2 rw,errors=remount-ro\n26 25 8:1 / /boot rw - vfat /dev/vda1 rw,fmask=0177\n",
        );
        assert_eq!(found.len(), 2);
        assert!(found[0].options.contains("errors=remount-ro"));
        let found = routes(
            "default via 192.0.2.1 dev eth0 metric 20\ndefault via nope dev eth0\ndefault via 2001:db8::1 dev eth0\n",
            false,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].metric, Some(20));
        assert!(routes("default dev ../../bad", false).is_empty());
    }
    #[test]
    fn inventories_are_reobserved_and_diagnostic_identity_is_not_authorization() {
        let root = Scratch::new("inspect-disk-change");
        let listing = |device: &str| {
            format!("KNAME=\"{device}\" TYPE=\"disk\" SIZE=\"1073741824\" SERIAL=\"fixture\"\n")
        };
        let mut runner = MockRunner::new([output(&listing("vda")), output("df fixture")]);
        let first = storage(&mut runner, &root.0);
        let mut runner = MockRunner::new([output(&listing("vdb")), output("df fixture")]);
        let second = storage(&mut runner, &root.0);
        assert_eq!(first.devices[0].name, "vda");
        assert_eq!(second.devices[0].name, "vdb");
    }
    #[test]
    fn complete_collection_preserves_prefixes_mtu_routes_and_unknown_policy() {
        let root = Scratch::new("inspect-network");
        root.write("etc/os-release", "PRETTY_NAME=\"Fixture Linux 1\"\n");
        root.write(
            "etc/resolv.conf",
            "nameserver 192.0.2.53\nsearch example.test\n# secret is ignored\n",
        );
        root.write("proc/uptime", "3661.25 1.00\n");
        root.write("proc/1/comm", "init\n");
        root.write("sys/class/net/eth0/mtu", "1450\n");
        root.write("sys/class/net/eth0/address", "02:00:00:00:00:01\n");
        root.write("sys/class/net/eth0/operstate", "up\n");
        root.write("sys/class/net/eth0/speed", "-1\n");
        let mut runner = MockRunner::new([
            output("glibc 2.40"),
            output("kvm"),
            output("malformed lsblk"),
            output("df fixture"),
            output(
                "2: eth0 inet 192.0.2.2/24 brd 192.0.2.255\n2: eth0 inet6 2001:db8::2/64 scope global\n2: eth0 inet not-an-ip/2\n",
            ),
            output("default via 192.0.2.1 dev eth0"),
            output("default via 2001:db8::1 dev eth0"),
        ]);
        let facts = collect(&mut runner, &root.0);
        assert_eq!(facts.system.uptime_seconds, Some(3661));
        assert_eq!(facts.system.pid1, "init");
        assert_eq!(
            facts.network.interfaces[0].addresses,
            ["192.0.2.2/24", "2001:db8::2/64"]
        );
        assert_eq!(facts.network.interfaces[0].mtu, Some(1450));
        assert_eq!(facts.network.interfaces[0].speed_mbps, None);
        assert_eq!(facts.network.defaults.len(), 2);
        assert!(facts.network.interfaces[0].policy.starts_with("Unknown"));
        assert_eq!(facts.virtualization.environment, "KVM/QEMU");
        assert!(!facts.render_current().contains("secret"));
        assert!(!facts.render_current().contains("public IPv4"));
    }
    #[test]
    fn firmware_vendor_does_not_determine_boot_mode_and_unknown_is_honest() {
        let root = Scratch::new("inspect-firmware");
        root.write("sys/class/dmi/id/bios_vendor", "OVMF\n");
        fs::create_dir_all(root.0.join("sys/firmware")).unwrap();
        assert_eq!(firmware(&root.0, false).mode, "BIOS");
        fs::create_dir_all(root.0.join("sys/firmware/efi")).unwrap();
        assert_eq!(firmware(&root.0, false).mode, "UEFI");
        assert_eq!(firmware(&root.0, false).bitness, UNKNOWN);
        assert_eq!(firmware(&root.0, false).secure_boot, UNKNOWN);
    }
    #[test]
    fn read_bounds_and_terminal_control_characters_are_enforced() {
        let root = Scratch::new("inspect-read-bounds");
        root.write("bounded", &"x".repeat(MAX_TEXT as usize + 1));
        assert_eq!(read(&root.0, "bounded"), UNKNOWN);
        root.write("escaped", "name\u{1b}[31m\rvalue\n");
        let text = read(&root.0, "escaped");
        assert!(!text.contains('\u{1b}'));
        assert!(!text.contains('\r'));
    }
    #[test]
    fn mount_options_redact_credentials_and_bound_path_lists() {
        let value = mount_options(&format!(
            "rw,password=hunter2,credentials=/secret,user=account,lowerdir={}",
            "x".repeat(2048)
        ));
        assert!(!value.contains("hunter2"));
        assert!(!value.contains("/secret"));
        assert!(!value.contains("account"));
        assert!(value.contains("password=[redacted]"));
        assert!(value.len() < 1024);
    }
    #[test]
    fn failed_preflight_keeps_gentoo_and_btrfs_target_settings_visible() {
        let mut plan = InstallPlan::new(crate::model::Target::Gentoo);
        plan.system.root_filesystem = crate::config::RootFilesystem::Btrfs;
        let target = planned_target(&plan, None);
        assert!(target.contains("amd64-nomultilib-openrc"));
        assert!(target.contains("no-multilib (64-bit only)"));
        assert!(target.contains("gentoo-kernel-bin"));
        assert!(target.contains("Unknown (unverified)"));
        assert!(target.contains("at /boot"));
        assert!(target.contains("outside root snapshots"));
        assert!(target.contains("source policy unverified"));
        assert!(target.contains("SSH port:"));
        assert!(!target.contains("count: 0"));
    }
    #[test]
    fn detected_container_with_filtered_sysfs_has_unknown_firmware() {
        let root = Scratch::new("inspect-container-firmware");
        fs::create_dir_all(root.0.join("sys/firmware")).unwrap();
        let mut runner = MockRunner::new([output("glibc 2.40"), output("lxc")]);
        let facts = collect(&mut runner, &root.0);
        assert!(facts.virtualization.container);
        assert_eq!(facts.firmware.mode, UNKNOWN);
        assert_eq!(facts.firmware.bitness, UNKNOWN);
        assert_eq!(facts.firmware.secure_boot, UNKNOWN);
        assert!(!facts.render_current().contains("Boot mode: BIOS"));
    }
}
