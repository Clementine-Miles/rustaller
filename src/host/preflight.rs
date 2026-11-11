use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use crate::model::{BootMode, Target, validate_host_arch, validate_ram};
use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostFacts {
    pub os: String,
    pub architecture: String,
    pub effective_uid: u32,
    pub root_fs_type: String,
    pub in_container: bool,
    pub memory_mb: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreflightError {
    UnsupportedOs,
    UnsupportedArchitecture,
    RootRequired,
    LiveRoot,
    ContainerUnsupported,
    MemoryUndetected,
    ProcUnavailable(&'static str),
    InsufficientRam {
        target: Target,
        required_mb: u64,
        available_mb: u64,
    },
}

impl fmt::Display for PreflightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedOs => f.write_str("Only Linux hosts are supported."),
            Self::UnsupportedArchitecture => f.write_str("Only Linux x86_64 hosts are supported."),
            Self::RootRequired => f.write_str("Please run as root."),
            Self::LiveRoot => {
                f.write_str("Cannot run this script when the root filesystem is tmpfs.")
            }
            Self::ContainerUnsupported => {
                f.write_str("Installation from a container is unsupported.")
            }
            Self::MemoryUndetected => f.write_str("Could not detect RAM size."),
            Self::ProcUnavailable(file) => {
                write!(f, "Cannot read Linux system information: {file}")
            }
            Self::InsufficientRam {
                target,
                required_mb,
                available_mb,
            } => write!(
                f,
                "Could not install {target}: RAM < {required_mb} MB (detected {available_mb} MB)."
            ),
        }
    }
}

impl std::error::Error for PreflightError {}

pub fn validate_host(facts: &HostFacts, target: Target) -> Result<(), PreflightError> {
    validate_host_base(facts)?;

    let required_mb = target.minimum_ram_mb();
    if required_mb == 0 {
        return Ok(());
    }
    let available_mb = facts.memory_mb.ok_or(PreflightError::MemoryUndetected)?;
    validate_ram(target, available_mb).map_err(|error| PreflightError::InsufficientRam {
        target,
        required_mb: error.required_mb,
        available_mb: error.available_mb,
    })
}

fn validate_host_base(facts: &HostFacts) -> Result<(), PreflightError> {
    if facts.os != "linux" {
        return Err(PreflightError::UnsupportedOs);
    }
    validate_host_arch(&facts.architecture).map_err(|_| PreflightError::UnsupportedArchitecture)?;
    if facts.effective_uid != 0 {
        return Err(PreflightError::RootRequired);
    }
    if facts.root_fs_type == "tmpfs" {
        return Err(PreflightError::LiveRoot);
    }
    if facts.in_container {
        return Err(PreflightError::ContainerUnsupported);
    }
    Ok(())
}

pub fn collect_host_facts<R: CommandRunner>(
    runner: &mut R,
    target: Target,
) -> Result<HostFacts, PreflightError> {
    let os = std::env::consts::OS.to_owned();
    if os != "linux" {
        return Err(PreflightError::UnsupportedOs);
    }
    let architecture = std::env::consts::ARCH.to_owned();
    validate_host_arch(&architecture).map_err(|_| PreflightError::UnsupportedArchitecture)?;

    let status = fs::read_to_string("/proc/self/status")
        .map_err(|_| PreflightError::ProcUnavailable("/proc/self/status"))?;
    let uid = effective_uid(&status).ok_or(PreflightError::ProcUnavailable("Uid"))?;
    if uid != 0 {
        return Err(PreflightError::RootRequired);
    }

    let mounts = fs::read_to_string("/proc/mounts")
        .map_err(|_| PreflightError::ProcUnavailable("/proc/mounts"))?;
    let root_fs_type =
        root_filesystem_type(&mounts).ok_or(PreflightError::ProcUnavailable("root mount"))?;
    if root_fs_type == "tmpfs" {
        return Err(PreflightError::LiveRoot);
    }

    let in_container = detect_container(runner);
    if in_container {
        return Err(PreflightError::ContainerUnsupported);
    }

    let meminfo = fs::read_to_string("/proc/meminfo")
        .map_err(|_| PreflightError::ProcUnavailable("/proc/meminfo"))?;
    let memory_mb = if target.minimum_ram_mb() == 0 {
        None
    } else {
        Some(detect_ram_mb(runner, &meminfo).ok_or(PreflightError::MemoryUndetected)?)
    };
    let facts = HostFacts {
        os,
        architecture,
        effective_uid: uid,
        root_fs_type,
        in_container,
        memory_mb,
    };
    validate_host(&facts, target)?;
    Ok(facts)
}

