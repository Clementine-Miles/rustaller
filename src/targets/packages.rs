//! Shared package planning and native package-manager policies.
use crate::command::{
    CommandError, CommandInput, CommandRunner, CommandSpec, OutputMode, run_checked,
};
use crate::config::InstallProfile;
use crate::model::{BootMode, InstallPlan, PlanError, Target};
use crate::postinstall::ConfigurationPlan;
use crate::shared::configure::{
    ConfigFile, publish_files, read_config, replace_managed_block, validate_planned_file,
};
use crate::shared::configure::{ConfigurationError, validate_root};
use crate::worker::live::ValidatedLivePlan;
use std::path::Path;

const BEGIN: &str = "# BEGIN rustaller minimal packages";
const END: &str = "# END rustaller minimal packages";

pub fn configuration_for_plan(
    plan: &InstallPlan,
    target: Target,
    is_virtual_machine: bool,
) -> Result<ConfigurationPlan, PlanError> {
    if plan.target != target || plan.hold == Some(1) {
        return Err(PlanError::UnexpectedOption("automatic-install"));
    }
    plan.validate()?;
    let mut configuration = plan
        .post_install
        .prepare(target, plan.no_cloud_kernel, is_virtual_machine)
        .map_err(PlanError::PostInstall)?;
    for package in plan
        .system
        .required_packages(target)
        .iter()
        .chain(plan.system.tool_packages(target))
    {
        if !configuration.extra_packages.contains(package) {
            configuration.extra_packages.push(package);
        }
    }
    Ok(configuration)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootfsMethod {
    AlpineApk,
    DebianMinbase,
    ArchPacstrap,
    GentooNomultilibStage3,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ProfilePackagePlan {
    pub method: RootfsMethod,
    pub bootstrap_packages: Vec<String>,
    pub later_packages: Vec<String>,
    pub strip_docs: bool,
    pub clear_package_cache: bool,
}

pub fn package_plan(
    plan: &InstallPlan,
    boot_mode: BootMode,
    is_virtual_machine: bool,
    firmware_packages: &[String],
) -> Result<ProfilePackagePlan, PlanError> {
    if !plan.target.is_automatic() {
        return Err(PlanError::UnexpectedOption("automatic-install"));
    }
    let configuration = configuration_for_plan(plan, plan.target, is_virtual_machine)?;
    if plan
        .force_boot_mode
        .is_some_and(|expected| expected != boot_mode)
    {
        return Err(PlanError::UnexpectedOption(
            "force-boot-mode does not match detected firmware",
        ));
    }
    let (method, mut bootstrap_packages, mut later_packages) = match plan.target {
        Target::Alpine => {
            let base = [
                "alpine-base",
                "openssh",
                "ca-certificates",
                "dhcpcd",
                "ifupdown-ng",
                // Boot refresh requires the real util-linux mount inspector.
                "findmnt",
                match plan.system.root_filesystem {
                    crate::config::RootFilesystem::Xfs => "xfsprogs",
                    crate::config::RootFilesystem::Ext4 => "e2fsprogs",
                    crate::config::RootFilesystem::Btrfs => "btrfs-progs",
                },
            ]
            .map(str::to_owned)
            .to_vec();
            let mut later = vec![
                configuration.kernel.package.to_owned(),
                "dosfstools".to_owned(),
            ];
            later.extend(firmware_packages.iter().cloned());
            if plan.system.swap.size_mib.is_some() {
                // util-linux swapon provides --show for post-boot diagnostics.
                later.push("util-linux-misc".into());
            }
            if plan.username.as_deref().is_some_and(|user| user != "root") {
                later.push("sudo".into());
            }
            (RootfsMethod::AlpineApk, base, later)
        }
        Target::Debian => (
            RootfsMethod::DebianMinbase,
            vec![],
            super::debian::apt_package_list_for_plan(plan, boot_mode, firmware_packages)?,
        ),
        Target::Arch => {
            let mut base = pacstrap_packages(
                boot_mode,
                plan.system.root_filesystem.as_str(),
                plan.username.as_deref().unwrap_or("root"),
                firmware_packages,
            );
            if plan.system.profile == InstallProfile::Minimal {
                base.push("archlinux-keyring".into());
            }
            (
                RootfsMethod::ArchPacstrap,
                base,
                vec![configuration.kernel.package.into(), "mkinitcpio".into()],
            )
        }
        Target::Gentoo => (
            RootfsMethod::GentooNomultilibStage3,
            vec![],
            emerge_package_set_for_plan(
                plan,
                boot_mode,
                plan.system.root_filesystem.as_str(),
                firmware_packages,
            )?,
        ),
        _ => unreachable!(),
    };
    if matches!(plan.target, Target::Alpine | Target::Arch) {
        for package in configuration.extra_packages {
            if !later_packages.iter().any(|old| old == package)
                && !bootstrap_packages.iter().any(|old| old == package)
            {
                later_packages.push(package.into());
            }
        }
    }
    // Reporting duplicate requirements is misleading; preserve the first order.
    let mut seen = std::collections::BTreeSet::new();
    bootstrap_packages.retain(|package| seen.insert(package.clone()));
    later_packages.retain(|package| seen.insert(package.clone()));
    Ok(ProfilePackagePlan {
        method,
        bootstrap_packages,
        later_packages,
        strip_docs: plan.system.strip_docs(),
        clear_package_cache: plan.system.clear_package_cache(),
    })
}

pub fn pacstrap_packages(
    _boot_mode: BootMode,
    root_filesystem: &str,
    username: &str,
    firmware_packages: &[String],
) -> Vec<String> {
    let mut packages = vec!["base".into(), "dosfstools".into(), "openssh".into()];
    match root_filesystem {
        "xfs" => packages.push("xfsprogs".into()),
        "ext4" => packages.push("e2fsprogs".into()),
        "btrfs" => packages.push("btrfs-progs".into()),
        _ => {}
    }
    if username != "root" {
        packages.push("sudo".into());
    }
    packages.extend(firmware_packages.iter().cloned());
    packages
}

/// Install the kernel after locale/firmware setup, as in the preserved flow.
/// In particular, this does not add the kernel to the earlier pacstrap phase.
pub fn kernel_install_command_for_plan(
    plan: &InstallPlan,
    root: &Path,
) -> Result<CommandSpec, PlanError> {
    let configuration = configuration_for_plan(plan, Target::Arch, false)?;
    let mut args = vec![
        root.as_os_str().to_owned(),
        "pacman".into(),
        "-Syu".into(),
        "--noconfirm".into(),
        configuration.kernel.package.into(),
        "mkinitcpio".into(),
    ];
    args.extend(configuration.extra_packages.into_iter().map(Into::into));
    Ok(CommandSpec::new("chroot", args)
        .phase("arch-kernel-install")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream))
}

/// Native repository resolution runs inside the target before the kernel
/// transaction. It checks the selected package and dependency closure without
/// installing a second kernel or relying on a hard-coded release availability.
/// The target adapters must first initialize their authenticated repositories.
pub fn verify_kernel_available<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &InstallPlan,
) -> Result<(), KernelAvailabilityError> {
    let configuration =
        configuration_for_plan(plan, plan.target, false).map_err(KernelAvailabilityError::Plan)?;
    let mut args = vec![root.as_os_str().to_owned()];
    let native: Vec<&str> = match plan.target {
        Target::Alpine => vec![
            "apk",
            "add",
            "--simulate",
            "--no-cache",
            configuration.kernel.package,
        ],
        Target::Arch => vec![
            "pacman",
            "-Syp",
            "--noconfirm",
            "--print-format",
            "%n",
            configuration.kernel.package,
            "mkinitcpio",
        ],
        Target::Gentoo => vec![
            "emerge",
            "--pretend",
            "--getbinpkg",
            "--usepkg",
            "--noreplace",
            configuration.kernel.package,
            "sys-kernel/dracut",
            "sys-kernel/installkernel",
        ],
        Target::Debian => vec![
            "apt-get",
            "--simulate",
            "install",
            "--no-install-recommends",
            configuration.kernel.package,
            "initramfs-tools",
        ],
        Target::Reset => {
            return Err(KernelAvailabilityError::Plan(PlanError::UnexpectedOption(
                "automatic-install",
            )));
        }
    };
    args.extend(native.into_iter().map(Into::into));
    run_checked(
        runner,
        &crate::command::target_command("chroot", args, "target-kernel-availability")
            .env("LC_ALL", "C")
            .env("DEBIAN_FRONTEND", "noninteractive"),
    )
    .map_err(KernelAvailabilityError::Command)
}

