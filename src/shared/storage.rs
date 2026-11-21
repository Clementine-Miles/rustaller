//! Validated disk layouts, partitioning and mount operations.
use crate::command::{CommandOutput, CommandRunner, CommandSpec, OutputMode, shell_attempt};
use crate::config::RootFilesystem;
use crate::model::{BootMode, Target};
use crate::worker::live::ValidatedLivePlan;
use std::{
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
};

#[derive(Debug)]
pub enum StorageError {
    InvalidDisk,
    InvalidUuid,
    UnsupportedGeometry,
    InsufficientRootSpace,
    RescuePlan,
    ConflictingDisk,
    InvalidMountPath,
    Filesystem(io::Error),
    CommandStart {
        program: String,
        error: io::Error,
    },
    CommandFailed {
        program: String,
        status: Option<i32>,
    },
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUuid => f.write_str("Invalid filesystem UUID"),
            Self::InvalidDisk => f.write_str("Invalid Live target disk or size"),
            Self::InsufficientRootSpace => f.write_str("Root must have at least 512 MiB after boot partitions; this is a formatting floor, not a distro installation size guarantee"),
            Self::UnsupportedGeometry => f.write_str("Target disk must be writable with 512-byte sectors, or 4096-byte sectors under UEFI; unknown geometry and BIOS on 4Kn are unsupported"),
            Self::RescuePlan => f.write_str("Alpine Live rescue must not partition a disk"),
            Self::ConflictingDisk => {
                f.write_str("Live disk differs from the explicit install plan")
            }
            Self::InvalidMountPath => f.write_str("Invalid target mount path"),
            Self::Filesystem(error) => write!(f, "Live storage filesystem error: {error}"),
            Self::CommandStart { program, error } => {
                write!(f, "Cannot start Live storage command {program}: {error}")
            }
            Self::CommandFailed { program, status } => write!(
                f,
                "Live storage command {program} failed with status {status:?}"
            ),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<io::Error> for StorageError {
    fn from(error: io::Error) -> Self {
        Self::Filesystem(error)
    }
}

pub(crate) use crate::command::live_command as command;

pub(crate) fn run<R: CommandRunner>(
    runner: &mut R,
    spec: &CommandSpec,
) -> Result<CommandOutput, StorageError> {
    runner
        .run(spec)
        .map_err(|error| StorageError::CommandStart {
            program: spec.program.to_string_lossy().into_owned(),
            error,
        })
}

pub(crate) fn checked<R: CommandRunner>(
    runner: &mut R,
    spec: &CommandSpec,
) -> Result<(), StorageError> {
    let output = run(runner, spec)?;
    if output.status == Some(0) {
        Ok(())
    } else {
        Err(StorageError::CommandFailed {
            program: spec.program.to_string_lossy().into_owned(),
            status: output.status,
        })
    }
}

// Partition-table refresh tolerates SIGPIPE (141) and sleeps only between attempts.
pub(crate) fn retry<R: CommandRunner>(
    runner: &mut R,
    spec: &CommandSpec,
    attempts: usize,
) -> Result<(), StorageError> {
    for attempt in 1..=attempts {
        let output = shell_attempt(runner, spec);
        if matches!(output.status, Some(0 | 141)) {
            return Ok(());
        }
        if attempt == attempts {
            return Err(StorageError::CommandFailed {
                program: spec.program.to_string_lossy().into_owned(),
                status: output.status,
            });
        }
        checked(runner, &command("sleep", ["5"], &spec.phase))?;
    }
    unreachable!("retry is called with at least one attempt")
}

pub(crate) fn apk<R: CommandRunner>(runner: &mut R, args: &[&str]) -> Result<(), StorageError> {
    retry(runner, &command("apk", args, "live-partition-packages"), 5)
}

const TWO_TIB: u64 = 2 * 1024_u64.pow(4);

/// Recheck immediately before wiping: cloud volumes can be attached read-only,
/// and 4Kn requires a FAT32 ESP large enough for at least 65525 clusters.
pub fn validate_disk_geometry<R: CommandRunner>(
    runner: &mut R,
    device: &str,
    boot: BootMode,
) -> Result<(), StorageError> {
    let output = run(
        runner,
        &command(
            "lsblk",
            ["-dnro", "RO,LOG-SEC", "--", device],
            "check-disk-geometry",
        )
        .stdout_mode(OutputMode::Capture),
    )?;
    let text =
        std::str::from_utf8(&output.stdout).map_err(|_| StorageError::UnsupportedGeometry)?;
    let mut values = text.split_whitespace();
    if output.status != Some(0)
        || values.next() != Some("0")
        || !matches!(
            (values.next(), boot),
            (Some("512"), _) | (Some("4096"), BootMode::Efi)
        )
        || values.next().is_some()
    {
        return Err(StorageError::UnsupportedGeometry);
    }
    Ok(())
}

/// Partition roles are explicit for both two- and three-partition layouts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartitionRole {
    BiosBoot,
    Boot,
    Root,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Partition {
    pub number: u8,
    pub role: PartitionRole,
    pub start_mib: u64,
    pub end_mib: Option<u64>,
}
/// Pure geometry calculation shared by host preview and the gated Live layout.
pub fn planned_partitions(
    boot_mode: BootMode,
    bytes: u64,
    boot_size_mib: u32,
) -> Result<Vec<Partition>, StorageError> {
    planned_partition_sizes(boot_mode, bytes, boot_size_mib, None)
}

pub fn planned_partition_sizes(
    boot_mode: BootMode,
    bytes: u64,
    boot_size_mib: u32,
    root_size_mib: Option<u32>,
) -> Result<Vec<Partition>, StorageError> {
    if !(512..=4096).contains(&boot_size_mib) {
        return Err(StorageError::UnsupportedGeometry);
    }
    let bios_gpt = boot_mode == BootMode::Bios && bytes >= TWO_TIB;
    let boot_number = if bios_gpt { 2 } else { 1 };
    let boot_start = if bios_gpt { 2 } else { 1 };
    let root_start_mib = boot_start + u64::from(boot_size_mib);
    let mut partitions = Vec::new();
    if bios_gpt {
        partitions.push(Partition {
            number: 1,
            role: PartitionRole::BiosBoot,
            start_mib: 1,
            end_mib: Some(2),
        });
    }
    partitions.push(Partition {
        number: boot_number,
        role: PartitionRole::Boot,
        start_mib: boot_start,
        end_mib: Some(root_start_mib),
    });
    partitions.push(Partition {
        number: boot_number + 1,
        role: PartitionRole::Root,
        start_mib: root_start_mib,
        end_mib: root_size_mib.map(|size| root_start_mib + u64::from(size)),
    });
    // Fail before wipefs, including space reserved for ESP/alignment.
    // A fixed MiB endpoint on GPT must also leave room for the backup table
    // (128 entries + header, up to 20 KiB on the supported 4Kn geometry).
    let reserve_bytes = if (boot_mode == BootMode::Efi || bios_gpt) && root_size_mib.is_some() {
        20 * 1024
    } else {
        0
    };
    let root_mib = root_size_mib.unwrap_or(512);
    if root_mib < 512
        || bytes.saturating_sub(reserve_bytes) / (1024 * 1024)
            < root_start_mib + u64::from(root_mib)
    {
        return Err(StorageError::InsufficientRootSpace);
    }

    Ok(partitions)
}
/// Constructible only after the Live plan gate; still not authorization to erase.
pub struct DiskLayout {
    disk: String,
    target: Target,
    boot_mode: BootMode,
    partitions: Vec<Partition>,
    root_filesystem: RootFilesystem,
    btrfs_compression: u8,
}

impl DiskLayout {
    pub fn new(
        plan: &ValidatedLivePlan,
        disk: &str,
        boot_mode: BootMode,
        bytes: u64,
    ) -> Result<Self, StorageError> {
        if !valid_disk_name(disk) || bytes == 0 {
            return Err(StorageError::InvalidDisk);
        }
        if plan.plan().hold == Some(1) {
            return Err(StorageError::RescuePlan);
        }
        if plan
            .plan()
            .target_disk
            .as_deref()
            .is_some_and(|selected| selected != format!("/dev/{disk}"))
        {
            return Err(StorageError::ConflictingDisk);
        }
        if plan
            .plan()
            .force_boot_mode
            .is_some_and(|expected| expected != boot_mode)
        {
            return Err(StorageError::UnsupportedGeometry);
        }
        let partitions = planned_partition_sizes(
            boot_mode,
            bytes,
            plan.plan().system.boot_size_mib.unwrap_or(512),
            plan.plan().system.root_size_mib,
        )?;
        Ok(Self {
            disk: disk.to_owned(),
            target: plan.plan().target,
            boot_mode,
            partitions,
            root_filesystem: plan.plan().system.root_filesystem,
            btrfs_compression: plan.plan().system.btrfs_compression.unwrap_or(3),
        })
    }