fn detect_container<R: CommandRunner>(runner: &mut R) -> bool {
    if command_available("systemd-detect-virt") {
        let command = CommandSpec::new("systemd-detect-virt", ["-qc"])
            .phase("host-container-check")
            .stdin(CommandInput::Null)
            .stdout_mode(OutputMode::Discard)
            .stderr_mode(OutputMode::Discard);
        if runner
            .run(&command)
            .is_ok_and(|output| output.status == Some(0))
        {
            return true;
        }
    }

    container_markers(Path::new("/"))
}

fn container_markers(root: &Path) -> bool {
    [
        ".dockerenv",
        "run/.containerenv",
        "run/systemd/container",
        "proc/vz",
    ]
    .iter()
    .any(|path| root.join(path).exists())
        || fs::read(root.join("proc/1/environ")).is_ok_and(|data| {
            data.split(|byte| *byte == 0).any(|entry| {
                entry
                    .strip_prefix(b"container=")
                    .is_some_and(|value| !value.is_empty())
            })
        })
}

/// Firmware mode is a property of the running boot, not of the CPU or disk's
/// partition table. Linux x86_64 can run under 32-bit EFI, which cannot execute
/// our BOOTX64.EFI. Used again in Live, where EFI runtime variables may not be
/// accessible after kexec.
pub fn inspect_firmware(root: &Path) -> Result<BootMode, &'static str> {
    if !root.join("sys/firmware/efi").is_dir() {
        return Ok(BootMode::Bios);
    }
    let bits = fs::read_to_string(root.join("sys/firmware/efi/fw_platform_size"))
        .map_err(|_| "cannot establish EFI firmware bitness")?;
    if bits.trim() != "64" {
        return Err("the x86_64 EFI loader requires 64-bit firmware");
    }
    Ok(BootMode::Efi)
}

/// Do not infer disk-boot support from a hypervisor brand. Ordinary KVM,
/// VMware, Hyper-V and Xen HVM still need the normal firmware/storage checks.
pub fn validate_virtualization(root: &Path) -> Result<(), &'static str> {
    if container_markers(root) {
        return Err("containers cannot replace the host kernel or boot disk");
    }
    let xen = root.join("sys/hypervisor");
    if fs::read_to_string(xen.join("type")).is_ok_and(|kind| kind.trim() == "xen")
        || root.join("proc/xen").exists()
    {
        if fs::read_to_string(root.join("proc/xen/capabilities"))
            .is_ok_and(|caps| caps.split_whitespace().any(|cap| cap == "control_d"))
            || fs::read_to_string(xen.join("flags/initdomain")).is_ok_and(|flag| flag.trim() == "1")
        {
            return Err("reinstallation of the Xen control domain is unsupported");
        }
        // Linux publishes PV, PVH or HVM. PV/PVH and unknown Xen boot paths
        // may depend on a provider-supplied kernel instead of our disk GRUB.
        if !fs::read_to_string(xen.join("guest_type")).is_ok_and(|kind| kind.trim() == "HVM") {
            return Err(
                "Xen PV/PVH or unknown guest boot mode requires a provider-specific installer",
            );
        }
    }
    if [
        "dev/sev-guest",
        "dev/tdx_guest",
        "sys/devices/platform/sev-guest",
        "sys/devices/virtual/misc/tdx_guest",
    ]
    .iter()
    .any(|path| root.join(path).exists())
    {
        return Err("confidential-VM kexec is not validated for this installer");
    }
    Ok(())
}