#[derive(Debug)]
pub enum KernelAvailabilityError {
    Plan(PlanError),
    Command(CommandError),
}
impl std::fmt::Display for KernelAvailabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plan(error) => error.fmt(f),
            Self::Command(error) => write!(
                f,
                "Selected target kernel package or dependencies are unavailable: {error}"
            ),
        }
    }
}
impl std::error::Error for KernelAvailabilityError {}

pub fn emerge_package_set(
    _boot_mode: BootMode,
    root_filesystem: &str,
    username: &str,
    firmware_packages: &[String],
) -> Vec<String> {
    let mut packages = vec![
        "sys-block/io-scheduler-udev-rules".to_owned(),
        "sys-fs/dosfstools".into(),
    ];
    match root_filesystem {
        "xfs" => packages.push("sys-fs/xfsprogs".to_owned()),
        "ext4" => packages.push("sys-fs/e2fsprogs".to_owned()),
        "btrfs" => packages.push("sys-fs/btrfs-progs".to_owned()),
        _ => {}
    }
    if username != "root" {
        packages.push("app-admin/sudo".to_owned());
    }
    packages.extend(firmware_packages.iter().cloned());
    packages.push("sys-kernel/gentoo-kernel-bin".to_owned());
    packages
}

pub fn emerge_package_set_for_plan(
    plan: &InstallPlan,
    boot_mode: BootMode,
    root_filesystem: &str,
    firmware_packages: &[String],
) -> Result<Vec<String>, PlanError> {
    let configuration = configuration_for_plan(plan, Target::Gentoo, false)?;
    let mut packages = emerge_package_set(
        boot_mode,
        root_filesystem,
        plan.username.as_deref().unwrap_or("root"),
        firmware_packages,
    );
    packages.extend(configuration.extra_packages.into_iter().map(str::to_owned));
    packages.extend(
        ["sys-kernel/dracut", "sys-kernel/installkernel"]
            .into_iter()
            .map(str::to_owned),
    );
    if plan.system.gentoo_init == Some(crate::config::GentooInit::Openrc) {
        packages.extend(
            [
                "sys-apps/openrc",
                "net-misc/netifrc",
                "net-misc/dhcpcd",
                "net-dns/openresolv",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    Ok(packages)
}

/// Verify every installed kernel has the selected filesystem and storage/network
/// drivers needed by this guest. This uses authenticated package-owned module
/// inventories, not the capabilities of the Alpine Live kernel.
pub fn verify_boot_driver_support(
    root: &Path,
    filesystem: &str,
    disk_device: &str,
    interfaces: &[crate::deployment::NetworkInterface],
) -> Result<(), String> {
    use std::collections::BTreeSet;
    use std::fs;
    validate_root(root).map_err(|e| e.to_string())?;
    if !matches!(filesystem, "xfs" | "ext4" | "btrfs") {
        return Err("unsupported target root filesystem".into());
    }
    let mut required = BTreeSet::from([filesystem.to_owned(), "vfat".to_owned()]);
    let disk = Path::new(disk_device)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid target disk name")?;
    let storage = visible_device_modules(&Path::new("/sys/class/block").join(disk));
    required.extend(storage.iter().cloned());
    // Standard guest device names still establish the frontend when optional
    // sysfs driver/module links are absent (including built-in Live drivers).
    let needs_scsi = disk.starts_with("sd");
    if disk.starts_with("vd") {
        required.insert("virtio_blk".into());
    } else if disk.starts_with("xvd") {
        required.insert("xen_blkfront".into());
    } else if disk.starts_with("nvme") {
        required.insert("nvme".into());
    } else if needs_scsi {
        required.insert("sd_mod".into());
    }
    for interface in interfaces {
        if !crate::deployment::valid_interface_name(&interface.name) {
            return Err("invalid target network interface".into());
        }
        required.extend(visible_device_modules(
            &Path::new("/sys/class/net").join(&interface.name),
        ));
    }
    let canonical_root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let modules = fs::canonicalize(root.join("lib/modules"))
        .map_err(|_| "target kernel module inventory is missing")?;
    if !modules.starts_with(&canonical_root) {
        return Err("kernel modules escape target root".into());
    }
    let releases = fs::read_dir(&modules)
        .map_err(|e| e.to_string())?
        .take(17)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if releases.is_empty() || releases.len() > 16 {
        return Err("require 1-16 installed kernel module releases".into());
    }
    let mut checked_releases = 0;
    for release in releases {
        if !release.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        checked_releases += 1;
        let mut present = BTreeSet::new();
        for (file, dependency) in [("modules.builtin", false), ("modules.dep", true)] {
            let path = release.path().join(file);
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| format!("missing {file} for installed kernel"))?;
            if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
                return Err("invalid kernel module inventory".into());
            }
            let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            for line in text.lines().filter(|line| !line.is_empty()) {
                let path = if dependency {
                    line.split_once(':').ok_or("invalid modules.dep")?.0
                } else {
                    line
                };
                let path = Path::new(path);
                if path
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
                {
                    return Err("unsafe module inventory path".into());
                }
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    return Err("invalid module inventory filename".into());
                };
                let name = name
                    .strip_suffix(".xz")
                    .or_else(|| name.strip_suffix(".zst"))
                    .or_else(|| name.strip_suffix(".gz"))
                    .unwrap_or(name);
                let Some(name) = name.strip_suffix(".ko") else {
                    return Err("invalid module inventory extension".into());
                };
                let name = name.replace('-', "_");
                // Built-ins need no external payload. Required loadable drivers
                // must actually exist inside the target, not just in a stale depmod file.
                if dependency && required.contains(&name) {
                    let actual = fs::canonicalize(release.path().join(path))
                        .map_err(|_| format!("required module {name} has no payload"))?;
                    if !actual.starts_with(&canonical_root) || !actual.is_file() {
                        return Err("required module escapes target root".into());
                    }
                }
                present.insert(name);
            }
        }
        if let Some(missing) = required.iter().find(|name| !present.contains(*name)) {
            return Err(format!(
                "installed kernel {} lacks required driver {missing}",
                release.file_name().to_string_lossy()
            ));
        }
        // SCSI frontend names do not identify their controller. If sysfs could
        // not identify one, require a supported controller in the installed image.
        if needs_scsi
            && !storage.iter().any(|name| {
                matches!(
                    name.as_str(),
                    "virtio_scsi"
                        | "vmw_pvscsi"
                        | "hv_storvsc"
                        | "ahci"
                        | "mptspi"
                        | "mpt3sas"
                        | "megaraid_sas"
                )
            })
            && ![
                "virtio_scsi",
                "vmw_pvscsi",
                "hv_storvsc",
                "ahci",
                "mptspi",
                "mpt3sas",
                "megaraid_sas",
            ]
            .iter()
            .any(|name| present.contains(*name))
        {
            return Err("installed kernel has no supported SCSI disk controller".into());
        }
    }
    if checked_releases == 0 {
        return Err("target has no installed kernel module releases".into());
    }
    Ok(())
}