    pub fn partitions(&self) -> &[Partition] {
        &self.partitions
    }
    pub fn device(&self) -> String {
        format!("/dev/{}", self.disk)
    }
    pub fn boot_mode(&self) -> BootMode {
        self.boot_mode
    }
    pub fn target(&self) -> Target {
        self.target
    }
    pub fn root_filesystem(&self) -> RootFilesystem {
        self.root_filesystem
    }
    pub fn table(&self) -> &'static str {
        if self.boot_mode == BootMode::Efi || self.role_number(PartitionRole::BiosBoot).is_some() {
            "gpt"
        } else {
            "msdos"
        }
    }
    pub fn role_number(&self, role: PartitionRole) -> Option<u8> {
        self.partitions
            .iter()
            .find(|p| p.role == role)
            .map(|p| p.number)
    }
    pub fn boot_partition(&self) -> String {
        self.partition(
            self.role_number(PartitionRole::Boot)
                .expect("boot partition"),
        )
    }
    pub fn root_partition(&self) -> String {
        self.partition(
            self.role_number(PartitionRole::Root)
                .expect("root partition"),
        )
    }

    fn partition(&self, number: u8) -> String {
        let separator = if self.disk.as_bytes().last().is_some_and(u8::is_ascii_digit) {
            "p"
        } else {
            ""
        };
        format!("/dev/{}{separator}{number}", self.disk)
    }
}

