//! Apply requested basic settings to an already-installed, caller-owned root.
//!
//! The caller must install `PostInstallConfig::prepare().extra_packages` first,
//! and prevent concurrent changes to the target tree. Validation completes
//! before publication, but replacement of several files is not a transaction:
//! an I/O error during publication can leave some settings already written.

use crate::config::SystemConfigError;
use crate::fsutil::atomic_replace;
use crate::postinstall::PostInstallError;
use crate::worker::live::ValidatedLivePlan;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

const MAX_ZONEFILE_SIZE: u64 = 1024 * 1024;

#[derive(Debug)]
pub enum ConfigurationError {
    InvalidRoot,
    UnsafePath,
    RescuePlan,
    ZonefileOutsideTarget,
    InvalidZonefile,
    Plan(PostInstallError),
    SystemConfig(SystemConfigError),
    MissingNetworkConfig,
    InvalidConfigFile,
    UnsupportedNetworkKernel,
    Filesystem(io::Error),
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot => {
                f.write_str("Target configuration requires an isolated absolute root")
            }
            Self::UnsafePath => {
                f.write_str("Target configuration path is not a safe file or directory")
            }
            Self::RescuePlan => {
                f.write_str("Alpine Live rescue must not configure a target system")
            }
            Self::ZonefileOutsideTarget => {
                f.write_str("Timezone file escapes the target zoneinfo directory")
            }
            Self::InvalidZonefile => f.write_str("Timezone file is not a bounded TZif file"),
            Self::Plan(error) => error.fmt(f),
            Self::SystemConfig(error) => error.fmt(f),
            Self::MissingNetworkConfig => {
                f.write_str("Static DNS requires the generated target systemd-networkd files")
            }
            Self::InvalidConfigFile => f.write_str(
                "Target configuration is oversized, invalid or has ambiguous managed blocks",
            ),
            Self::UnsupportedNetworkKernel => f.write_str("Requested congestion control/qdisc is absent from the installed target kernel configuration"),
            Self::Filesystem(error) => write!(f, "Cannot configure target system: {error}"),
        }
    }
}

impl std::error::Error for ConfigurationError {}

impl From<io::Error> for ConfigurationError {
    fn from(error: io::Error) -> Self {
        Self::Filesystem(error)
    }
}

/// Apply hostname and timezone data without running any host or target command.
/// Kernel installation and GRUB generation remain the target adapter's job.
pub fn apply_basic_configuration(
    root: &Path,
    plan: &ValidatedLivePlan,
    is_virtual_machine: bool,
) -> Result<(), ConfigurationError> {
    let plan = plan.plan();
    if plan.hold == Some(1) {
        return Err(ConfigurationError::RescuePlan);
    }
    let configuration = plan
        .post_install
        .prepare(plan.target, plan.no_cloud_kernel, is_virtual_machine)
        .map_err(ConfigurationError::Plan)?;
    validate_root(root)?;
    let etc = root.join("etc");
    validate_directory_tree(&etc)?;

    let hostname = etc.join("hostname");
    let timezone = etc.join("timezone");
    let localtime = etc.join("localtime");
    if configuration.hostname_contents.is_some() {
        validate_destination(&hostname, false)?;
    }
    let zonefile = if let Some(relative_path) = &configuration.zoneinfo_relative_path {
        validate_destination(&timezone, false)?;
        // Installed systems commonly have /etc/localtime as a symlink. Publish
        // over that link with rename; never read or chmod its target.
        validate_destination(&localtime, true)?;
        Some(read_target_zonefile(root, relative_path)?)
    } else {
        None
    };
    let dns_files = super::dns::prepare_target_dns(root, plan)?;

    // Every path and all zone data are checked before the first config write.
    if let Some(contents) = configuration.hostname_contents {
        atomic_replace(&hostname, contents.as_bytes(), 0o644)?;
    }
    if let (Some(contents), Some(zonefile)) = (configuration.timezone_contents, zonefile) {
        atomic_replace(&timezone, contents.as_bytes(), 0o644)?;
        atomic_replace(&localtime, &zonefile, 0o644)?;
    }
    super::dns::publish_files(dns_files)?;
    Ok(())
}