fn visible_device_modules(device: &Path) -> std::collections::BTreeSet<String> {
    use std::{collections::BTreeSet, fs};
    let mut modules = BTreeSet::new();
    let Ok(mut device) = fs::canonicalize(device) else {
        return modules;
    };
    for _ in 0..24 {
        if !device.starts_with("/sys/devices") {
            break;
        }
        if let Ok(module) = fs::canonicalize(device.join("driver/module"))
            .or_else(|_| fs::canonicalize(device.join("driver")))
            && let Some(name) = module.file_name().and_then(|name| name.to_str())
            && let name = match name {
                "sd" => "sd_mod".to_owned(),
                "vbd" => "xen_blkfront".to_owned(),
                "vif" => "xen_netfront".to_owned(),
                name => name.replace('-', "_"),
            }
            && matches!(
                name.as_str(),
                "virtio_blk"
                    | "virtio_scsi"
                    | "virtio_pci"
                    | "sd_mod"
                    | "vmw_pvscsi"
                    | "hv_storvsc"
                    | "xen_blkfront"
                    | "nvme"
                    | "nvme_core"
                    | "ahci"
                    | "mptspi"
                    | "mpt3sas"
                    | "megaraid_sas"
                    | "virtio_net"
                    | "xen_netfront"
                    | "hv_netvsc"
                    | "vmxnet3"
                    | "e1000"
                    | "e1000e"
                    | "ena"
                    | "gve"
            )
        {
            modules.insert(name);
        }
        if !device.pop() {
            break;
        }
    }
    modules
}