/// Render the sole target fstab policy, independent of filesystem mutation.
pub fn render_fstab(
    layout: &DiskLayout,
    root_uuid: &str,
    boot_uuid: &str,
) -> Result<String, StorageError> {
    if !valid_uuid(root_uuid) || !valid_uuid(boot_uuid) {
        return Err(StorageError::InvalidUuid);
    }
    let filesystem = layout.root_filesystem();
    let options = if filesystem == RootFilesystem::Btrfs {
        btrfs_options(layout.btrfs_compression)
    } else {
        "defaults".into()
    };
    let pass = if filesystem == RootFilesystem::Ext4 {
        1
    } else {
        0
    };
    Ok(format!(
        "UUID={root_uuid} / {} {options} 0 {pass}\nUUID={boot_uuid} /boot vfat fmask=0177,dmask=0077,quiet 0 2\n",
        filesystem.as_str()
    ))
}
fn valid_disk_name(disk: &str) -> bool {
    disk.as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && disk
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

#[derive(Default)]
pub struct PartitionEnvironment {
    pub has_partprobe: bool,
    pub has_partx: bool,
    /// None means no device links were present. Callers must explicitly supply
    /// /dev/disk in Live; tests supply their own isolated directory.
    pub device_links: Option<PathBuf>,
}

pub fn create_partitions<R: CommandRunner>(
    runner: &mut R,
    layout: &DiskLayout,
    environment: &PartitionEnvironment,
) -> Result<(), StorageError> {
    let filesystem = layout.root_filesystem.as_str();
    let tools = match layout.root_filesystem {
        RootFilesystem::Xfs => "xfsprogs",
        RootFilesystem::Ext4 => "e2fsprogs",
        RootFilesystem::Btrfs => "btrfs-progs",
    };
    apk(runner, &["add", "parted", tools])?;
    apk(runner, &["add", "dosfstools"])?;
    apk(runner, &["add", "wipefs"])?;
    let device = layout.device();
    checked(
        runner,
        &command("wipefs", ["-a", "-f", &device], "live-wipe-disk"),
    )?;
    apk(runner, &["del", "wipefs"])?;

    let mut args = vec![
        device.clone(),
        "-s".into(),
        "--".into(),
        "mklabel".into(),
        layout.table().into(),
    ];
    for partition in &layout.partitions {
        args.push("mkpart".into());
        args.push(
            if layout.table() == "msdos" {
                "primary"
            } else {
                "rustaller"
            }
            .into(),
        );
        args.push(
            if partition.role == PartitionRole::Boot {
                "fat32"
            } else if partition.role == PartitionRole::BiosBoot {
                "ext2"
            } else {
                filesystem
            }
            .into(),
        );
        args.push(format!("{}MiB", partition.start_mib));
        args.push(
            partition
                .end_mib
                .map(|v| format!("{v}MiB"))
                .unwrap_or("100%".into()),
        );
    }
    let flag = if layout.boot_mode == BootMode::Efi {
        "esp"
    } else if layout.role_number(PartitionRole::BiosBoot).is_some() {
        "bios_grub"
    } else {
        "boot"
    };
    args.extend(["set".into(), "1".into(), flag.into(), "on".into()]);
    checked(runner, &command("parted", args, "live-partition-disk"))?;
    refresh_partition_table(runner, layout, environment)?;

    {
        let efi = layout.boot_partition();
        checked(
            runner,
            &command(
                "mkfs.fat",
                ["-F", "32", "-s", "1", &efi],
                "live-format-boot",
            ),
        )?;
    }
    let root = layout.root_partition();
    // Preserve conservative XFS features and fully allocated optional swapfiles.
    // No reflink sharing is needed on this root, including its optional swapfile.
    let args = vec![
        "-f",
        // Avoid a full-volume discard during installation on thin VPS disks.
        "-K",
        "-m",
        "reflink=0,bigtime=0,inobtcount=0",
        "-i",
        "nrext64=0",
        &root,
    ];
    match layout.root_filesystem {
        RootFilesystem::Xfs => checked(runner, &command("mkfs.xfs", args, "live-format-root"))?,
        RootFilesystem::Btrfs => checked(
            runner,
            &command("mkfs.btrfs", ["-f", "-K", &root], "live-format-root"),
        )?,
        // Fully initialize inode tables/journal before the target's first boot.
        // Avoid whole-volume discard on thin-provisioned VPS disks.
        RootFilesystem::Ext4 => checked(
            runner,
            &command(
                "mkfs.ext4",
                [
                    "-F",
                    "-E",
                    "nodiscard,lazy_itable_init=0,lazy_journal_init=0",
                    &root,
                ],
                "live-format-root",
            ),
        )?,
    }
    refresh_partition_table(runner, layout, environment)?;
    if layout.target == Target::Alpine {
        apk(runner, &["del", "parted"])?;
    }
    Ok(())
}

pub fn refresh_partition_table<R: CommandRunner>(
    runner: &mut R,
    layout: &DiskLayout,
    environment: &PartitionEnvironment,
) -> Result<(), StorageError> {
    let phase = "live-refresh-partitions";
    checked(runner, &command("sleep", ["1"], phase))?;
    checked(runner, &command("sync", [] as [&str; 0], phase))?;
    checked(runner, &command("sleep", ["1"], phase))?;
    let device = layout.device();
    if environment.has_partprobe {
        let spec = command("partprobe", [&device], phase).stderr_mode(OutputMode::Discard);
        // Shell uses `partprobe ... || true`.
        let _ = shell_attempt(runner, &spec);
        checked(runner, &command("sleep", ["1"], phase))?;
    }
    if environment.has_partx {
        checked(runner, &command("partx", ["-u", &device], phase))?;
        checked(runner, &command("sleep", ["1"], phase))?;
    }
    retry(
        runner,
        &command("rc-service", ["-q", "mdev", "stop"], phase),
        10,
    )?;
    checked(runner, &command("sleep", ["1"], phase))?;
    // Rust replaces rm's glob expansion and recursive filesystem operation.
    if let Some(directory) = &environment.device_links {
        for attempt in 1..=5 {
            match remove_device_links(directory, layout) {
                Ok(()) => break,
                Err(error) if attempt == 5 => return Err(error),
                Err(_) => checked(runner, &command("sleep", ["5"], phase))?,
            }
        }
    }
    checked(
        runner,
        &command("mdev", ["-sf"], phase).stderr_mode(OutputMode::Discard),
    )?;
    checked(runner, &command("sleep", ["1"], phase))?;
    ensure_mdev_started(runner)?;
    checked(runner, &command("sleep", ["1"], phase))
}

fn remove_device_links(directory: &Path, layout: &DiskLayout) -> Result<(), StorageError> {
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err(StorageError::InvalidMountPath),
        Err(error) => return Err(error.into()),
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        // Only invalidate links to this disk. The old global deletion also
        // removed identities of unrelated cloud data volumes and recovery media.
        if entry.file_name().as_encoded_bytes().starts_with(b".") {
            continue;
        }
        if entry.file_type()?.is_dir() {
            remove_device_links(&entry.path(), layout)?;
        } else if entry.file_type()?.is_symlink() {
            let target = fs::read_link(entry.path())?;
            if let Some(name) = target.file_name().and_then(OsStr::to_str) {
                let partition_prefix = if layout
                    .disk
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_digit)
                {
                    format!("{}p", layout.disk)
                } else {
                    layout.disk.clone()
                };
                let belongs = name == layout.disk
                    || name.strip_prefix(&partition_prefix).is_some_and(|suffix| {
                        !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
                    });
                if belongs
                    && (target == Path::new("/dev").join(name)
                        || target == Path::new("../..").join(name))
                {
                    fs::remove_file(entry.path())?;
                }
            }
        }
    }
    Ok(())
}

