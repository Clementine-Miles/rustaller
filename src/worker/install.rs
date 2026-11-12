//! Identity-bound Alpine Live entry. Never call this on the source host.
//! There is exactly one final reboot, and only after a verified target install.
use crate::command::{CommandError, CommandRunner, CommandSpec, OutputMode, run_checked};
use crate::deployment::{DeploymentError, DeploymentManifest, DiskIdentity, read_manifest};
use crate::host::handoff::read_disk_identity;
use crate::model::Target;
use crate::plan_file::PrivatePlanFile;
use crate::shared::storage::{DiskLayout, PartitionEnvironment, create_partitions};
use crate::targets::install::{self, InstallError};
use crate::worker::live::{
    LivePreflightError, ValidatedLivePlan, parse_free_memory_mb, validate_live_plan,
};
use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallOutcome {
    Rescue,
    Held,
    Installed,
}
#[derive(Debug)]
pub enum LiveInstallError {
    Invalid(&'static str),
    Filesystem(io::Error),
    Deployment(DeploymentError),
    Preflight(LivePreflightError),
    Command(CommandError),
    Target(InstallError),
    Storage(crate::shared::storage::StorageError),
}
impl std::fmt::Display for LiveInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "Live installation stopped: {reason}"),
            Self::Filesystem(e) => write!(f, "Live installation filesystem error: {e}"),
            Self::Deployment(e) => e.fmt(f),
            Self::Preflight(e) => e.fmt(f),
            Self::Command(e) => e.fmt(f),
            Self::Target(e) => e.fmt(f),
            Self::Storage(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for LiveInstallError {}
impl From<io::Error> for LiveInstallError {
    fn from(e: io::Error) -> Self {
        Self::Filesystem(e)
    }
}
impl From<DeploymentError> for LiveInstallError {
    fn from(e: DeploymentError) -> Self {
        Self::Deployment(e)
    }
}
impl From<LivePreflightError> for LiveInstallError {
    fn from(e: LivePreflightError) -> Self {
        Self::Preflight(e)
    }
}
impl From<CommandError> for LiveInstallError {
    fn from(e: CommandError) -> Self {
        Self::Command(e)
    }
}
impl From<InstallError> for LiveInstallError {
    fn from(e: InstallError) -> Self {
        Self::Target(e)
    }
}
impl From<crate::shared::storage::StorageError> for LiveInstallError {
    fn from(e: crate::shared::storage::StorageError) -> Self {
        Self::Storage(e)
    }
}

pub fn run_from_files<R: CommandRunner>(
    runner: &mut R,
    plan_path: &Path,
    manifest_path: &Path,
) -> Result<InstallOutcome, LiveInstallError> {
    // Validate runtime provenance before even opening the credential-bearing plan.
    if unsafe { geteuid() } != 0 {
        return Err(LiveInstallError::Invalid("root is required"));
    }
    if !Path::new("/etc/alpine-release").is_file() {
        return Err(LiveInstallError::Invalid(
            "the installer must run from verified Alpine Live",
        ));
    }
    let command_line = fs::read_to_string("/proc/cmdline")?;
    if !command_line
        .split_whitespace()
        .any(|arg| arg == "rustaller_live=1")
    {
        return Err(LiveInstallError::Invalid(
            "missing private kexec Live boot marker",
        ));
    }
    let filesystem = capture(
        runner,
        "findmnt",
        ["-nro", "FSTYPE", "--target", "/"],
        "live-verify-memory-root",
    )?;
    validate_live_root(filesystem.trim())?;
    let architecture = capture(runner, "uname", ["-m"], "live-verify-architecture")?;
    let memory = capture(runner, "free", ["-m"], "live-check-ram-before-prepare")?;
    let mut plan_file = PrivatePlanFile::open(plan_path)
        .map_err(|_| LiveInstallError::Invalid("private installation plan is not readable"))?;
    let plan = plan_file
        .read()
        .map_err(|_| LiveInstallError::Invalid("private installation plan is invalid"))?;
    plan_file.persist();
    let manifest = read_manifest(manifest_path)?;
    let plan = validate_live_plan(
        plan,
        architecture.trim(),
        parse_free_memory_mb(&memory)?,
        crate::RUNTIME_VERSION,
    )?;
    install::validate_install_options(&plan, &manifest)?;
    announce(&format!(
        "RUSTALLER_LIVE_START target={} firmware={}",
        plan.plan().target,
        manifest.boot_mode.as_str()
    ));
    // openssh is in the verified overlay APK world, so recovery must become
    // available before any long package/stage3 download can fail.
    install::start_recovery_ssh(runner, &plan)?;
    let work = Path::new("/run/rustaller");
    DirBuilder::new().mode(0o700).create(work)?;
    let _lock = InstallLock::create(work)?;
    let assets = install::prepare(runner, &plan, &manifest, work)?;
    install::start_live_services(runner, &plan, work)?;
    if plan.plan().hold == Some(1) {
        announce("Alpine Live rescue is ready. No disk changes were made.");
        return Ok(InstallOutcome::Rescue);
    }
    let disk = select_bound_disk(runner, &manifest.disk)?;
    verify_unmounted_disk(runner, &disk.device)?;
    crate::host::preflight::validate_virtualization(Path::new("/"))
        .map_err(LiveInstallError::Invalid)?;
    let firmware = crate::host::preflight::inspect_firmware(Path::new("/"))
        .map_err(LiveInstallError::Invalid)?;
    if firmware != manifest.boot_mode {
        return Err(LiveInstallError::Invalid(
            "firmware mode changed across kexec",
        ));
    }
    // Rebind only after every supplied stable identity and the byte size match.
    let mut rebound = crate::codec::decode_plan(
        &crate::codec::encode_plan(plan.plan())
            .map_err(|_| LiveInstallError::Invalid("cannot copy validated plan"))?,
    )
    .map_err(|_| LiveInstallError::Invalid("cannot copy validated plan"))?;
    rebound.target_disk = Some(disk.device.clone());
    let memory = capture(runner, "free", ["-m"], "live-check-ram-before-wipe")?;
    let rebound = validate_live_plan(
        rebound,
        architecture.trim(),
        parse_free_memory_mb(&memory)?,
        crate::RUNTIME_VERSION,
    )?;
    install::validate_install_options(&rebound, &manifest)?;
    // Re-read identity and mount/swap inventory at the destructive boundary.
    let current = read_disk_identity(runner, &disk.device)
        .map_err(|_| LiveInstallError::Invalid("cannot revalidate the selected block device"))?;
    if !same_physical_disk(&manifest.disk, &current) {
        return Err(LiveInstallError::Invalid(
            "target disk identity changed before partitioning",
        ));
    }
    verify_unmounted_disk(runner, &disk.device)?;
    let root = Path::new("/os");
    crate::shared::storage::validate_disk_geometry(runner, &disk.device, firmware)?;
    if fs::symlink_metadata(root).is_ok() {
        return Err(LiveInstallError::Invalid(
            "the target mount directory already exists",
        ));
    }
    let name = disk
        .device
        .strip_prefix("/dev/")
        .ok_or(LiveInstallError::Invalid("invalid bound whole-disk path"))?;
    let layout = DiskLayout::new(&rebound, name, manifest.boot_mode, disk.size_bytes)?;
    let environment = PartitionEnvironment {
        has_partprobe: program_exists("partprobe"),
        has_partx: program_exists("partx"),
        device_links: Path::new("/dev/disk")
            .is_dir()
            .then(|| PathBuf::from("/dev/disk")),
    };
    create_partitions(runner, &layout, &environment)?;
    if let Err(error) = crate::shared::storage::mount_target(runner, &layout, root) {
        let _ = unmount_target(runner, root, &layout);
        return Err(error.into());
    }
    let result = install_mounted(runner, root, &rebound, &manifest, &assets, &layout);
    if let Err(error) = result {
        // Unmount only our own target tree. Never reboot an incomplete install.
        let _ = unmount_target(runner, root, &layout);
        announce(
            "Installation failed. Alpine Live and recovery SSH remain running; no final reboot was requested.",
        );
        return Err(error);
    }
    unmount_target(runner, root, &layout)?;
    checked(runner, "sync", [] as [&str; 0], "live-final-sync")?;
    announce(&format!(
        "RUSTALLER_INSTALL_COMPLETE target={} firmware={}",
        rebound.plan().target,
        layout.boot_mode().as_str()
    ));
    if rebound.plan().hold == Some(2) {
        announce("Installation verified. Reboot manually to enter the new system.");
        return Ok(InstallOutcome::Held);
    }
    checked(runner, "reboot", [] as [&str; 0], "live-one-final-reboot")?;
    Ok(InstallOutcome::Installed)
}

/// The signed modloop is embedded in the private RAM overlay. Alpine's stock
/// modloop service searches mounted boot media and excludes /, so it cannot
/// discover this file. This entry is used only by our Live sysinit service.
pub fn prepare_live_modules<R: CommandRunner>(runner: &mut R) -> Result<(), LiveInstallError> {
    if unsafe { geteuid() } != 0
        || !Path::new("/etc/alpine-release").is_file()
        || !fs::read_to_string("/proc/cmdline")?
            .split_whitespace()
            .any(|v| v == "rustaller_live=1")
    {
        return Err(LiveInstallError::Invalid(
            "module setup requires the private Alpine Live environment",
        ));
    }
    let filesystem = capture(
        runner,
        "findmnt",
        ["-nro", "FSTYPE", "--target", "/"],
        "live-modules-root",
    )?;
    validate_live_root(filesystem.trim())?;
    let payload = fs::symlink_metadata("/modloop-virt")?;
    if !payload.is_file() || payload.len() == 0 {
        return Err(LiveInstallError::Invalid(
            "verified Live module payload is missing",
        ));
    }
    let version = capture(runner, "uname", ["-r"], "live-modules-kernel")?;
    let version = version.trim();
    if version.is_empty()
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
    {
        return Err(LiveInstallError::Invalid("invalid Live kernel release"));
    }
    fs::create_dir_all("/.modloop")?;
    checked(
        runner,
        "mount",
        [
            "-t",
            "squashfs",
            "-o",
            "loop,ro",
            "/modloop-virt",
            "/.modloop",
        ],
        "live-mount-verified-modules",
    )?;
    if !Path::new("/.modloop/modules").join(version).is_dir() {
        return Err(LiveInstallError::Invalid(
            "Live kernel and verified module payload do not match",
        ));
    }
    match fs::symlink_metadata("/lib/modules") {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all("/lib/modules")?,
        Ok(_) => fs::remove_file("/lib/modules")?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    std::os::unix::fs::symlink("/.modloop/modules", "/lib/modules")?;
    fs::create_dir_all("/lib/firmware")?;
    if let Ok(entries) = fs::read_dir("/.modloop/modules/firmware") {
        for entry in entries {
            let entry = entry?;
            let destination = Path::new("/lib/firmware").join(entry.file_name());
            if fs::symlink_metadata(&destination).is_err() {
                std::os::unix::fs::symlink(entry.path(), destination)?;
            }
        }
    }
    Ok(())
}

fn install_mounted<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
    manifest: &DeploymentManifest,
    assets: &install::PreparedAssets,
    layout: &DiskLayout,
) -> Result<(), LiveInstallError> {
    install::bootstrap(runner, root, plan, manifest, assets, layout)?;
    mount_pseudo(runner, root)?;
    let root_uuid = filesystem_uuid(runner, &layout.root_partition())?;
    let efi_uuid = layout
        .efi_partition()
        .map(|device| filesystem_uuid(runner, &device))
        .transpose()?;
    install::configure(
        runner,
        root,
        plan,
        manifest,
        assets,
        layout,
        &root_uuid,
        efi_uuid.as_deref(),
    )?;
    Ok(())
}

/// Factored pure checks are used by mocks; production still establishes all
/// observations itself and does not accept a CLI bypass for these guards.
pub fn validate_live_root(filesystem: &str) -> Result<(), LiveInstallError> {
    if matches!(filesystem, "tmpfs" | "rootfs" | "ramfs") {
        Ok(())
    } else {
        Err(LiveInstallError::Invalid(
            "the running root is not an in-memory Live filesystem",
        ))
    }
}
pub fn same_physical_disk(expected: &DiskIdentity, found: &DiskIdentity) -> bool {
    expected.size_bytes == found.size_bytes
        && (!expected.serial.is_empty()
            || !expected.wwn.is_empty()
            || !expected.partition_table_id.is_empty())
        && (expected.serial.is_empty() || expected.serial == found.serial)
        && (expected.wwn.is_empty() || expected.wwn.eq_ignore_ascii_case(&found.wwn))
        && (expected.partition_table_id.is_empty()
            || expected
                .partition_table_id
                .eq_ignore_ascii_case(&found.partition_table_id))
}
pub fn select_bound_disk<R: CommandRunner>(
    runner: &mut R,
    expected: &DiskIdentity,
) -> Result<DiskIdentity, LiveInstallError> {
    let inventory = capture(
        runner,
        "lsblk",
        ["-dnpo", "NAME,TYPE"],
        "live-enumerate-block-disks",
    )?;
    let mut matches = Vec::new();
    for line in inventory.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 2 {
            return Err(LiveInstallError::Invalid("invalid whole-disk inventory"));
        }
        if fields[1] != "disk" {
            continue;
        }
        let found = read_disk_identity(runner, fields[0])
            .map_err(|_| LiveInstallError::Invalid("cannot read whole-disk identity"))?;
        if same_physical_disk(expected, &found) {
            matches.push(found);
        }
    }
    if matches.len() != 1 {
        return Err(LiveInstallError::Invalid(
            "the approved stable disk identity is missing or ambiguous",
        ));
    }
    Ok(matches.remove(0))
}
pub fn verify_unmounted_disk<R: CommandRunner>(
    runner: &mut R,
    device: &str,
) -> Result<(), LiveInstallError> {
    if !device.strip_prefix("/dev/").is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    }) {
        return Err(LiveInstallError::Invalid("invalid whole-disk device"));
    }
    let inventory = capture(
        runner,
        "lsblk",
        ["-nrpo", "NAME,TYPE,MOUNTPOINTS", device],
        "live-check-target-descendants",
    )?;
    let descendants = parse_unmounted_descendants(&inventory, device)?;
    let swaps = fs::read_to_string("/proc/swaps")?;
    reject_target_swaps(&descendants, &swaps)?;
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    if mountinfo.lines().any(|line| {
        line.split_once(" - ")
            .and_then(|(_, rest)| rest.split_whitespace().nth(1))
            .is_some_and(|source| descendants.iter().any(|path| path == source))
    }) {
        return Err(LiveInstallError::Invalid(
            "a target block-device descendant is still mounted",
        ));
    }
    Ok(())
}
pub fn parse_unmounted_descendants(
    output: &str,
    device: &str,
) -> Result<Vec<String>, LiveInstallError> {
    let mut paths = Vec::new();
    let mut disk = false;
    for line in output.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 2
            || !fields[0].starts_with("/dev/")
            || !matches!(fields[1], "disk" | "part")
        {
            return Err(LiveInstallError::Invalid(
                "target contains mounts, LVM/RAID/encryption or an invalid descendant inventory",
            ));
        }
        if fields[0] == device {
            if fields[1] != "disk" || disk {
                return Err(LiveInstallError::Invalid(
                    "selected device is not a unique whole disk",
                ));
            }
            disk = true;
        }
        paths.push(fields[0].into());
    }
    if !disk {
        return Err(LiveInstallError::Invalid(
            "selected device is missing from its descendant inventory",
        ));
    }
    Ok(paths)
}
pub fn reject_target_swaps(descendants: &[String], swaps: &str) -> Result<(), LiveInstallError> {
    if !swaps
        .lines()
        .next()
        .is_some_and(|line| line.starts_with("Filename"))
    {
        return Err(LiveInstallError::Invalid(
            "cannot establish active swap inventory",
        ));
    }
    for line in swaps.lines().skip(1) {
        let source = line
            .split_whitespace()
            .next()
            .ok_or(LiveInstallError::Invalid("invalid active swap inventory"))?;
        if descendants.iter().any(|path| path == source) {
            return Err(LiveInstallError::Invalid(
                "target disk has active swap; refusing to disable or overwrite it",
            ));
        }
    }
    Ok(())
}