pub const PRESERVED_ROOTS: &[&str] = &[
    "boot",
    "lib/modules",
    "usr/lib/modules",
    "etc/ssl",
    "usr/share/zoneinfo",
    "usr/share/licenses",
    "usr/share/common-licenses",
    "var/lib/dpkg",
    "var/lib/pacman/local",
    "lib/apk/db",
    "var/db/pkg",
    "var/db/repos",
];

#[derive(Debug, Default, Eq, PartialEq)]
pub struct CleanupPlan {
    pub documentation_roots: Vec<&'static str>,
    pub package_cache_roots: Vec<&'static str>,
}

pub fn cleanup_plan(plan: &ValidatedLivePlan) -> Result<CleanupPlan, ConfigurationError> {
    let plan = plan.plan();
    if plan.hold == Some(1) {
        return Err(ConfigurationError::RescuePlan);
    }
    plan.system
        .validate(plan.target, plan.hold)
        .map_err(ConfigurationError::SystemConfig)?;
    let documentation_roots = if plan.system.strip_docs() {
        vec![
            "usr/share/doc",
            "usr/share/man",
            "usr/share/info",
            "usr/share/locale",
        ]
    } else {
        vec![]
    };
    let package_cache_roots = if plan.system.clear_package_cache() {
        match plan.target {
            Target::Alpine => vec!["var/cache/apk"],
            Target::Debian => vec!["var/cache/apt/archives", "var/lib/apt/lists"],
            Target::Arch => vec!["var/cache/pacman/pkg"],
            Target::Gentoo => vec!["var/cache/distfiles", "var/cache/binpkgs"],
            _ => unreachable!("validated automatic plan"),
        }
    } else {
        vec![]
    };
    Ok(CleanupPlan {
        documentation_roots,
        package_cache_roots,
    })
}