fn ensure_mdev_started<R: CommandRunner>(runner: &mut R) -> Result<(), StorageError> {
    let spec = command("rc-service", ["-q", "mdev", "start"], "live-start-mdev")
        .stderr_mode(OutputMode::Discard);
    let mut output = shell_attempt(runner, &spec);
    if output.status == Some(0) {
        return Ok(());
    }
    // ensure_service_started first retries immediately, then sleeps after each
    // failed retry (including the last one), matching the Alpine helper.
    for _ in 0..10 {
        output = shell_attempt(runner, &spec);
        if output.status == Some(0) {
            return Ok(());
        }
        checked(runner, &command("sleep", ["5"], "live-start-mdev"))?;
    }
    Err(StorageError::CommandFailed {
        program: "rc-service".into(),
        status: output.status,
    })
}

pub(crate) fn validate_root(root: &Path) -> Result<(), StorageError> {
    if !root.is_absolute()
        || root == Path::new("/")
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        Err(StorageError::InvalidMountPath)
    } else {
        let mut prefix = std::path::PathBuf::new();
        for part in root.components() {
            prefix.push(part);
            match fs::symlink_metadata(&prefix) {
                Ok(metadata) if !metadata.file_type().is_dir() => {
                    return Err(StorageError::InvalidMountPath);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
        }
        Ok(())
    }
}

pub fn mount_target<R: CommandRunner>(
    runner: &mut R,
    layout: &DiskLayout,
    root: &Path,
) -> Result<(), StorageError> {
    validate_root(root)?;
    fs::create_dir_all(root)?;
    checked(
        runner,
        &command(
            "mount",
            [
                std::ffi::OsStr::new("-t"),
                std::ffi::OsStr::new(layout.root_filesystem.as_str()),
                layout.root_partition().as_ref(),
                root.as_os_str(),
            ],
            "live-mount-root",
        ),
    )?;
    if layout.root_filesystem.as_str() == "btrfs" {
        let subvolume = root.join("@");
        let created = checked(
            runner,
            &command(
                "btrfs",
                [
                    "subvolume".as_ref(),
                    "create".as_ref(),
                    subvolume.as_os_str(),
                ],
                "create-root-subvolume",
            ),
        );
        let unmounted = checked(
            runner,
            &command("umount", [root.as_os_str()], "unmount-btrfs-top"),
        );
        created?;
        unmounted?;
        checked(
            runner,
            &command(
                "mount",
                [
                    "-t".as_ref(),
                    "btrfs".as_ref(),
                    "-o".as_ref(),
                    btrfs_options(layout.btrfs_compression).as_ref(),
                    layout.root_partition().as_ref(),
                    root.as_os_str(),
                ],
                "mount-btrfs-root",
            ),
        )?;
    }
    {
        let efi = layout.boot_partition();
        let efi_root = root.join("boot");
        validate_root(&efi_root)?;
        fs::create_dir_all(&efi_root)?;
        checked(
            runner,
            &command(
                "mount",
                [
                    std::ffi::OsStr::new("-t"),
                    "vfat".as_ref(),
                    "-o".as_ref(),
                    // FAT masks are authoritative. quiet lets native package chmod/chown
                    // requests succeed without pretending FAT stores Unix metadata.
                    // Symlink creation is separately disabled in distro kernel policy.
                    "fmask=0177,dmask=0077,quiet".as_ref(),
                    efi.as_ref(),
                    efi_root.as_os_str(),
                ],
                "live-mount-boot",
            ),
        )?;
    }
    Ok(())
}

/// Safe bounded filesystem identifier for blkid, fstab and bootloader arguments.
/// Root-format constraints belong to the caller; FAT uses shorter identifiers.
pub(crate) fn valid_uuid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

pub fn btrfs_options(compression: u8) -> String {
    if compression == 0 {
        "subvol=@".into()
    } else {
        format!("subvol=@,compress=zstd:{compression}")
    }
}
