use crate::command::{
    CommandError, CommandInput, CommandRunner, CommandSpec, OutputMode, run_checked,
};
use crate::fsutil::replace_config as atomic_replace;
use crate::host::sources::{DEBIAN_ARCHIVE as DEFAULT_ARCHIVE, DEBIAN_SECURITY_ARCHIVE};
use crate::model::{BootMode, InstallPlan, PlanError, Target};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug)]
pub enum DebianError {
    InvalidCodename,
    InvalidArchive,
    UnavailableKernel,
    Command(CommandError),
    Filesystem(io::Error),
}

impl fmt::Display for DebianError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCodename => f.write_str("Invalid Debian stable codename."),
            Self::InvalidArchive => f.write_str("Invalid Debian archive URL."),
            Self::UnavailableKernel => f.write_str("Selected Debian kernel package or initramfs dependencies are unavailable in this release."),
            Self::Command(error) => write!(f, "Debian: {error}"),
            Self::Filesystem(error) => write!(f, "Debian configuration write failed: {error}"),
        }
    }
}

impl std::error::Error for DebianError {}

pub fn debian_kernel_package(no_cloud_kernel: bool) -> &'static str {
    if no_cloud_kernel {
        "linux-image-amd64"
    } else {
        "linux-image-cloud-amd64"
    }
}

pub fn validate_codename(codename: &str) -> Result<(), DebianError> {
    if !codename.is_empty() && codename.bytes().all(|byte| byte.is_ascii_lowercase()) {
        Ok(())
    } else {
        Err(DebianError::InvalidCodename)
    }
}

fn validate_archive(archive: &str) -> Result<(), DebianError> {
    if archive.len() > "https://".len()
        && archive.starts_with("https://")
        && archive
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~:/-".contains(&byte))
    {
        Ok(())
    } else {
        Err(DebianError::InvalidArchive)
    }
}

pub fn apt_sources_text(codename: &str, archive: Option<&str>) -> Result<String, DebianError> {
    validate_codename(codename)?;
    let archive = archive.unwrap_or(DEFAULT_ARCHIVE);
    validate_archive(archive)?;
    Ok(format!(
        "# Debian stable repositories\ndeb {archive} {codename} main contrib non-free non-free-firmware\ndeb {archive} {codename}-updates main contrib non-free non-free-firmware\ndeb {DEBIAN_SECURITY_ARCHIVE} {codename}-security main contrib non-free non-free-firmware\n"
    ))
}

pub fn write_apt_sources(
    root: &Path,
    codename: &str,
    archive: Option<&str>,
) -> Result<(), DebianError> {
    let contents = apt_sources_text(codename, archive)?;
    let apt = root.join("etc/apt");
    let sources = apt.join("sources.list");
    fs::create_dir_all(apt.join("sources.list.d")).map_err(DebianError::Filesystem)?;
    atomic_replace(&sources, contents.as_bytes()).map_err(DebianError::Filesystem)?;
    remove_if_exists(&apt.join("sources.list.d/debian.sources")).map_err(DebianError::Filesystem)
}

pub fn apt_package_list(
    no_cloud_kernel: bool,
    boot_mode: BootMode,
    username: &str,
    firmware_packages: &[String],
) -> Vec<String> {
    apt_package_list_with_kernel(
        debian_kernel_package(no_cloud_kernel),
        boot_mode,
        username,
        firmware_packages,
    )
}

pub fn apt_package_list_for_plan(
    plan: &InstallPlan,
    boot_mode: BootMode,
    firmware_packages: &[String],
) -> Result<Vec<String>, PlanError> {
    let configuration = super::packages::configuration_for_plan(plan, Target::Debian, false)?;
    let mut packages = apt_package_list_with_kernel(
        configuration.kernel.package,
        boot_mode,
        plan.username.as_deref().unwrap_or("root"),
        firmware_packages,
    );
    if plan.system.root_filesystem.as_str() == "btrfs" {
        packages.push("btrfs-progs".into());
    }
    packages.extend(configuration.extra_packages.into_iter().map(str::to_owned));
    Ok(packages)
}

fn apt_package_list_with_kernel(
    kernel_package: &str,
    _boot_mode: BootMode,
    username: &str,
    firmware_packages: &[String],
) -> Vec<String> {
    let mut packages = vec![
        "ca-certificates".to_owned(),
        "systemd-sysv".to_owned(),
        "systemd-resolved".to_owned(),
        "openssh-server".to_owned(),
        "xfsprogs".to_owned(),
        kernel_package.to_owned(),
        "dosfstools".to_owned(),
        "initramfs-tools".to_owned(),
        "iproute2".to_owned(),
        "e2fsprogs".to_owned(),
    ];
    if username != "root" {
        packages.push("sudo".to_owned());
    }
    packages.extend(firmware_packages.iter().cloned());
    packages
}