/// Stage target policy before subsequent package installation. Returned cleanup
/// roots are advisory input to a future safe cleanup stage, never rm -rf inputs.
pub fn apply_package_policy(
    root: &Path,
    plan: &ValidatedLivePlan,
) -> Result<CleanupPlan, ConfigurationError> {
    let cleanup = cleanup_plan(plan)?;
    validate_root(root)?;
    let plan = plan.plan();
    if plan.system.profile != InstallProfile::Minimal {
        return Ok(cleanup);
    }
    let strip = plan.system.strip_docs();
    let mut files = Vec::new();
    match plan.target {
        Target::Debian => {
            let path = root.join("etc/dpkg/dpkg.cfg.d/99-rustaller-minimal");
            validate_planned_file(&path)?;
            let contents = if strip {
                "# rustaller minimal; retain copyright and locale aliases\npath-exclude=/usr/share/doc/*\npath-include=/usr/share/doc/*/copyright\npath-exclude=/usr/share/man/*\npath-exclude=/usr/share/info/*\npath-exclude=/usr/share/locale/*\npath-include=/usr/share/locale/locale.alias\n"
            } else {
                "# rustaller: documentation filtering disabled\n"
            };
            files.push(ConfigFile {
                path,
                contents: contents.as_bytes().to_vec(),
            });
        }
        Target::Arch => {
            let path = root.join("etc/pacman.conf");
            validate_planned_file(&path)?;
            let contents = arch_policy(&read_config(&path)?, strip)?;
            files.push(ConfigFile {
                path,
                contents: contents.into_bytes(),
            });
        }
        Target::Gentoo => {
            let path = root.join("etc/portage/make.conf");
            validate_planned_file(&path)?;
            let body = if strip {
                "FEATURES=\"${FEATURES} nodoc noman noinfo\"\n"
            } else {
                ""
            };
            let contents = replace_managed_block(&read_config(&path)?, body, BEGIN, END, false)?;
            if !contents.is_empty() {
                files.push(ConfigFile {
                    path,
                    contents: contents.into_bytes(),
                });
            }
        }
        Target::Alpine => {} // Alpine docs/translations are separately selected packages.
        _ => unreachable!("validated automatic plan"),
    }
    publish_files(files)?;
    Ok(cleanup)
}