fn mount_pseudo<R: CommandRunner>(runner: &mut R, root: &Path) -> Result<(), LiveInstallError> {
    for name in ["dev", "proc", "sys", "run"] {
        fs::create_dir_all(root.join(name))?;
    }
    checked(
        runner,
        "mount",
        [
            OsStr::new("--rbind"),
            "/dev".as_ref(),
            root.join("dev").as_os_str(),
        ],
        "target-mount-device-tree",
    )?;
    checked(
        runner,
        "mount",
        [OsStr::new("--make-rslave"), root.join("dev").as_os_str()],
        "target-isolate-device-mounts",
    )?;
    checked(
        runner,
        "mount",
        [
            OsStr::new("-t"),
            "proc".as_ref(),
            "proc".as_ref(),
            root.join("proc").as_os_str(),
        ],
        "target-mount-proc",
    )?;
    checked(
        runner,
        "mount",
        [
            OsStr::new("-t"),
            "sysfs".as_ref(),
            "sysfs".as_ref(),
            root.join("sys").as_os_str(),
        ],
        "target-mount-sys",
    )?;
    Ok(())
}
fn unmount_target<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    layout: &DiskLayout,
) -> Result<(), LiveInstallError> {
    for name in ["sys", "proc", "dev"] {
        let path = root.join(name);
        let path_text = path.to_string_lossy();
        let mounted = capture(
            runner,
            "findmnt",
            ["-nro", "TARGET", "--mountpoint", &path_text],
            "target-check-owned-mount",
        );
        if mounted.is_ok_and(|text| !text.trim().is_empty()) {
            checked(
                runner,
                "umount",
                [OsStr::new("--recursive"), path.as_os_str()],
                "target-unmount-owned-pseudo",
            )?;
        }
    }
    if layout.efi_partition().is_some() {
        let path = root.join(
            if matches!(layout.target(), Target::Alpine | Target::Debian) {
                "boot/efi"
            } else {
                "efi"
            },
        );
        let mountpoint = path.to_string_lossy();
        if capture(
            runner,
            "findmnt",
            ["-nro", "TARGET", "--mountpoint", &mountpoint],
            "target-check-owned-efi-mount",
        )
        .is_ok_and(|text| !text.trim().is_empty())
        {
            checked(runner, "umount", [path.as_os_str()], "target-unmount-efi")?;
        }
    }
    checked(runner, "umount", [root.as_os_str()], "target-unmount-root")?;
    Ok(())
}
fn filesystem_uuid<R: CommandRunner>(
    runner: &mut R,
    device: &str,
) -> Result<String, LiveInstallError> {
    let text = capture(
        runner,
        "blkid",
        ["-s", "UUID", "-o", "value", device],
        "read-new-filesystem-uuid",
    )?;
    let text = text.trim();
    if text.is_empty()
        || text.len() > 64
        || !text.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
    {
        return Err(LiveInstallError::Invalid("new filesystem UUID is invalid"));
    }
    Ok(text.into())
}
fn checked<R, I, S>(
    runner: &mut R,
    program: &str,
    args: I,
    phase: &str,
) -> Result<(), LiveInstallError>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_checked(runner, &command(program, args, phase))?;
    Ok(())
}
fn command<I, S>(program: &str, args: I, phase: &str) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    crate::command::live_command(program, args, phase).env("LC_ALL", "C")
}
fn capture<R, I, S>(
    runner: &mut R,
    program: &str,
    args: I,
    phase: &str,
) -> Result<String, LiveInstallError>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let spec = command(program, args, phase).stdout_mode(OutputMode::Capture);
    let bytes = crate::command::capture_checked(runner, &spec)?;
    String::from_utf8(bytes)
        .map_err(|_| LiveInstallError::Invalid("native inventory output is not UTF-8"))
}
fn program_exists(name: &str) -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|directory| directory.join(name).is_file())
}
fn announce(message: &str) {
    println!("{message}");
    for path in ["/dev/ttyS0", "/dev/console"] {
        if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_char_device())
            && let Ok(mut file) = OpenOptions::new().write(true).open(path)
        {
            let _ = writeln!(file, "{message}");
        }
    }
}
struct InstallLock {
    path: PathBuf,
}
impl InstallLock {
    fn create(work: &Path) -> Result<Self, LiveInstallError> {
        let path = work.join("install.lock");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        writeln!(file, "{}", std::process::id())?;
        Ok(Self { path })
    }
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
unsafe extern "C" {
    fn geteuid() -> u32;
}
