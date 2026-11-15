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
    Command(CommandError),
    Filesystem(io::Error),
}

impl fmt::Display for DebianError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCodename => f.write_str("Invalid Debian stable codename."),
            Self::InvalidArchive => f.write_str("Invalid Debian archive URL."),
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
    ];
    if username != "root" {
        packages.push("sudo".to_owned());
    }
    packages.extend(firmware_packages.iter().cloned());
    packages
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