/// Reusable for both the Live pacstrap configuration and the installed target.
/// Repositories are preserved, but repository-level signature bypasses fail closed.
pub fn arch_policy(existing: &str, strip_docs: bool) -> Result<String, ConfigurationError> {
    let mut offset = 0;
    let mut section_start = None;
    let mut section_end = existing.len();
    for line in existing.split_inclusive('\n') {
        let text = line.trim();
        if text == "[options]" {
            if section_start.is_some() {
                return Err(ConfigurationError::InvalidConfigFile);
            }
            section_start = Some(offset + line.len());
        } else if text.starts_with('[')
            && text.ends_with(']')
            && section_start.is_some()
            && section_end == existing.len()
        {
            section_end = offset;
        }
        offset += line.len();
    }
    let start = section_start.ok_or(ConfigurationError::InvalidConfigFile)?;
    let body = if strip_docs {
        "SigLevel = Required DatabaseOptional\nNoExtract = usr/share/doc/* usr/share/man/* usr/share/info/* usr/share/locale/*\nNoExtract = !usr/share/doc/*/copyright !usr/share/locale/locale.alias\n"
    } else {
        "SigLevel = Required DatabaseOptional\n"
    };
    for line in existing[section_end..].lines() {
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "SigLevel"
            && value
                .split_whitespace()
                .take_while(|s| !s.starts_with('#'))
                .any(|s| {
                    !matches!(
                        s,
                        "Required"
                            | "PackageRequired"
                            | "DatabaseRequired"
                            | "DatabaseOptional"
                            | "TrustedOnly"
                            | "PackageTrustedOnly"
                            | "DatabaseTrustedOnly"
                    )
                })
        {
            return Err(ConfigurationError::InvalidConfigFile);
        }
    }
    // Remove legacy global assignments before installing the one managed policy.
    let clean = replace_managed_block(&existing[start..section_end], "", BEGIN, END, false)?;
    let clean = clean
        .split_inclusive('\n')
        .filter(|line| {
            line.split_once('=')
                .is_none_or(|(key, _)| key.trim() != "SigLevel")
        })
        .collect::<String>();
    let options = replace_managed_block(&clean, body, BEGIN, END, false)?;
    let mut result = existing[..start].to_owned();
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&options);
    result.push_str(&existing[section_end..]);
    Ok(result)
}