pub(crate) fn validate_root(root: &Path) -> Result<(), ConfigurationError> {
    if !root.is_absolute()
        || root == Path::new("/")
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(ConfigurationError::InvalidRoot);
    }
    validate_directory_tree(root)
}

pub(crate) fn validate_directory_tree(path: &Path) -> Result<(), ConfigurationError> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        let metadata = fs::symlink_metadata(&prefix)?;
        if !metadata.file_type().is_dir() {
            return Err(ConfigurationError::UnsafePath);
        }
    }
    Ok(())
}

pub(crate) fn validate_destination(
    path: &Path,
    allow_symlink: bool,
) -> Result<(), ConfigurationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.file_type().is_file()
                || (allow_symlink && metadata.file_type().is_symlink()) =>
        {
            Ok(())
        }
        Ok(_) => Err(ConfigurationError::UnsafePath),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn read_target_zonefile(
    root: &Path,
    relative_path: &str,
) -> Result<Vec<u8>, ConfigurationError> {
    let zoneinfo = root.join("usr/share/zoneinfo");
    validate_directory_tree(&zoneinfo)?;
    let zoneinfo = fs::canonicalize(zoneinfo)?;
    let path = fs::canonicalize(root.join(relative_path))?;
    if path == zoneinfo || !path.starts_with(&zoneinfo) {
        return Err(ConfigurationError::ZonefileOutsideTarget);
    }
    // Reject special files before open, so a malformed target cannot make a
    // configuration read block on a FIFO or access a device.
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_ZONEFILE_SIZE {
        return Err(ConfigurationError::InvalidZonefile);
    }
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_ZONEFILE_SIZE {
        return Err(ConfigurationError::InvalidZonefile);
    }
    let mut bytes = Vec::new();
    file.take(MAX_ZONEFILE_SIZE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_ZONEFILE_SIZE || !valid_tzif(&bytes) {
        return Err(ConfigurationError::InvalidZonefile);
    }
    Ok(bytes)
}

fn valid_tzif(bytes: &[u8]) -> bool {
    let Some((version, first_end)) = tzif_block(bytes, 4) else {
        return false;
    };
    if version == 0 {
        return first_end == bytes.len();
    }
    let Some((second_version, second_end)) = tzif_block(&bytes[first_end..], 8) else {
        return false;
    };
    if second_version != version {
        return false;
    }
    // Version 2+ ends with a newline-framed POSIX TZ string, possibly empty.
    // Interpretations of its rules are left to the target's timezone library.
    let footer = &bytes[first_end + second_end..];
    footer.len() >= 2
        && footer.first() == Some(&b'\n')
        && footer.last() == Some(&b'\n')
        && footer[1..footer.len() - 1]
            .iter()
            .all(|byte| (0x20..=0x7e).contains(byte))
}

/// Validate one TZif header and its count-sized data block without allocating.
fn tzif_block(bytes: &[u8], time_width: usize) -> Option<(u8, usize)> {
    let header = bytes.get(..44)?;
    let version = header[4];
    if &header[..4] != b"TZif"
        || !matches!(version, 0 | b'2' | b'3' | b'4')
        || header[5..20].iter().any(|byte| *byte != 0)
    {
        return None;
    }
    let count = |offset: usize| {
        u32::from_be_bytes([
            header[offset],
            header[offset + 1],
            header[offset + 2],
            header[offset + 3],
        ]) as usize
    };
    let utc_count = count(20);
    let standard_count = count(24);
    let leap_count = count(28);
    let transition_count = count(32);
    let type_count = count(36);
    let character_count = count(40);
    if !(1..=256).contains(&type_count)
        || character_count == 0
        || (utc_count != 0 && utc_count != type_count)
        || (standard_count != 0 && standard_count != type_count)
    {
        return None;
    }
    let time_bytes = transition_count.checked_mul(time_width)?;
    let type_bytes = type_count.checked_mul(6)?;
    let leap_bytes = leap_count.checked_mul(time_width.checked_add(4)?)?;
    let block_size = time_bytes
        .checked_add(transition_count)?
        .checked_add(type_bytes)?
        .checked_add(character_count)?
        .checked_add(leap_bytes)?
        .checked_add(standard_count)?
        .checked_add(utc_count)?;
    let end = 44_usize.checked_add(block_size)?;
    let block = bytes.get(44..end)?;

    let mut previous = None;
    for timestamp in block[..time_bytes].chunks_exact(time_width) {
        let timestamp = if time_width == 4 {
            i64::from(i32::from_be_bytes(timestamp.try_into().ok()?))
        } else {
            i64::from_be_bytes(timestamp.try_into().ok()?)
        };
        if previous.is_some_and(|value| value >= timestamp) {
            return None;
        }
        previous = Some(timestamp);
    }
    let types_start = time_bytes + transition_count;
    if block[time_bytes..types_start]
        .iter()
        .any(|index| usize::from(*index) >= type_count)
    {
        return None;
    }
    let names_start = types_start + type_bytes;
    let names_end = names_start + character_count;
    let names = &block[names_start..names_end];
    if names.last() != Some(&0)
        || names
            .iter()
            .any(|byte| *byte != 0 && !(0x20..=0x7e).contains(byte))
    {
        return None;
    }
    for info in block[types_start..names_start].as_chunks::<6>().0 {
        let abbreviation = usize::from(info[5]);
        if info[4] > 1 || abbreviation >= names.len() || !names[abbreviation..].contains(&0) {
            return None;
        }
    }
    let standard_start = names_end + leap_bytes;
    let utc_start = standard_start + standard_count;
    let standard = &block[standard_start..utc_start];
    if standard.iter().any(|flag| *flag > 1) {
        return None;
    }
    for (index, flag) in block[utc_start..].iter().enumerate() {
        if *flag > 1 || (*flag == 1 && standard.get(index) != Some(&1)) {
            return None;
        }
    }
    Some((version, end))
}

// Offline OpenRC service publication, after configuration validation.
use super::dns::{ensure_parent, validate_planned_file, validate_planned_parent};
use std::os::unix::fs::{PermissionsExt, symlink};
pub(crate) struct OpenRcLink {
    path: PathBuf,
    target: PathBuf,
}
pub(crate) fn prepare_openrc(
    root: &Path,
    names: &[&str],
) -> Result<Vec<OpenRcLink>, ConfigurationError> {
    validate_root(root)?;
    let mut links = Vec::new();
    for name in names {
        // This internal API exposes only services actually used by these policies.
        if !matches!(*name, "sysctl" | "modules" | "swap") {
            return Err(ConfigurationError::InvalidConfigFile);
        }
        let init = root.join("etc/init.d").join(name);
        validate_planned_file(&init)?;
        let metadata = fs::symlink_metadata(&init)?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(ConfigurationError::UnsafePath);
        }
        let path = root.join("etc/runlevels/boot").join(name);
        validate_planned_parent(&path)?;
        let target = PathBuf::from(format!("/etc/init.d/{name}"));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_symlink() && fs::read_link(&path)? == target => {}
            Ok(_) => return Err(ConfigurationError::UnsafePath),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                links.push(OpenRcLink { path, target })
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(links)
}
pub(crate) fn publish_openrc(links: Vec<OpenRcLink>) -> Result<(), ConfigurationError> {
    for link in links {
        ensure_parent(&link.path)?;
        symlink(&link.target, &link.path)?;
    }
    Ok(())
}
