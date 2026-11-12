//! Create an explicitly requested, inactive swapfile in an isolated XFS/ext4 root.
//! No swapon/swapoff is called: target native boot services activate its fstab.
use super::configure::{ConfigurationError, validate_root};
use super::dns::{read_config, validate_planned_file};
use crate::command::{
    CommandError, CommandInput, CommandRunner, CommandSpec, OutputMode, run_checked,
};
use crate::fsutil::atomic_replace;
use crate::worker::live::ValidatedLivePlan;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
const MIB: u64 = 1024 * 1024;

#[derive(Debug)]
pub enum SwapfileError {
    Configuration(ConfigurationError),
    Command(CommandError),
    Filesystem(io::Error),
    ExistingSwapfile,
    UnsupportedFilesystem,
    InsufficientSpace,
    InvalidInventory,
}
impl std::fmt::Display for SwapfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration(e) => e.fmt(f), Self::Command(e) => e.fmt(f),
            Self::Filesystem(e) => write!(f,"Cannot prepare target swapfile: {e}"),
            Self::ExistingSwapfile => f.write_str("Refusing to overwrite an existing swapfile or fstab entry"),
            Self::UnsupportedFilesystem => f.write_str("Target swapfile requires its own mounted root matching the selected XFS/ext4 filesystem"),
            Self::InsufficientSpace => f.write_str("Target swapfile must leave at least 512 MiB of free disk space"),
            Self::InvalidInventory => f.write_str("Cannot verify target swapfile filesystem/free space"),
        }
    }
}
impl std::error::Error for SwapfileError {}
impl From<ConfigurationError> for SwapfileError {
    fn from(e: ConfigurationError) -> Self {
        Self::Configuration(e)
    }
}
impl From<io::Error> for SwapfileError {
    fn from(e: io::Error) -> Self {
        Self::Filesystem(e)
    }
}
impl From<CommandError> for SwapfileError {
    fn from(e: CommandError) -> Self {
        Self::Command(e)
    }
}

/// Returns true only when a new target file and fstab entry were prepared.
/// Caller owns the mounted root exclusively and must enable native swap boot.
pub fn prepare_swapfile<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
    mut progress: impl FnMut(u32, u32),
) -> Result<bool, SwapfileError> {
    let plan = plan.plan();
    if plan.hold == Some(1) {
        return Err(ConfigurationError::RescuePlan.into());
    }
    plan.system
        .validate(plan.target, plan.hold)
        .map_err(ConfigurationError::SystemConfig)?;
    validate_root(root)?;
    let Some(mib) = plan.system.swap.size_mib else {
        return Ok(false);
    };
    let destination = root.join("swapfile");
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err(SwapfileError::ExistingSwapfile),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let fstab = root.join("etc/fstab");
    validate_planned_file(&fstab)?;
    // fstab's parent must already exist, not be created as part of allocation.
    super::configure::validate_directory_tree(&root.join("etc"))?;
    let mut text = read_config(&fstab)?;
    if text
        .lines()
        .any(|line| line.split_whitespace().next() == Some("/swapfile"))
    {
        return Err(SwapfileError::ExistingSwapfile);
    }
    let swappiness = root.join("etc/sysctl.d/90-rustaller-swap.conf");
    if plan.system.swap.swappiness.is_some() {
        validate_planned_file(&swappiness)?;
    }
    let links = if plan.target == crate::model::Target::Alpine {
        let mut services = vec!["swap"];
        if plan.system.swap.swappiness.is_some() {
            services.push("sysctl");
        }
        super::configure::prepare_openrc(root, &services)?
    } else {
        // fstab's systemd generator handles swap; never accept an explicit mask.
        let target = root.join("etc/systemd/system/swap.target");
        if fs::symlink_metadata(&target).is_ok_and(|m| m.is_symlink()) {
            return Err(ConfigurationError::UnsafePath.into());
        }
        Vec::new()
    };
    let fs_type = capture(
        runner,
        &CommandSpec::new(
            "findmnt",
            [
                std::ffi::OsStr::new("-nro"),
                "FSTYPE".as_ref(),
                "--mountpoint".as_ref(),
                root.as_os_str(),
            ],
        ),
    )?;
    if fs_type.trim() != plan.system.root_filesystem.as_str() {
        return Err(SwapfileError::UnsupportedFilesystem);
    }
    let inventory = capture(
        runner,
        &CommandSpec::new(
            "df",
            [std::ffi::OsStr::new("-Pm"), "--".as_ref(), root.as_os_str()],
        ),
    )?;
    let available: u64 = inventory
        .lines()
        .nth(1)
        .and_then(|line| line.split_whitespace().nth(3))
        .and_then(|s| s.parse().ok())
        .ok_or(SwapfileError::InvalidInventory)?;
    if available < u64::from(mib) + 512 {
        return Err(SwapfileError::InsufficientSpace);
    }
    let temporary = root.join(format!(
        ".rustaller-swap-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut owned = false;
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        owned = true;
        // Live installs GNU coreutils for status=progress. Never use truncate or
        // fallocate: write every byte so swap extents are fully allocated.
        progress(0, mib);
        run_checked(
            runner,
            &crate::command::live_command(
                "dd",
                [
                    "if=/dev/zero".to_owned(),
                    format!("of={}", temporary.display()),
                    "bs=1M".to_owned(),
                    format!("count={mib}"),
                    "conv=notrunc,fsync".to_owned(),
                    "status=progress".to_owned(),
                ],
                "target-swapfile-allocate",
            ),
        )?;
        file.sync_all()?;
        let metadata = file.metadata()?;
        let expected = u64::from(mib) * MIB;
        if metadata.len() != expected || metadata.blocks().saturating_mul(512) < expected {
            return Err(SwapfileError::InvalidInventory);
        }
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        progress(mib, mib);
        run_checked(runner, &CommandSpec::format_target_swapfile(&temporary))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        // hard_link is create-new, unlike rename: never overwrite a late arrival.
        fs::hard_link(&temporary, &destination)?;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("/swapfile none swap defaults 0 0\n");
        if let Err(e) = atomic_replace(&fstab, text.as_bytes(), 0o644) {
            let _ = fs::remove_file(&destination);
            return Err(e.into());
        }
        if let Some(value) = plan.system.swap.swappiness {
            super::dns::publish_files(vec![super::dns::ConfigFile {
                path: swappiness,
                contents: format!("# rustaller target swap\nvm.swappiness = {value}\n")
                    .into_bytes(),
            }])?;
        }
        super::configure::publish_openrc(links)?;
        Ok(true)
    })();
    if owned {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn capture<R: CommandRunner>(runner: &mut R, spec: &CommandSpec) -> Result<String, SwapfileError> {
    let spec = spec
        .clone()
        .phase("target-swapfile-check")
        .env("LC_ALL", "C")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Capture);
    let bytes = crate::command::capture_checked(runner, &spec)?;
    String::from_utf8(bytes).map_err(|_| SwapfileError::InvalidInventory)
}