/// Source-side gate, before downloading/staging an installer. Firmware
/// capability is not Secure Boot state: read the actual UEFI global variable.
/// Neither kexec nor --no-nvram bypasses target firmware signature enforcement.
pub fn validate_source_boot(root: &Path) -> Result<BootMode, &'static str> {
    validate_virtualization(root)?;
    let mode = inspect_firmware(root)?;
    if mode == BootMode::Efi {
        let variable =
            root.join("sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c");
        let value = match fs::read(variable) {
            Ok(value) => value,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && efi_variables_mounted(root) =>
            {
                return Ok(mode);
            }
            Err(_) => {
                return Err(
                    "cannot read Secure Boot state; expose EFI variables read-only before retrying",
                );
            }
        };
        match value.as_slice() {
            [_, _, _, _, 0] => {}
            [_, _, _, _, 1] => {
                return Err(
                    "Secure Boot is enabled; the installer does not provide a signed target boot chain",
                );
            }
            _ => return Err("invalid Secure Boot variable; refusing an unknown firmware policy"),
        }
    }
    Ok(mode)
}

fn efi_variables_mounted(root: &Path) -> bool {
    // Linux efi_get_secureboot_mode treats EFI_NOT_FOUND as disabled: firmware
    // built without Secure Boot need not expose the variable. An unmounted
    // efivarfs directory also returns ENOENT, so establish the mount first.
    fs::read_to_string(root.join("proc/self/mountinfo")).is_ok_and(|mounts| {
        mounts.lines().any(|line| {
            line.split_once(" - ").is_some_and(|(mount, fs)| {
                mount.split_whitespace().nth(4) == Some("/sys/firmware/efi/efivars")
                    && fs.split_whitespace().next() == Some("efivarfs")
            })
        })
    }) && fs::read_dir(root.join("sys/firmware/efi/efivars")).is_ok()
}

fn command_available(name: &str) -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|directory| {
        let path = directory.join(name);
        fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    })
}

pub fn detect_ram_mb<R: CommandRunner>(runner: &mut R, proc_meminfo: &str) -> Option<u64> {
    if let Some(output) = capture(runner, "lsmem", &["-b"])
        && let Ok(text) = String::from_utf8(output)
        && let Some(memory) = parse_lsmem_memory(&text)
    {
        return Some(memory);
    }
    if let Some(output) = capture(runner, "dmidecode", &["-t", "17"])
        && let Ok(text) = String::from_utf8(output)
        && let Some(memory) = parse_dmidecode_memory(&text)
    {
        return Some(memory);
    }
    if let Some(output) = capture(runner, "lshw", &["-c", "memory", "-short"])
        && let Ok(text) = String::from_utf8(output)
        && let Some(memory) = parse_lshw_memory(&text)
    {
        return Some(memory);
    }
    parse_memtotal_fallback(proc_meminfo)
}

fn capture<R: CommandRunner>(runner: &mut R, program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let command = CommandSpec::new(program, args)
        .phase("host-ram-detection")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Discard);
    let output = runner.run(&command).ok()?;
    (output.status == Some(0)).then_some(output.stdout)
}

pub fn effective_uid(proc_status: &str) -> Option<u32> {
    let line = proc_status.lines().find(|line| line.starts_with("Uid:"))?;
    line.split_whitespace().nth(2)?.parse().ok()
}

pub fn root_mount_is_tmpfs(proc_mounts: &str) -> bool {
    root_filesystem_type(proc_mounts).as_deref() == Some("tmpfs")
}

fn root_filesystem_type(proc_mounts: &str) -> Option<String> {
    proc_mounts.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        fields.next()?;
        if fields.next()? == "/" {
            Some(fields.next()?.to_owned())
        } else {
            None
        }
    })
}

