//! Read-only inspection shared by CLI and TUI; renderers never probe the host.
use crate::{
    command::{CommandRunner, CommandSpec, capture_checked},
    deployment::DeploymentManifest,
    model::{BootMode, InstallPlan},
};
use std::{fs, io::Read, path::Path};
#[derive(Clone, Default)]
pub struct PreflightReport {
    pub current: String,
    pub target: String,
    pub warning: String,
}
impl From<String> for PreflightReport {
    fn from(current: String) -> Self {
        Self {
            current,
            target: "Planned configuration unavailable".into(),
            warning: String::new(),
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
fn read(root: &Path, name: &str) -> String {
    fs::File::open(root.join(name))
        .ok()
        .and_then(|f| {
            let mut text = String::new();
            f.take(1024 * 1024).read_to_string(&mut text).ok()?;
            (text.len() < 1024 * 1024).then_some(text)
        })
        .map(|v| {
            v.chars()
                .filter(|c| !c.is_control() || *c == '\n')
                .collect()
        })
        .unwrap_or_else(|| "unknown".into())
}
fn fact(runner: &mut impl CommandRunner, program: &str, args: &[&str]) -> String {
    capture_checked(
        runner,
        &CommandSpec::new(program, args).phase("host-inspection"),
    )
    .ok()
    .filter(|v| v.len() < 65536)
    .and_then(|v| String::from_utf8(v).ok())
    .map(|s| s.trim().to_owned())
    .filter(|v| !v.is_empty())
    .unwrap_or_else(|| "unknown".into())
}
pub fn report(
    runner: &mut impl CommandRunner,
    root: &Path,
    plan: &InstallPlan,
    manifest: &DeploymentManifest,
) -> PreflightReport {
    let os = read(root, "etc/os-release")
        .lines()
        .find_map(|v| v.strip_prefix("PRETTY_NAME="))
        .unwrap_or("unknown")
        .trim_matches('"')
        .to_owned();
    let cpu = read(root, "proc/cpuinfo");
    let model = cpu
        .lines()
        .find_map(|v| {
            v.strip_prefix("model name")
                .and_then(|v| v.split_once(':'))
                .map(|(_, v)| v.trim())
        })
        .unwrap_or("unknown");
    let count = cpu.lines().filter(|v| v.starts_with("processor")).count();
    let vcpus = if count == 0 {
        "unknown".into()
    } else {
        count.to_string()
    };
    let memory = read(root, "proc/meminfo")
        .lines()
        .filter(|v| v.starts_with("MemTotal:") || v.starts_with("MemAvailable:"))
        .collect::<Vec<_>>()
        .join("; ");
    let memory = if memory.is_empty() {
        "unknown"
    } else {
        &memory
    };
    let environment = fact(runner, "systemd-detect-virt", &[]);
    let geometry = fact(
        runner,
        "lsblk",
        &["-dnro", "LOG-SEC,PTTYPE", "--", &manifest.disk.device],
    );
    let storage = fact(
        runner,
        "lsblk",
        &[
            "-nrpo",
            "NAME,SIZE,TYPE,FSTYPE",
            "--",
            &manifest.disk.device,
        ],
    );
    let resolver = read(root, "etc/resolv.conf")
        .lines()
        .filter(|v| v.starts_with("nameserver ") || v.starts_with("search "))
        .collect::<Vec<_>>()
        .join("; ");
    let resolver = if resolver.is_empty() {
        "unknown"
    } else {
        &resolver
    };
    let ssh = fact(runner, "sshd", &["-T"])
        .lines()
        .filter(|v| {
            [
                "port ",
                "passwordauthentication ",
                "pubkeyauthentication ",
                "permitrootlogin ",
            ]
            .iter()
            .any(|p| v.starts_with(p))
        })
        .collect::<Vec<_>>()
        .join("; ");
    let mut current = format!(
        "OS: {os}\nKernel: {}\nArchitecture: {}\nCPU: {model}\nvCPU: {vcpus}\nMemory: {memory}\nVirtualization: {environment}\nFirmware: {}\nEFI bitness: {}\nSecure Boot: {}\nFirmware vendor (supplementary): {}\nCurrent bootloader: unknown (not inferred from files)\nDisk: {} ({} bytes)\nSerial: {} WWN: {} PTUUID: {}\nSector/table: {geometry}\nExisting layout:\n{storage}\nResolver: {resolver}\nSSH: {}\n",
        read(root, "proc/sys/kernel/osrelease").trim(),
        std::env::consts::ARCH,
        manifest.boot_mode.as_str(),
        if manifest.boot_mode == BootMode::Efi {
            "64 (validated)"
        } else {
            "not applicable"
        },
        if manifest.boot_mode == BootMode::Efi {
            "disabled (validated)"
        } else {
            "not applicable"
        },
        read(root, "sys/class/dmi/id/bios_vendor").trim(),
        manifest.disk.device,
        manifest.disk.size_bytes,
        manifest.disk.serial,
        manifest.disk.wwn,
        manifest.disk.partition_table_id,
        if ssh.is_empty() { "unknown" } else { &ssh }
    );
    for nic in &manifest.network {
        current.push_str(&format!("{} MAC={}\nIPv4: {:?} gateway={:?}\nIPv6: {:?} gateway={:?}\nDHCPv4={} DHCPv6={} RA={}\n",nic.name,nic.mac,nic.ipv4_addresses,nic.ipv4_gateway,nic.ipv6_addresses,nic.ipv6_gateway,nic.dhcp4,nic.dhcp6,nic.accept_ra));
    }
    if plan.hold == Some(1) {
        return PreflightReport {
            current,
            target: "Alpine Live rescue; no target partitioning or bootloader installation".into(),
            warning: format!(
                "RESCUE: interrupts connectivity on {}",
                manifest.disk.device
            ),
        };
    }
    let partitions = crate::shared::storage::planned_partitions(
        manifest.boot_mode,
        manifest.disk.size_bytes,
        plan.system.boot_size_mib.unwrap_or(512),
    );
    let count = partitions.as_ref().map(Vec::len).unwrap_or(0);
    let bios_gpt = count == 3;
    let table = if manifest.boot_mode == BootMode::Efi || bios_gpt {
        "GPT"
    } else {
        "MBR"
    };
    let kernel = crate::targets::packages::configuration_for_plan(plan, plan.target, true)
        .map(|v| v.kernel.package.to_owned())
        .unwrap_or("unknown".into());
    let target = format!(
        "Distribution: {}\nKernel package/family: {kernel} (version determined after installation)\nBootloader: Limine {}\nFirmware: {} (detected)\nPartition table: {table}; count: {}\nFAT32: {} MiB at /efi\nRoot: {} at /\nBtrfs: {}\nSwapfile: {}; swappiness {}\nHostname: {}\nTimezone: {}\nNetwork: inherited validated source policy; MTU {}\nIPv6: {}; address preference: {}\nTCP congestion / queue: {} / {}\nDNS: {} {:?}; search {:?}\nSSH port: {}; account: {}\nPublic keys: {} sources; credentials hidden\nProfile: {}; tools={}; retain docs={}; cache={}\n",
        plan.target,
        crate::targets::limine::VERSION,
        manifest.boot_mode.as_str(),
        count,
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
    PreflightReport {
        current,
        target,
        warning: format!(
            "ERASE ALL PARTITIONS ON {} ({} bytes). Exact disk-identity confirmation required. Limine method: {}.",
            manifest.disk.device,
            manifest.disk.size_bytes,
            if manifest.boot_mode == BootMode::Efi {
                "UEFI fallback EFI/BOOT/BOOTX64.EFI; no NVRAM"
            } else if bios_gpt {
                "BIOS/GPT dedicated stage2 partition 1"
            } else {
                "BIOS/MBR with reserved embedding gap"
            }
        ),
    }
}
