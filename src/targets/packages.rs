//! Shared package planning and native package-manager policies.
use crate::command::{CommandInput, CommandSpec, OutputMode};
use crate::config::InstallProfile;
use crate::model::{BootMode, InstallPlan, PlanError, Target};
use crate::postinstall::ConfigurationPlan;
use crate::shared::configure::{ConfigurationError, validate_root};
use crate::shared::dns::{
    ConfigFile, publish_files, read_config, replace_managed_block, validate_planned_file,
};
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
    GentooSystemdStage3,
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
    let boot_mode = plan.force_boot_mode.unwrap_or(boot_mode);
    let (method, mut bootstrap_packages, mut later_packages) = match plan.target {
        Target::Alpine => {
            let base = [
                "alpine-base",
                "openssh",
                "ca-certificates",
                "dhcpcd",
                match plan.system.root_filesystem {
                    crate::config::RootFilesystem::Xfs => "xfsprogs",
                    crate::config::RootFilesystem::Ext4 => "e2fsprogs",
                },
            ]
            .map(str::to_owned)
            .to_vec();
            let mut later = vec![
                configuration.kernel.package.to_owned(),
                if boot_mode == BootMode::Efi {
                    "grub-efi"
                } else {
                    "grub-bios"
                }
                .to_owned(),
            ];
            if boot_mode == BootMode::Efi {
                later.extend(["efibootmgr".into(), "dosfstools".into()]);
            }
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
                vec![configuration.kernel.package.into()],
            )
        }
        Target::Gentoo => (
            RootfsMethod::GentooSystemdStage3,
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
    boot_mode: BootMode,
    root_filesystem: &str,
    username: &str,
    firmware_packages: &[String],
) -> Vec<String> {
    let mut packages = vec!["base".into(), "grub".into(), "openssh".into()];
    if boot_mode == BootMode::Efi {
        packages.extend(["efibootmgr".into(), "dosfstools".into()]);
    }
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
    ];
    args.extend(configuration.extra_packages.into_iter().map(Into::into));
    Ok(CommandSpec::new("chroot", args)
        .phase("arch-kernel-install")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream))
}

pub fn emerge_package_set(
    boot_mode: BootMode,
    root_filesystem: &str,
    username: &str,
    firmware_packages: &[String],
) -> Vec<String> {
    let mut packages = vec!["sys-block/io-scheduler-udev-rules".to_owned()];
    if boot_mode == BootMode::Efi {
        packages.extend([
            "sys-fs/dosfstools".to_owned(),
            "sys-boot/efibootmgr".to_owned(),
        ]);
    }
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
        plan.force_boot_mode.unwrap_or(boot_mode),
        root_filesystem,
        plan.username.as_deref().unwrap_or("root"),
        firmware_packages,
    );
    packages.extend(configuration.extra_packages.into_iter().map(str::to_owned));
    Ok(packages)
}

pub const fn grub_platforms(boot_mode: BootMode) -> &'static str {
    match boot_mode {
        BootMode::Efi => "efi-64",
        BootMode::Bios => "pc",
    }
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