pub fn parse_lsmem_memory(output: &str) -> Option<u64> {
    const MEBIBYTE: u64 = 1024 * 1024;
    let bytes = output
        .lines()
        .find(|line| line.contains("Total online memory:"))?
        .split_whitespace()
        .last()?
        .parse::<u64>()
        .ok()?;
    if bytes == 0 || bytes % MEBIBYTE != 0 {
        return None;
    }
    Some(bytes / MEBIBYTE)
}

pub fn parse_dmidecode_memory(output: &str) -> Option<u64> {
    let mut total_mb = 0u64;
    for line in output
        .lines()
        .filter(|line| line.contains("Size") && line.contains("B"))
    {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (Some(value), Some(unit)) = (fields.get(fields.len().checked_sub(2)?), fields.last())
        else {
            continue;
        };
        let Ok(value) = value.parse::<u64>() else {
            continue;
        };
        let size_mb = match *unit {
            "GB" => value.checked_mul(1024)?,
            "MB" => value,
            _ => continue,
        };
        total_mb = total_mb.checked_add(size_mb)?;
    }
    (total_mb > 0).then_some(total_mb)
}

pub fn parse_lshw_memory(output: &str) -> Option<u64> {
    let line = output
        .lines()
        .find(|line| line.to_ascii_lowercase().contains("system memory"))?;
    let value = line.split_whitespace().nth(2)?;
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    let amount = digits.parse::<u64>().ok()?;
    if value.contains("GiB") {
        amount.checked_mul(1024)
    } else {
        Some(amount)
    }
}

pub fn parse_memtotal_fallback(proc_meminfo: &str) -> Option<u64> {
    let kb = proc_meminfo
        .lines()
        .find(|line| line.starts_with("MemTotal:"))?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()?;
    (kb / 1024).checked_add(68)
}

// Read-only policy checks for the native kexec handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KexecError {
    ToolMissing,
    KernelUnsupported,
    AdministrativelyDisabled,
    Lockdown,
    UnknownPolicy,
}
impl fmt::Display for KexecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ToolMissing => "One-reboot installation requires the system kexec tool",
            Self::KernelUnsupported => "One-reboot installation requires CONFIG_KEXEC_FILE=y and CONFIG_KEXEC_CORE=y in the running kernel",
            Self::AdministrativelyDisabled => "kexec is administratively disabled; two-reboot fallback is not permitted",
            Self::Lockdown => "Kernel lockdown prevents the requested kexec handoff",
            Self::UnknownPolicy => "Cannot establish kexec policy; do not silently switch to a two-reboot installer",
        })
    }
}
impl std::error::Error for KexecError {}

pub fn validate_one_reboot_support(
    kernel_config: &str,
    load_disabled: &str,
    lockdown: Option<&str>,
    tool_available: bool,
) -> Result<(), KexecError> {
    if !tool_available {
        return Err(KexecError::ToolMissing);
    }
    match load_disabled.trim() {
        "0" => {}
        "1" => return Err(KexecError::AdministrativelyDisabled),
        _ => return Err(KexecError::UnknownPolicy),
    }
    if let Some(lockdown) = lockdown {
        if lockdown
            .split_whitespace()
            .any(|part| matches!(part, "[integrity]" | "[confidentiality]"))
        {
            return Err(KexecError::Lockdown);
        }
        if !lockdown.split_whitespace().any(|part| part == "[none]") {
            return Err(KexecError::UnknownPolicy);
        }
    }
    for symbol in ["CONFIG_KEXEC_FILE", "CONFIG_KEXEC_CORE"] {
        let mut states = kernel_config
            .lines()
            .filter_map(|line| line.split_once('='))
            .filter(|(key, _)| *key == symbol)
            .map(|(_, value)| value);
        if states.next() != Some("y") || states.next().is_some() {
            return Err(KexecError::KernelUnsupported);
        }
    }
    Ok(())
}

