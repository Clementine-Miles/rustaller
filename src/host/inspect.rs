//! Read-only inspection shared by CLI and TUI; renderers never probe the host.
use crate::{
    command::{CommandRunner, CommandSpec, capture_checked},
    deployment::DeploymentManifest,
    model::{BootMode, InstallPlan},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
};
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
fn optional_fact(runner: &mut impl CommandRunner, program: &str, args: &[&str]) -> Option<String> {
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
}
fn fact(runner: &mut impl CommandRunner, program: &str, args: &[&str]) -> String {
    optional_fact(runner, program, args)
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
    pub service_manager: String,
    pub load_average: Option<[f64; 3]>,
    pub tasks_running: Option<u64>,
    pub tasks_total: Option<u64>,
    /// Only recognized safe arguments retain values. Other values are redacted.
    pub kernel_command_line: String,
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
    pub hugepages: HugePageFacts,
    pub numa_nodes: Vec<NumaNodeFacts>,
    pub transparent_hugepages: String,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HugePageFacts {
    pub total: Option<u64>,
    pub free: Option<u64>,
    pub reserved: Option<u64>,
    pub surplus: Option<u64>,
    pub page_size: Option<u64>,
    pub anonymous_bytes: Option<u64>,
    pub shared_bytes: Option<u64>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NumaNodeFacts {
    pub name: String,
    pub cpus: String,
    pub total: Option<u64>,
    pub free: Option<u64>,
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
    pub nvme: Vec<NvmeFacts>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NvmeFacts {
    pub name: String,
    pub model: String,
    pub firmware: String,
    pub state: String,
    pub transport: String,
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
    pub driver: String,
    pub link_type: String,
    pub wireless: bool,
    pub qdisc: Vec<String>,
    pub ipv6_disabled: Option<bool>,
    pub accept_ra: Option<u64>,
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
    /// Observed process names only; installation ownership validation is separate.
    pub observed_managers: Vec<String>,
    pub congestion_control: String,
    pub available_congestion_controls: String,
    pub default_qdisc: String,
    pub ipv6_disabled: Option<bool>,
    pub ipv6_forwarding: Option<bool>,
    pub firewall: FirewallFacts,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FirewallFacts {
    pub nftables_table_count: Option<usize>,
    pub ipv4_policies: BTreeMap<String, String>,
    pub ipv6_policies: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SecurityFacts {
    pub lockdown: String,
    pub active_lsms: String,
    pub selinux: String,
    pub apparmor: String,
    pub tainted: Option<u64>,
    pub dmesg_restrict: Option<bool>,
    pub kptr_restrict: Option<u64>,
    pub unprivileged_bpf_disabled: Option<u64>,
    pub kexec_disabled: Option<bool>,
    pub vulnerabilities: Vec<(String, String)>,
}
/// Public diagnostics hide hardware identifiers. Raw typed inventory stays local
/// for disk selection; it is never uploaded by inspection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum InspectionPrivacy {
    #[default]
    Redacted,
    ShowIdentifiers,
}
impl InspectionPrivacy {
    fn identifier(self, value: &str) -> &str {
        if value.is_empty() || value == UNKNOWN {
            UNKNOWN
        } else if self == Self::Redacted {
            "[redacted]"
        } else {
            value
        }
    }
}
#[derive(Clone, Debug)]
pub struct FirmwareFacts {
    pub mode: String,
    pub bitness: String,
    pub secure_boot: String,
    pub vendor: String,
    pub version: String,
    pub release_date: String,
    pub setup_mode: String,
    pub efivarfs_mounted: Option<bool>,
    pub variable_store_readable: Option<bool>,
    pub tpm_devices: usize,
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
    pub security: SecurityFacts,
}

fn boolean(root: &Path, name: &str) -> Option<bool> {
    number(root, name).filter(|v| *v <= 1).map(|v| v == 1)
}
fn process_names(root: &Path) -> BTreeSet<String> {
    names(root, "proc")
        .into_iter()
        .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
        .map(|pid| line(&read(root, &format!("proc/{pid}/comm"))))
        .filter(|name| name != UNKNOWN)
        .collect()
}
fn service_manager(root: &Path) -> String {
    let pid1 = read(root, "proc/1/comm");
    match pid1.trim() {
        "systemd" => "systemd (PID 1)".into(),
        "runit" => "runit (PID 1)".into(),
        "s6-svscan" | "s6-linux-init" => "s6 (PID 1)".into(),
        "dinit" => "dinit (PID 1)".into(),
        _ if root.join("run/openrc/softlevel").is_file() => format!(
            "OpenRC (runtime runlevel {})",
            line(&read(root, "run/openrc/softlevel"))
        ),
        _ => "Unknown (PID 1 name alone does not identify a service manager)".into(),
    }
}
fn load_average(text: &str) -> (Option<[f64; 3]>, Option<u64>, Option<u64>) {
    let words = text.split_whitespace().collect::<Vec<_>>();
    let values = (0..3)
        .map(|i| {
            words
                .get(i)?
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.0)
        })
        .collect::<Option<Vec<_>>>();
    let load = values.map(|v| [v[0], v[1], v[2]]);
    let tasks = words
        .get(3)
        .and_then(|s| s.split_once('/'))
        .and_then(|(r, t)| {
            let r = r.parse::<u64>().ok()?;
            let t = t.parse::<u64>().ok()?;
            (r <= t && t > 0).then_some((r, t))
        });
    (load, tasks.map(|(r, _)| r), tasks.map(|(_, t)| t))
}
fn safe_word(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.,:+/-".contains(&b))
}
fn redacted_command_line(text: &str) -> String {
    if text == UNKNOWN {
        return UNKNOWN.into();
    }
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    for c in text.chars().take(65537) {
        if escaped {
            token.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && matches!(c, '\'' | '"') {
            quote = Some(c);
        } else if c.is_whitespace() && quote.is_none() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else {
            token.push(c);
        }
        if tokens.len() >= 256 || token.len() > 8192 {
            return "[redacted oversized command line]".into();
        }
    }
    if quote.is_some() || escaped || text.chars().count() > 65536 {
        return "[redacted malformed command line]".into();
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
        .into_iter()
        .map(|token| {
            if matches!(
                token.as_str(),
                "ro" | "rw"
                    | "quiet"
                    | "splash"
                    | "debug"
                    | "nosmt"
                    | "noapic"
                    | "nolapic"
                    | "nopti"
                    | "noibrs"
                    | "noibpb"
                    | "nospectre_v1"
                    | "nospectre_v2"
            ) {
                return token;
            }
            if let Some((key, value)) = token.split_once('=') {
                let safe = matches!(
                    key,
                    "console"
                        | "loglevel"
                        | "panic"
                        | "rootfstype"
                        | "mitigations"
                        | "nosmt"
                        | "spectre_v2"
                        | "pti"
                        | "audit"
                        | "selinux"
                        | "enforcing"
                        | "apparmor"
                        | "security"
                        | "lsm"
                        | "lockdown"
                        | "iommu"
                        | "amd_iommu"
                        | "intel_iommu"
                        | "ipv6.disable"
                        | "systemd.unified_cgroup_hierarchy"
                        | "cgroup_no_v1"
                        | "transparent_hugepage"
                );
                if safe && safe_word(value) {
                    format!("{key}={value}")
                } else if safe_word(key) {
                    format!("{key}=[redacted]")
                } else {
                    "[redacted]".into()
                }
            } else {
                "[redacted]".into()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
fn bracketed_policy(value: &str) -> String {
    value
        .split_whitespace()
        .find_map(|v| v.strip_prefix('[').and_then(|v| v.strip_suffix(']')))
        .filter(|v| safe_word(v))
        .map(str::to_owned)
        .unwrap_or_else(|| UNKNOWN.into())
}
fn security(root: &Path) -> SecurityFacts {
    let vulnerabilities = names(root, "sys/devices/system/cpu/vulnerabilities")
        .into_iter()
        .take(64)
        .map(|name| {
            let status = line(&read(
                root,
                &format!("sys/devices/system/cpu/vulnerabilities/{name}"),
            ));
            (name, status)
        })
        .collect();
    let selinux = match boolean(root, "sys/fs/selinux/enforce") {
        Some(true) => "Enforcing",
        Some(false) => "Permissive",
        None => UNKNOWN,
    }
    .into();
    let apparmor = match read(root, "sys/module/apparmor/parameters/enabled").trim() {
        "Y" => "Enabled",
        "N" => "Disabled",
        _ => UNKNOWN,
    }
    .into();
    SecurityFacts {
        lockdown: bracketed_policy(&read(root, "sys/kernel/security/lockdown")),
        active_lsms: line(&read(root, "sys/kernel/security/lsm")),
        selinux,
        apparmor,
        tainted: number(root, "proc/sys/kernel/tainted"),
        dmesg_restrict: boolean(root, "proc/sys/kernel/dmesg_restrict"),
        kptr_restrict: number(root, "proc/sys/kernel/kptr_restrict").filter(|v| *v <= 2),
        unprivileged_bpf_disabled: number(root, "proc/sys/kernel/unprivileged_bpf_disabled")
            .filter(|v| *v <= 2),
        kexec_disabled: boolean(root, "proc/sys/kernel/kexec_load_disabled"),
        vulnerabilities,
    }
}
fn numa(root: &Path) -> Vec<NumaNodeFacts> {
    names(root, "sys/devices/system/node")
        .into_iter()
        .filter(|name| {
            name.strip_prefix("node")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|name| {
            let text = read(root, &format!("sys/devices/system/node/{name}/meminfo"));
            let kb = |key| {
                text.lines().find_map(|v| {
                    let words = v.split_whitespace().collect::<Vec<_>>();
                    let i = words.iter().position(|word| *word == key)?;
                    if words.get(i + 2) != Some(&"kB") {
                        return None;
                    }
                    words.get(i + 1)?.parse::<u64>().ok()?.checked_mul(1024)
                })
            };
            let cpus = line(&read(
                root,
                &format!("sys/devices/system/node/{name}/cpulist"),
            ));
            let cpus = if cpus
                .bytes()
                .all(|b| b.is_ascii_digit() || b",-".contains(&b))
            {
                cpus
            } else {
                UNKNOWN.into()
            };
            NumaNodeFacts {
                name,
                cpus,
                total: kb("MemTotal:"),
                free: kb("MemFree:"),
            }
        })
        .collect()
}
fn nvme(root: &Path) -> Vec<NvmeFacts> {
    names(root, "sys/class/nvme")
        .into_iter()
        .filter(|name| {
            name.strip_prefix("nvme")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|name| NvmeFacts {
            model: line(&read(root, &format!("sys/class/nvme/{name}/model"))),
            firmware: line(&read(root, &format!("sys/class/nvme/{name}/firmware_rev"))),
            state: line(&read(root, &format!("sys/class/nvme/{name}/state"))),
            transport: line(&read(root, &format!("sys/class/nvme/{name}/transport"))),
            name,
        })
        .collect()
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
    let meminfo = read(root, "proc/meminfo");
    let count = |key| {
        meminfo.lines().find_map(|v| {
            let (name, value) = v.split_once(':')?;
            if name != key {
                return None;
            }
            value.trim().parse::<u64>().ok()
        })
    };
    let mut result = MemoryFacts {
        hugepages: HugePageFacts {
            total: count("HugePages_Total"),
            free: count("HugePages_Free"),
            reserved: count("HugePages_Rsvd"),
            surplus: count("HugePages_Surp"),
            page_size: fields.get("Hugepagesize").copied(),
            anonymous_bytes: fields.get("AnonHugePages").copied(),
            shared_bytes: fields.get("ShmemHugePages").copied(),
        },
        numa_nodes: numa(root),
        transparent_hugepages: bracketed_policy(&read(
            root,
            "sys/kernel/mm/transparent_hugepage/enabled",
        )),
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
                "uuid",
                "partuuid",
                "serial",
                "wwn",
                "fsid",
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
        if key.is_empty()
            || key.len() > 64
            || !key
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
        nvme: nvme(root),
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
fn driver(root: &Path, name: &str) -> String {
    fs::read_link(root.join(format!("sys/class/net/{name}/device/driver")))
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .filter(|v| safe_name(v))
        .unwrap_or_else(|| UNKNOWN.into())
}
fn link_type(root: &Path, name: &str) -> String {
    let base = root.join(format!("sys/class/net/{name}"));
    if base.join("wireless").is_dir() || base.join("phy80211").exists() {
        return "Wireless".into();
    }
    if base.join("bridge").is_dir() {
        return "Bridge".into();
    }
    if base.join("bonding").is_dir() {
        return "Bond".into();
    }
    match number(root, &format!("sys/class/net/{name}/type")) {
        Some(1) => "Ethernet (virtual/physical not inferred)".into(),
        Some(772) => "Loopback".into(),
        Some(512) => "PPP".into(),
        Some(768) => "IP tunnel".into(),
        Some(776) => "IPv6 tunnel".into(),
        Some(n) => format!("Linux ARPHRD {n}"),
        None => UNKNOWN.into(),
    }
}
fn qdiscs(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut result: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for v in text.lines().take(1024) {
        let w = v.split_whitespace().collect::<Vec<_>>();
        if w.first() != Some(&"qdisc") {
            continue;
        }
        let Some(kind) = w.get(1).filter(|v| safe_name(v)) else {
            continue;
        };
        let Some(name) = w
            .iter()
            .position(|v| *v == "dev")
            .and_then(|i| w.get(i + 1))
            .filter(|v| safe_name(v))
        else {
            continue;
        };
        let q = result.entry((*name).into()).or_default();
        if q.len() < 16 && !q.iter().any(|v| v == kind) {
            q.push((*kind).into());
        }
    }
    result
}
fn firewall_policies(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .take(1024)
        .filter_map(|v| {
            let w = v.split_whitespace().collect::<Vec<_>>();
            if w.len() != 3
                || w[0] != "-P"
                || !matches!(w[1], "INPUT" | "FORWARD" | "OUTPUT")
                || !matches!(w[2], "ACCEPT" | "DROP")
            {
                return None;
            }
            Some((w[1].into(), w[2].into()))
        })
        .collect()
}
fn firewall(runner: &mut impl CommandRunner) -> FirewallFacts {
    let tables = optional_fact(runner, "nft", &["list", "tables"]).and_then(|text| {
        text.lines()
            .take(1025)
            .map(|v| {
                let words = v.split_whitespace().collect::<Vec<_>>();
                (words.len() == 3
                    && words[0] == "table"
                    && matches!(
                        words[1],
                        "ip" | "ip6" | "inet" | "arp" | "bridge" | "netdev"
                    ))
                .then_some(())
            })
            .collect::<Option<Vec<_>>>()
            .filter(|v| v.len() <= 1024)
            .map(|v| v.len())
    });
    let ipv4_policies = firewall_policies(&fact(runner, "iptables", &["-S"]));
    let ipv6_policies = firewall_policies(&fact(runner, "ip6tables", &["-S"]));
    FirewallFacts {
        nftables_table_count: tables,
        ipv4_policies,
        ipv6_policies,
    }
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
    let mut interfaces: Vec<InterfaceFacts> = interface_names
        .into_iter()
        .map(|name| {
            let base = format!("sys/class/net/{name}");
            InterfaceFacts {
                driver: driver(root, &name),
                link_type: link_type(root, &name),
                wireless: root.join(format!("sys/class/net/{name}/wireless")).is_dir()
                    || root.join(format!("sys/class/net/{name}/phy80211")).exists(),
                ipv6_disabled: boolean(
                    root,
                    &format!("proc/sys/net/ipv6/conf/{name}/disable_ipv6"),
                ),
                accept_ra: number(root, &format!("proc/sys/net/ipv6/conf/{name}/accept_ra"))
                    .filter(|v| *v <= 2),
                qdisc: Vec::new(),
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
            v.split_once(' ').is_some_and(|(name, _)| {
                [
                    "port",
                    "passwordauthentication",
                    "pubkeyauthentication",
                    "kbdinteractiveauthentication",
                    "permitemptypasswords",
                    "authenticationmethods",
                    "permitrootlogin",
                ]
                .iter()
                .any(|key| name.eq_ignore_ascii_case(key))
            })
        })
        .map(line)
        .take(16)
        .collect();
    let qdiscs = qdiscs(&fact(runner, "tc", &["qdisc", "show"]));
    for interface in &mut interfaces {
        interface.qdisc = qdiscs.get(&interface.name).cloned().unwrap_or_default();
    }
    let observed_managers = process_names(root)
        .into_iter()
        .filter(|name| {
            matches!(
                name.as_str(),
                "systemd-network"
                    | "systemd-networkd"
                    | "NetworkManager"
                    | "dhcpcd"
                    | "dhclient"
                    | "connmand"
                    | "wickedd"
                    | "networking"
            )
        })
        .collect();
    NetworkFacts {
        interfaces,
        defaults,
        dns,
        ssh_policy,
        observed_managers,
        congestion_control: line(&read(root, "proc/sys/net/ipv4/tcp_congestion_control")),
        available_congestion_controls: line(&read(
            root,
            "proc/sys/net/ipv4/tcp_available_congestion_control",
        )),
        default_qdisc: line(&read(root, "proc/sys/net/core/default_qdisc")),
        ipv6_disabled: boolean(root, "proc/sys/net/ipv6/conf/all/disable_ipv6"),
        ipv6_forwarding: boolean(root, "proc/sys/net/ipv6/conf/all/forwarding"),
        firewall: firewall(runner),
    }
}
fn efi_boolean(root: &Path, name: &str) -> String {
    let path = root.join(format!(
        "sys/firmware/efi/efivars/{name}-8be4df61-93ca-11d2-aa0d-00e098032b8c"
    ));
    let mut bytes = [0u8; 6];
    match fs::File::open(path).and_then(|mut f| f.read(&mut bytes)) {
        Ok(5) if bytes[4] == 0 => "Disabled".into(),
        Ok(5) if bytes[4] == 1 => "Enabled".into(),
        _ => UNKNOWN.into(),
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
    let secure_boot = if mode == "UEFI" {
        efi_boolean(root, "SecureBoot")
    } else if mode == "BIOS" {
        "Not applicable".into()
    } else {
        UNKNOWN.into()
    };
    let setup_mode = if mode == "UEFI" {
        efi_boolean(root, "SetupMode")
    } else if mode == "BIOS" {
        "Not applicable".into()
    } else {
        UNKNOWN.into()
    };
    let mounts = read(root, "proc/self/mountinfo");
    let efivarfs_mounted = if mode == "UEFI" && mounts != UNKNOWN {
        Some(mounts.lines().any(|line| {
            line.split_once(" - ").is_some_and(|(left, right)| {
                left.split_whitespace().nth(4) == Some("/sys/firmware/efi/efivars")
                    && right.split_whitespace().next() == Some("efivarfs")
            })
        }))
    } else {
        None
    };
    let variable_store_readable = efivarfs_mounted
        .map(|mounted| mounted && fs::read_dir(root.join("sys/firmware/efi/efivars")).is_ok());
    FirmwareFacts {
        version: line(&read(root, "sys/class/dmi/id/bios_version")),
        release_date: line(&read(root, "sys/class/dmi/id/bios_date")),
        setup_mode,
        efivarfs_mounted,
        variable_store_readable,
        tpm_devices: names(root, "sys/class/tpm").len(),
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
    let (load_average, tasks_running, tasks_total) = load_average(&read(root, "proc/loadavg"));
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
            service_manager: service_manager(root),
            load_average,
            tasks_running,
            tasks_total,
            kernel_command_line: redacted_command_line(&read(root, "proc/cmdline")),
        },
        cpu: cpu::inspect(root),
        virtualization,
        memory: memory(root),
        storage: storage(runner, root),
        firmware,
        network: network(runner, root),
        security: security(root),
    }
}
fn value<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| UNKNOWN.into())
}
fn yes_no(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => UNKNOWN,
    }
}
fn policy_summary(policies: &BTreeMap<String, String>) -> String {
    if policies.is_empty() {
        return "Unavailable".into();
    }
    policies
        .iter()
        .map(|(chain, policy)| format!("{chain}={policy}"))
        .collect::<Vec<_>>()
        .join(", ")
}
fn private_source(source: &str, privacy: InspectionPrivacy) -> String {
    if source.contains('@') && (source.starts_with("//") || source.contains("://")) {
        return "[network mount credentials redacted]".into();
    }
    if privacy == InspectionPrivacy::Redacted
        && (source.starts_with("/dev/disk/by-")
            || source.starts_with("/dev/mapper/luks-")
            || source.starts_with("UUID=")
            || source.starts_with("PARTUUID="))
    {
        "[stable device identifier redacted]".into()
    } else {
        source.into()
    }
}
impl HostInspection {
    pub fn render_summary(&self) -> String {
        format!(
            "[Summary]\n{}; kernel {}; service manager {}\nCPU: {} logical / {} allowed; {}\nRAM: {} used / {}; swap {} used / {}\nDefault-route interfaces: {}\n",
            self.system.distribution,
            self.system.kernel,
            self.system.service_manager,
            value(self.cpu.logical_cpus),
            value(self.cpu.schedulable_cpus),
            self.cpu
                .isa_level
                .map(cpu::IsaLevel::as_str)
                .unwrap_or(UNKNOWN),
            display_bytes(self.memory.used),
            display_bytes(self.memory.total),
            display_bytes(self.memory.swap_used),
            display_bytes(self.memory.swap_total),
            if self.network.defaults.is_empty() {
                UNKNOWN.into()
            } else {
                self.network
                    .defaults
                    .iter()
                    .map(|r| r.interface.as_str())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )
    }
    pub fn render_current(&self) -> String {
        self.render_current_with_privacy(InspectionPrivacy::Redacted)
    }
    /// Only explicit local callers can request unredacted MAC and disk identities.
    /// Credentials and unsafe kernel arguments remain redacted in either mode.
    pub fn render_current_with_privacy(&self, privacy: InspectionPrivacy) -> String {
        let s = &self.system;
        let mut result = self.render_summary();
        if privacy == InspectionPrivacy::Redacted {
            result.push_str("Privacy: MAC and stable disk identifiers are redacted.\n");
        }
        result.push_str(&format!("\n[System]\nOS: {}\nKernel: {}\nCPU Architecture: {}\nHostname: {}\nUptime: {}\nInit / PID 1: {}\nService manager: {}\nlibc: {}\nLoad averages (1/5/15 min): {}\nTasks running / total: {} / {}\nKernel command line (filtered): {}\n",
            s.distribution,s.kernel,s.architecture,s.hostname,
            s.uptime_seconds.map(|v|format!("{}d {}h {}m",v/86400,(v%86400)/3600,(v%3600)/60)).unwrap_or_else(||UNKNOWN.into()),
            s.pid1,s.service_manager,s.libc,
            s.load_average.map(|v|format!("{:.2} / {:.2} / {:.2}",v[0],v[1],v[2])).unwrap_or_else(||UNKNOWN.into()),
            value(s.tasks_running),value(s.tasks_total),s.kernel_command_line));
        let security = &self.security;
        result.push_str(&format!("\n[Security]\nLockdown: {}\nActive LSMs: {}\nSELinux: {}; AppArmor: {}\nKernel taint flags: {}\ndmesg restricted: {}; kernel pointer restriction: {}\nUnprivileged BPF policy: {}; kexec disabled: {}\n",
            security.lockdown,security.active_lsms,security.selinux,security.apparmor,value(security.tainted),yes_no(security.dmesg_restrict),value(security.kptr_restrict),value(security.unprivileged_bpf_disabled),yes_no(security.kexec_disabled)));
        if security.vulnerabilities.is_empty() {
            result.push_str("CPU vulnerability/mitigation status: Unavailable\n");
        }
        for (name, status) in &security.vulnerabilities {
            result.push_str(&format!("{name}: {status}\n"));
        }
        result.push_str("Security facts are kernel observations; no security audit or active probes were performed.\n");
        result.push_str(&format!("\n[CPU and ISA]\n{}\n[Virtualization]\nEnvironment: {}\nDMI manufacturer/product: {} / {}\nPhysical host CPU: Unverifiable; capabilities are guest-visible\n\n[Memory]\nRAM total / available / used: {} / {} / {}\nSwap total / used: {} / {}\nBalloon indicators: {}\n",
            self.cpu.render(),self.virtualization.environment,self.virtualization.manufacturer,self.virtualization.product,
            display_bytes(self.memory.total),display_bytes(self.memory.available),display_bytes(self.memory.used),display_bytes(self.memory.swap_total),display_bytes(self.memory.swap_used),
            if self.memory.balloon_drivers.is_empty() {"No loaded known balloon driver detected (availability unknown)".into()} else {format!("{} loaded; active reclamation unknown",self.memory.balloon_drivers.join(", "))}));
        let huge = &self.memory.hugepages;
        result.push_str(&format!("Huge pages total/free/reserved/surplus: {} / {} / {} / {}\nHuge page size: {}; anonymous/shared huge pages: {} / {}\nTransparent huge pages: {}\n",
            value(huge.total),value(huge.free),value(huge.reserved),value(huge.surplus),display_bytes(huge.page_size),display_bytes(huge.anonymous_bytes),display_bytes(huge.shared_bytes),self.memory.transparent_hugepages));
        if self.memory.numa_nodes.is_empty() {
            result.push_str("NUMA nodes: Unavailable\n");
        }
        for node in &self.memory.numa_nodes {
            result.push_str(&format!(
                "NUMA {}: CPUs {}; total/free {} / {}\n",
                node.name,
                node.cpus,
                display_bytes(node.total),
                display_bytes(node.free)
            ));
        }
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
                d.name,nonempty(&d.kind),display_bytes(d.bytes),nonempty(&d.model),privacy.identifier(&d.identity),nonempty(&d.transport),nonempty(&d.parent),
                value(d.logical_sector),value(d.physical_sector),nonempty(&d.partition_table),nonempty(&d.filesystem),yes_no(d.rotational),value(d.discard_max)));
            if !d.slaves.is_empty() || !d.holders.is_empty() {
                result.push_str(&format!(
                    "Relationships: slaves {:?}; holders {:?}\n",
                    d.slaves, d.holders
                ));
            }
        }
        for controller in &self.storage.nvme {
            result.push_str(&format!(
                "NVMe {}: {}; firmware {}; state {}; transport {} (sysfs; no device probe)\n",
                controller.name,
                controller.model,
                controller.firmware,
                controller.state,
                controller.transport
            ));
        }
        if self.storage.mounts.is_empty() {
            result.push_str("Current root/boot mounts: Unavailable\n");
        }
        for m in &self.storage.mounts {
            result.push_str(&format!(
                "Mount {}: {} {}; options {}\n",
                m.path,
                private_source(&m.source, privacy),
                m.filesystem,
                m.options
            ));
        }
        // df can use a stable device symlink as its source; redact that field too.
        let utilization = self
            .storage
            .utilization
            .lines()
            .map(|line| {
                let Some((source, rest)) = line.split_once(char::is_whitespace) else {
                    return line.to_owned();
                };
                format!("{} {}", private_source(source, privacy), rest.trim_start())
            })
            .collect::<Vec<_>>()
            .join("\n");
        let f = &self.firmware;
        result.push_str(&format!("Disk utilization (bytes):\n{}\nRAID status:\n{}\n\n[Firmware]\nBoot mode: {}\nUEFI bitness: {}\nSecure Boot: {}\nUEFI setup mode: {}\nFirmware vendor/version/date: {} / {} / {}\nefivarfs mounted / store readable: {} / {}\nTPM devices detected: {}\nCurrent bootloader: Unknown (not inferred from files)\n\n[Network]\n",
            utilization,nonempty(&self.storage.raid),f.mode,f.bitness,f.secure_boot,f.setup_mode,f.vendor,f.version,f.release_date,yes_no(f.efivarfs_mounted),yes_no(f.variable_store_readable),f.tpm_devices));
        if self.network.interfaces.is_empty() {
            result.push_str("Interfaces: Unavailable\n");
        }
        for n in &self.network.interfaces {
            result.push_str(&format!("{}: MAC {}; MTU {}; state {}; speed {}\nLink type: {}; driver {}; wireless {}\nAddresses: {}\nAddress policy: {}\nIPv6 disabled: {}; accept_ra {}; qdisc {}\n",
            n.name,privacy.identifier(&n.mac),value(n.mtu),n.state,n.speed_mbps.map(|v|format!("{v} Mbit/s")).unwrap_or_else(||"Unavailable".into()),
            n.link_type,n.driver,if n.wireless {"detected"} else {"not detected"},
            if n.addresses.is_empty() {UNKNOWN.into()} else {n.addresses.join(", ")},n.policy,
            yes_no(n.ipv6_disabled),value(n.accept_ra),if n.qdisc.is_empty() {"Unavailable".into()} else {n.qdisc.join(", ")}));
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
                value(r.metric)
            ));
        }
        let net = &self.network;
        result.push_str(&format!("Observed network manager processes: {} (ownership unverified)\nTCP congestion control: {}; available {}\nDefault qdisc: {}\nIPv6 globally disabled / forwarding: {} / {}\nDNS/search configuration: {}\nSSH policy: {}\nNftables tables: {}; IPv4 default policies: {}; IPv6 default policies: {}\nFirewall summary omits rule contents; effective filtering is unverified.\nConnectivity, NAT, public IP, ASN/provider/location: Not tested (no online lookup).\n\n[Installation compatibility]\n{}\nDiagnostics do not authorize installation; strict preflight and disk identity checks remain required.\n",
            if net.observed_managers.is_empty() {"Unavailable".into()} else {net.observed_managers.join(", ")},net.congestion_control,net.available_congestion_controls,net.default_qdisc,yes_no(net.ipv6_disabled),yes_no(net.ipv6_forwarding),
            if net.dns.is_empty() {UNKNOWN.into()} else {net.dns.join("; ")},if net.ssh_policy.is_empty() {UNKNOWN.into()} else {net.ssh_policy.join("; ")},
            value(net.firewall.nftables_table_count),policy_summary(&net.firewall.ipv4_policies),policy_summary(&net.firewall.ipv6_policies),self.virtualization.compatibility));
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
            if plan.system.explicit_network.is_none() {
                interface.policy = format!(
                    "validated source policy: DHCPv4={}; DHCPv6={}; RA={}",
                    nic.dhcp4, nic.dhcp6, nic.accept_ra
                );
            } else {
                interface.policy =
                    "Unknown; explicit installation policy is shown in TARGET SYSTEM".into();
            }
        }
    }
    let mut current = observed.render_current();
    current.push_str(&format!("Selected disk (recorded preflight identity): {} ({} bytes)\nStable disk identifiers: [redacted]; installation confirmation is checked separately.\n",manifest.disk.device,manifest.disk.size_bytes));
    if plan.hold == Some(1) {
        return PreflightReport {
            current,
            target: planned_target(plan, Some(manifest)),
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

fn family_policy(
    dynamic: bool,
    ra: bool,
    addresses: &[String],
    v6: bool,
    disabled: bool,
) -> String {
    if disabled {
        return "disabled".into();
    }
    match (dynamic, ra, !addresses.is_empty()) {
        (true, true, _) => "SLAAC + DHCPv6".into(),
        (true, false, _) => if v6 { "DHCPv6" } else { "DHCPv4" }.into(),
        (false, true, _) => "SLAAC / router advertisements".into(),
        (false, false, true) => "static".into(),
        _ => "disabled (no address policy)".into(),
    }
}
fn planned_interface(
    name: &str,
    addresses4: &[String],
    gateway4: Option<&str>,
    addresses6: &[String],
    gateway6: Option<&str>,
    dynamic: [bool; 3],
    disable6: bool,
) -> String {
    let [dhcp4, dhcp6, ra] = dynamic;
    let addresses = |values: &[String], automatic, disabled| {
        if disabled {
            "not configured".into()
        } else if automatic {
            if values.is_empty() {
                "assigned at runtime".into()
            } else {
                format!(
                    "observed snapshot {}; reassigned at runtime",
                    values.join(", ")
                )
            }
        } else if values.is_empty() {
            "not configured".into()
        } else {
            values.join(", ")
        }
    };
    let gateway = |value: Option<&str>, automatic, disabled| {
        if disabled {
            "not configured".into()
        } else if automatic {
            value
                .map(|v| format!("observed snapshot {v}; learned at runtime"))
                .unwrap_or_else(|| "learned at runtime".into())
        } else {
            value.unwrap_or("none specified").to_owned()
        }
    };
    format!(
        "Interface: {name}; MAC [redacted]\n  IPv4: {}; addresses {}; gateway {}\n  IPv6: {}; addresses {}; gateway {}\n",
        family_policy(dhcp4, false, addresses4, false, false),
        addresses(addresses4, dhcp4, false),
        gateway(gateway4, dhcp4, false),
        family_policy(dhcp6, ra, addresses6, true, disable6),
        addresses(addresses6, dhcp6 || ra, disable6),
        gateway(gateway6, dhcp6 || ra, disable6)
    )
}
/// Effective address policies are rendered from the same typed fields used by
/// target network configuration. Dynamic leases remain snapshots, never pinned.
pub fn planned_network(plan: &InstallPlan, manifest: Option<&DeploymentManifest>) -> String {
    let disable6 = plan.system.network.ipv6 == crate::config::Ipv6Policy::Disable;
    let mut text = String::new();
    if let Some(explicit) = &plan.system.explicit_network {
        text.push_str("Network policy origin: explicit user configuration\n");
        let name = manifest
            .and_then(|m| {
                m.network
                    .iter()
                    .find(|n| n.mac.eq_ignore_ascii_case(&explicit.mac))
            })
            .map(|n| n.name.as_str())
            .unwrap_or("MAC-selected interface (name unverified)");
        text.push_str(&planned_interface(
            name,
            &explicit
                .ipv4_address
                .clone()
                .into_iter()
                .collect::<Vec<_>>(),
            explicit.ipv4_gateway.as_deref(),
            &explicit
                .ipv6_address
                .clone()
                .into_iter()
                .collect::<Vec<_>>(),
            explicit.ipv6_gateway.as_deref(),
            [explicit.dhcp4, explicit.dhcp6, explicit.accept_ra],
            disable6,
        ));
        if manifest.is_none() {
            text.push_str(
                "Explicit interface binding and installation compatibility: unverified\n",
            );
        }
    } else if let Some(manifest) = manifest {
        text.push_str("Network policy origin: inherited validated source policy\n");
        for nic in &manifest.network {
            text.push_str(&planned_interface(
                &nic.name,
                &nic.ipv4_addresses,
                nic.ipv4_gateway.as_deref(),
                &nic.ipv6_addresses,
                nic.ipv6_gateway.as_deref(),
                [nic.dhcp4, nic.dhcp6, nic.accept_ra],
                disable6,
            ));
        }
        if manifest.network.is_empty() {
            text.push_str("Per-interface policies: Unknown (inventory unavailable)\n");
        }
    } else {
        text.push_str("Network policy origin: inherited; source policy unverified\nPer-interface IPv4/IPv6 addressing and gateways: Unknown\n");
    }
    text.push_str(&format!(
        "Effective MTU: {}\n",
        plan.system
            .network
            .mtu
            .map(|v| v.to_string())
            .unwrap_or_else(|| "target defaults (source MTU not inherited)".into())
    ));
    if plan.system.dns.mode == crate::config::DnsMode::Static {
        text.push_str(&format!(
            "Effective DNS: static {}; search {}\n",
            plan.system
                .dns
                .servers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            if plan.system.dns.search_domains.is_empty() {
                "none".into()
            } else {
                plan.system.dns.search_domains.join(", ")
            }
        ));
    } else {
        text.push_str("Effective DNS: automatic acquisition via selected network manager; search domains learned at runtime\n");
    }
    text
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
    let account = plan.account_name();
    let root_ssh = if account == "root" {
        "public key only"
    } else {
        "disabled. The selected account has sudo access."
    };
    let ssh = format!(
        "SSH port: {}; account: {account}\nSSH public keys only: {} sources\nRoot SSH login: {root_ssh}\n",
        plan.ssh_port.unwrap_or(22),
        plan.ssh_keys.len(),
    );
    if plan.hold == Some(1) {
        return format!(
            "Alpine Live rescue\nInit: {}\nNo target partitioning or bootloader installation.\n{ssh}",
            crate::model::Target::Alpine
                .init_system()
                .unwrap_or(UNKNOWN),
        );
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
    let network = if plan.system.explicit_network.is_some() {
        "explicit user policy"
    } else if manifest.is_some() {
        "inherited validated source policy"
    } else {
        "source policy unverified"
    };
    let kernel = crate::postinstall::required_kernel(plan.target).unwrap_or(UNKNOWN);
    let init = plan.target.init_system().unwrap_or(UNKNOWN);
    let gentoo = if plan.target == crate::model::Target::Gentoo {
        format!(
            "Stage3: {}\nABI: amd64, no-multilib (64-bit only)\n",
            crate::host::sources::GENTOO_STAGE3_PREFIX,
        )
    } else {
        String::new()
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
        "Distribution: {}\nInit: {init}\n{gentoo}Kernel package: {kernel} (version determined after installation)\nBootloader: rustaller-boot schema {}\nFirmware: {}\nPartition table: {table}; count: {}\nFAT32: {} MiB at /boot\nRoot: {} at / ({root_size})\nSwapfile: {}; swappiness {}\nHostname: {}\nTimezone: {}\nNetwork: {network}; MTU {}\nIPv6: {}; address preference: {}\nTCP congestion / queue: {} / {}\nDNS: {} {:?}; search {:?}\n{ssh}Profile: {}; tools={}; retain docs={}; cache={}\n",
        plan.target,
        crate::targets::boot::VERSION,
        firmware,
        count
            .map(|n| n.to_string())
            .unwrap_or_else(|| UNKNOWN.into()),
        plan.system.boot_size_mib.unwrap_or(512),
        plan.system.root_filesystem.as_str(),
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
        plan.system.profile.as_str(),
        plan.system.with_tools,
        plan.system.keep_docs,
        plan.system.keep_package_cache
    );

    format!("{target}{}", planned_network(plan, manifest))
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
    fn failed_preflight_keeps_fixed_gentoo_and_ext4_target_settings_visible() {
        let mut plan = InstallPlan::new(crate::model::Target::Gentoo);
        plan.system.root_filesystem = crate::config::RootFilesystem::Ext4;
        plan.username = Some("administrator".into());
        let target = planned_target(&plan, None);
        assert!(target.contains("amd64-nomultilib-openrc"));
        assert!(target.contains("no-multilib (64-bit only)"));
        assert!(target.contains("sys-kernel/gentoo-kernel-bin"));
        assert!(target.contains("Init: OpenRC"));
        assert!(target.contains("Unknown (unverified)"));
        assert!(target.contains("at /boot"));
        assert!(target.contains("Root: ext4"));
        assert!(target.contains("source policy unverified"));
        assert!(target.contains("SSH port:"));
        assert!(target.contains("SSH public keys only"));
        assert!(target.contains("Root SSH login: disabled"));
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
    #[test]
    fn kernel_command_line_redacts_unknown_values_and_quoted_credentials() {
        let cmdline = "root=UUID=private-id rootfstype=btrfs ro quiet console=ttyS0,115200n8 secret=\"private password\" access_token=abc123 rd.luks.key=/secret.key init=\"/bin/sh -c hidden-command\"";
        let text = redacted_command_line(cmdline);
        assert!(text.contains("rootfstype=btrfs ro quiet console=ttyS0,115200n8"));
        for secret in [
            "private-id",
            "private password",
            "abc123",
            "/secret.key",
            "hidden-command",
        ] {
            assert!(!text.contains(secret), "leaked {secret}");
        }
        assert!(text.contains("access_token=[redacted]"));
        assert!(redacted_command_line("unknown='unclosed").contains("malformed"));
        assert_eq!(redacted_command_line("standalone-secret"), "[redacted]");
    }
    #[test]
    fn load_and_service_manager_are_optional_and_not_inferred_from_installed_packages() {
        assert_eq!(
            load_average("1.50 0.25 2.0 3/120 15"),
            (Some([1.5, 0.25, 2.0]), Some(3), Some(120))
        );
        assert_eq!(load_average("nan -1 inf 8/2 15"), (None, None, None));
        assert_eq!(load_average("broken"), (None, None, None));
        let root = Scratch::new("inspect-service-manager");
        root.write("proc/1/comm", "init\n");
        fs::create_dir_all(root.0.join("etc/init.d")).unwrap();
        assert!(service_manager(&root.0).starts_with("Unknown"));
        root.write("run/openrc/softlevel", "default\n");
        assert_eq!(
            service_manager(&root.0),
            "OpenRC (runtime runlevel default)"
        );
    }
    #[test]
    fn hugepages_numa_and_thp_use_kernel_counts_and_reject_overflow() {
        let root = Scratch::new("inspect-huge-nodes");
        root.write("proc/meminfo","MemTotal: 8192 kB\nMemAvailable: 4096 kB\nHugePages_Total: 4\nHugePages_Free: 2\nHugePages_Rsvd: 1\nHugePages_Surp: 0\nHugepagesize: 2048 kB\nAnonHugePages: 4096 kB\nShmemHugePages: 0 kB\n");
        root.write(
            "sys/devices/system/node/node0/meminfo",
            "Node 0 MemTotal: 8192 kB\nNode 0 MemFree: 1024 kB\n",
        );
        root.write("sys/devices/system/node/node0/cpulist", "0-3\n");
        root.write(
            "sys/kernel/mm/transparent_hugepage/enabled",
            "always [madvise] never\n",
        );
        let memory = memory(&root.0);
        assert_eq!(memory.hugepages.total, Some(4));
        assert_eq!(memory.hugepages.page_size, Some(2 * 1024 * 1024));
        assert_eq!(memory.hugepages.anonymous_bytes, Some(4 * 1024 * 1024));
        assert_eq!(memory.transparent_hugepages, "madvise");
        assert_eq!(memory.numa_nodes[0].cpus, "0-3");
        assert_eq!(memory.numa_nodes[0].total, Some(8192 * 1024));
        root.write(
            "sys/devices/system/node/node0/meminfo",
            "Node 0 MemTotal: 18446744073709551615 kB\n",
        );
        assert_eq!(numa(&root.0)[0].total, None);
    }
    #[test]
    fn security_reports_kernel_observations_without_probing_or_assuming_missing_means_disabled() {
        let root = Scratch::new("inspect-security");
        root.write(
            "sys/kernel/security/lockdown",
            "none [integrity] confidentiality\n",
        );
        root.write(
            "sys/kernel/security/lsm",
            "lockdown,capability,yama,apparmor\n",
        );
        root.write(
            "sys/devices/system/cpu/vulnerabilities/spectre_v2",
            "Mitigation: Enhanced IBRS\n",
        );
        root.write("proc/sys/kernel/tainted", "512\n");
        root.write("proc/sys/kernel/kexec_load_disabled", "1\n");
        root.write("sys/module/apparmor/parameters/enabled", "Y\n");
        let facts = security(&root.0);
        assert_eq!(facts.lockdown, "integrity");
        assert_eq!(facts.tainted, Some(512));
        assert_eq!(facts.kexec_disabled, Some(true));
        assert_eq!(facts.selinux, UNKNOWN);
        assert_eq!(facts.apparmor, "Enabled");
        assert_eq!(
            facts.vulnerabilities,
            [("spectre_v2".into(), "Mitigation: Enhanced IBRS".into())]
        );
    }
    #[test]
    fn network_diagnostics_only_render_qdisc_kinds_and_firewall_policies() {
        assert_eq!(
            qdiscs(
                "qdisc fq_codel 0: dev eth0 root refcnt 2 limit 10240p\nqdisc mq 0: dev eth1 root\nmalformed"
            )["eth0"],
            ["fq_codel"]
        );
        assert!(qdiscs("qdisc broken 0: dev ../../bad root").is_empty());
        let mut runner = MockRunner::new([
            output("table inet filter\ntable ip nat\n"),
            output(
                "-P INPUT DROP\n-P FORWARD DROP\n-P OUTPUT ACCEPT\n-A INPUT -m comment --comment private-firewall-comment -j ACCEPT\n",
            ),
            output("-P INPUT ACCEPT\n"),
        ]);
        let facts = firewall(&mut runner);
        assert_eq!(facts.nftables_table_count, Some(2));
        assert_eq!(facts.ipv4_policies["INPUT"], "DROP");
        assert!(!policy_summary(&facts.ipv4_policies).contains("private"));
        assert_eq!(runner.calls()[0].args, ["list", "tables"]);
        assert_eq!(runner.calls()[1].args, ["-S"]);
        let mut runner = MockRunner::new([output(""), output(""), output("")]);
        assert_eq!(firewall(&mut runner).nftables_table_count, Some(0));
    }
    #[test]
    fn default_report_masks_mac_stable_ids_and_keeps_raw_inventory_local() {
        let root = Scratch::new("inspect-private");
        root.write("sys/class/net/eth0/address", "52:54:00:12:34:56\n");
        let mut runner = MockRunner::empty();
        let mut facts = collect(&mut runner, &root.0);
        facts.storage.devices.push(DiskFacts {
            name: "vda".into(),
            kind: "disk".into(),
            identity: "SERIAL-PRIVATE / UUID-PRIVATE".into(),
            ..DiskFacts::default()
        });
        facts.storage.mounts.push(MountFacts {
            path: "/".into(),
            source: "/dev/disk/by-uuid/UUID-PRIVATE".into(),
            filesystem: "ext4".into(),
            options: "rw".into(),
        });
        let shared = facts.render_current();
        for secret in ["52:54:00:12:34:56", "SERIAL-PRIVATE", "UUID-PRIVATE"] {
            assert!(!shared.contains(secret), "shared report leaked {secret}");
        }
        assert!(shared.contains("[redacted]"));
        let full = facts.render_current_with_privacy(InspectionPrivacy::ShowIdentifiers);
        assert!(full.contains("52:54:00:12:34:56"));
        assert!(full.contains("SERIAL-PRIVATE"));
        assert_eq!(facts.network.interfaces[0].mac, "52:54:00:12:34:56");
        assert_eq!(
            private_source(
                "//user:password@example/share",
                InspectionPrivacy::ShowIdentifiers
            ),
            "[network mount credentials redacted]"
        );
    }
    #[test]
    fn nvme_sysfs_and_firmware_version_do_not_read_serials_or_efi_key_databases() {
        let root = Scratch::new("inspect-nvme-firmware");
        root.write("sys/class/nvme/nvme0/model", "Fixture NVMe\n");
        root.write("sys/class/nvme/nvme0/firmware_rev", "1.2\n");
        root.write("sys/class/nvme/nvme0/serial", "private-nvme-serial\n");
        root.write("sys/class/nvme/nvme0/state", "live\n");
        root.write("sys/class/nvme/nvme0/transport", "pcie\n");
        root.write("sys/class/dmi/id/bios_version", "Fixture 1.0\n");
        root.write("sys/class/dmi/id/bios_date", "11/24/2026\n");
        fs::create_dir_all(root.0.join("sys/firmware/efi/efivars")).unwrap();
        root.write(
            "proc/self/mountinfo",
            "30 20 0:5 / /sys/firmware/efi/efivars ro - efivarfs efivarfs ro\n",
        );
        root.write(
            "sys/firmware/efi/efivars/PK-private-key",
            "private-key-database",
        );
        let facts = nvme(&root.0);
        assert_eq!(facts[0].model, "Fixture NVMe");
        assert_eq!(facts[0].firmware, "1.2");
        let firmware = firmware(&root.0, false);
        assert_eq!(firmware.version, "Fixture 1.0");
        assert_eq!(firmware.efivarfs_mounted, Some(true));
        assert_eq!(firmware.variable_store_readable, Some(true));
        assert_eq!(firmware.secure_boot, UNKNOWN);
        assert!(!format!("{facts:?} {firmware:?}").contains("private"));
    }
}