/// Configure native Debian kernel and initramfs scripts for regular files on
/// FAT32. Preserve unrelated settings; enforce one unambiguous assignment.
pub fn configure_fat32_boot(root: &Path) -> Result<(), DebianError> {
    use crate::shared::configure::{read_config, validate_planned_file, validate_root};
    validate_root(root)
        .map_err(|_| DebianError::Filesystem(io::Error::other("invalid target root")))?;
    let path = root.join("etc/kernel-img.conf");
    validate_planned_file(&path)
        .map_err(|_| DebianError::Filesystem(io::Error::other("unsafe kernel-img.conf")))?;
    let existing = read_config(&path)
        .map_err(|_| DebianError::Filesystem(io::Error::other("invalid kernel-img.conf")))?;
    let mut contents = existing
        .lines()
        .filter(|line| {
            *line != "# Rustaller: FAT32 /boot uses versioned regular files."
                && !line
                    .split_once('=')
                    .is_some_and(|(key, _)| matches!(key.trim(), "do_symlinks" | "link_in_boot"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !contents.is_empty() {
        contents.push('\n');
    }
    contents.push_str("# Rustaller: FAT32 /boot uses versioned regular files.\ndo_symlinks = no\nlink_in_boot = no\n");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(DebianError::Filesystem)?;
    }
    atomic_replace(&path, contents.as_bytes()).map_err(DebianError::Filesystem)
}

pub fn install_apt_packages<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    packages: &[String],
) -> Result<(), DebianError> {
    let update = CommandSpec::new(
        "chroot",
        [
            root.as_os_str(),
            std::ffi::OsStr::new("apt-get"),
            std::ffi::OsStr::new("update"),
        ],
    )
    .phase("debian-apt-update")
    .env("DEBIAN_FRONTEND", "noninteractive")
    .stdin(CommandInput::Null)
    .stdout_mode(OutputMode::Stream)
    .stderr_mode(OutputMode::Stream);
    run_checked(runner, &update).map_err(DebianError::Command)?;

    // Debian's image packages otherwise maintain vmlinuz/initrd symlinks.
    // FAT32 /boot cannot represent them; versioned regular files remain the
    // authoritative package-owned inputs to Rustaller's immutable generations.
    configure_fat32_boot(root)?;
    let selected = packages
        .iter()
        .filter(|name| {
            matches!(
                name.as_str(),
                "linux-image-amd64" | "linux-image-cloud-amd64"
            )
        })
        .collect::<Vec<_>>();
    if selected.len() != 1 {
        return Err(DebianError::UnavailableKernel);
    }
    run_checked(
        runner,
        &CommandSpec::new(
            "chroot",
            [
                root.as_os_str(),
                std::ffi::OsStr::new("apt-get"),
                std::ffi::OsStr::new("--simulate"),
                std::ffi::OsStr::new("install"),
                std::ffi::OsStr::new("--no-install-recommends"),
                std::ffi::OsStr::new(selected[0]),
                std::ffi::OsStr::new("initramfs-tools"),
            ],
        )
        .phase("debian-kernel-availability")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream),
    )
    .map_err(|_| DebianError::UnavailableKernel)?;

    let mut args = vec![
        root.as_os_str().to_owned(),
        "apt-get".into(),
        "install".into(),
        "-y".into(),
        "--no-install-recommends".into(),
    ];
    args.extend(packages.iter().map(Into::into));
    let install = CommandSpec::new("chroot", args)
        .phase("debian-apt-install")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream);
    run_checked(runner, &install).map_err(DebianError::Command)
}

pub fn bootstrap_rootfs<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    codename: &str,
    archive: Option<&str>,
) -> Result<(), DebianError> {
    validate_codename(codename)?;
    let archive = archive.unwrap_or(DEFAULT_ARCHIVE);
    validate_archive(archive)?;
    let apk_add = CommandSpec::new("apk", ["add", "debootstrap"])
        .phase("debian-bootstrap-prerequisite")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream);
    run_checked(runner, &apk_add).map_err(DebianError::Command)?;

    let command = CommandSpec::new(
        "debootstrap",
        [
            "--arch=amd64",
            "--variant=minbase",
            "--include=ca-certificates",
            codename,
        ],
    )
    .phase("debian-bootstrap")
    .cwd("/")
    .stdin(CommandInput::Null)
    .stdout_mode(OutputMode::Stream)
    .stderr_mode(OutputMode::Stream);
    let mut command = command;
    command.args.push(root.as_os_str().to_owned());
    command.args.push(archive.into());
    run_checked(runner, &command).map_err(DebianError::Command)
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