#[cfg(test)]
mod boot_environment_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Machine(PathBuf);
    impl Machine {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "rustaller-firmware-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn write(&self, path: &str, bytes: &[u8]) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }
    impl Drop for Machine {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn secure_boot_and_firmware_bitness_are_independent_of_cpu_bitness() {
        let machine = Machine::new();
        assert_eq!(validate_source_boot(&machine.0), Ok(BootMode::Bios));
        machine.write("sys/firmware/efi/fw_platform_size", b"64\n");
        assert!(
            validate_source_boot(&machine.0)
                .unwrap_err()
                .contains("Secure Boot state")
        );
        let variable = "sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";
        for bytes in [
            b"\x07\0\0\0\x01".as_slice(),
            b"\x07\0\0\0\x02",
            b"\0",
            b"\x07\0\0\0\0\0",
        ] {
            machine.write(variable, bytes);
            assert!(validate_source_boot(&machine.0).is_err());
        }
        machine.write(variable, b"\x07\0\0\0\0");
        assert_eq!(validate_source_boot(&machine.0), Ok(BootMode::Efi));
        machine.write("sys/firmware/efi/fw_platform_size", b"32\n");
        assert!(inspect_firmware(&machine.0).is_err());
        machine.write("sys/firmware/efi/fw_platform_size", b"unknown\n");
        assert!(inspect_firmware(&machine.0).is_err());
    }
    #[test]
    fn firmware_without_secure_boot_requires_an_accessible_efivarfs_mount() {
        let machine = Machine::new();
        machine.write("sys/firmware/efi/fw_platform_size", b"64\n");
        machine.write(
            "sys/firmware/efi/efivars/BootCurrent-test",
            b"\x07\0\0\0\x01\0",
        );
        assert!(validate_source_boot(&machine.0).is_err());
        machine.write(
            "proc/self/mountinfo",
            b"1 0 0:1 / /sys/firmware/efi/efivars rw - tmpfs tmpfs rw\n",
        );
        assert!(validate_source_boot(&machine.0).is_err());
        machine.write(
            "proc/self/mountinfo",
            b"1 0 0:1 / /sys/firmware/efi/efivars ro - efivarfs efivarfs ro\n",
        );
        assert_eq!(validate_source_boot(&machine.0), Ok(BootMode::Efi));
        machine.write(
            "sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c",
            b"\x07\0\0\0\x01",
        );
        assert!(
            validate_source_boot(&machine.0)
                .unwrap_err()
                .contains("enabled")
        );
    }
    #[test]
    fn xen_pv_pvh_unknown_and_control_domains_are_not_treated_as_bios_guests() {
        let machine = Machine::new();
        machine.write("sys/hypervisor/type", b"xen\n");
        for kind in ["", "PV", "PVH", "unknown"] {
            machine.write("sys/hypervisor/guest_type", kind.as_bytes());
            assert!(validate_source_boot(&machine.0).is_err());
        }
        machine.write("sys/hypervisor/guest_type", b"HVM\n");
        assert_eq!(validate_source_boot(&machine.0), Ok(BootMode::Bios));
        machine.write("proc/xen/capabilities", b"control_d\n");
        assert!(
            validate_source_boot(&machine.0)
                .unwrap_err()
                .contains("control domain")
        );
    }
    #[test]
    fn container_markers_work_without_systemd_detect_virt_and_cvm_is_rejected() {
        for path in [
            ".dockerenv",
            "run/.containerenv",
            "run/systemd/container",
            "proc/vz",
        ] {
            let machine = Machine::new();
            machine.write(path, b"");
            assert!(
                validate_source_boot(&machine.0)
                    .unwrap_err()
                    .contains("containers")
            );
        }
        let machine = Machine::new();
        machine.write("proc/1/environ", b"PATH=/bin\0container=podman\0");
        assert!(validate_source_boot(&machine.0).is_err());
        machine.write("proc/1/environ", b"PATH=/bin\0container=\0");
        assert!(validate_source_boot(&machine.0).is_ok());
        machine.write("dev/tdx_guest", b"");
        assert!(
            validate_source_boot(&machine.0)
                .unwrap_err()
                .contains("confidential")
        );
    }
}
