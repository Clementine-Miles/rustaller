use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub enum DiskError {
    Command(io::Error),
    CommandFailed {
        program: &'static str,
        status: Option<i32>,
    },
    RootDeviceNotFound,
    InvalidLsblkOutput,
    NoRootDisks,
    AmbiguousRootDisks(Vec<String>),
    InvalidDiskSelection(String),
    InvalidDiskDevice(PathBuf),
    InvalidPartitionTableId,
}

impl fmt::Display for DiskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(error) => write!(f, "Disk discovery command failed to start: {error}"),
            Self::CommandFailed { program, status } => {
                write!(
                    f,
                    "Disk discovery command {program} failed with status {status:?}"
                )
            }
            Self::RootDeviceNotFound => f.write_str("Cannot determine the root filesystem device"),
            Self::InvalidLsblkOutput => f.write_str("Cannot parse lsblk disk inventory"),
            Self::NoRootDisks => f.write_str("No physical disk found for the root filesystem"),
            Self::AmbiguousRootDisks(disks) => {
                write!(
                    f,
                    "Root filesystem spans multiple disks: {}",
                    disks.join(", ")
                )
            }
            Self::InvalidDiskSelection(disk) => write!(f, "Invalid root disk selection: {disk}"),
            Self::InvalidDiskDevice(path) => {
                write!(
                    f,
                    "Target disk is not a block device under /dev: {}",
                    path.display()
                )
            }
            Self::InvalidPartitionTableId => {
                f.write_str("Disk partition-table identifier is invalid")
            }
        }
    }
}

impl std::error::Error for DiskError {}

pub fn validate_target_disk_path(path: &Path) -> Result<(), DiskError> {
    let under_dev = path.is_absolute()
        && path.components().next() == Some(Component::RootDir)
        && path.components().nth(1) == Some(Component::Normal(OsStr::new("dev")))
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir));
    if !under_dev
        || !fs::metadata(path)
            .map(|metadata| metadata.file_type().is_block_device())
            .unwrap_or(false)
    {
        return Err(DiskError::InvalidDiskDevice(path.to_owned()));
    }
    Ok(())
}

pub fn parse_root_device(mount_output: &str) -> Option<&str> {
    mount_output.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let device = fields.next()?;
        (fields.next()? == "on" && fields.next()? == "/").then_some(device)
    })
}

pub fn parse_root_disk_names(lsblk_output: &str) -> Result<Vec<String>, DiskError> {
    let mut names = Vec::new();
    for line in lsblk_output.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            return Err(DiskError::InvalidLsblkOutput);
        }
        if fields[5] == "disk" {
            let name = fields[0];
            if !valid_disk_name(name) {
                return Err(DiskError::InvalidLsblkOutput);
            }
            names.push(name.to_owned());
        }
    }
    names.sort_unstable();
    names.dedup();
    Ok(names)
}

pub fn select_root_disk(
    candidates: &[String],
    selected: Option<&str>,
) -> Result<String, DiskError> {
    if candidates.is_empty() {
        return Err(DiskError::NoRootDisks);
    }
    if let Some(selected) = selected {
        if candidates.iter().any(|candidate| candidate == selected) {
            return Ok(selected.to_owned());
        }
        return Err(DiskError::InvalidDiskSelection(selected.to_owned()));
    }
    match candidates {
        [only] => Ok(only.clone()),
        _ => Err(DiskError::AmbiguousRootDisks(candidates.to_vec())),
    }
}

pub fn parse_partition_table_id(fdisk_output: &str) -> Result<String, DiskError> {
    let identifiers: Vec<&str> = fdisk_output
        .lines()
        .filter(|line| line.contains("Disk identifier"))
        .filter_map(|line| line.split_whitespace().last())
        .collect();
    if identifiers.len() != 1 {
        return Err(DiskError::InvalidPartitionTableId);
    }
    let identifier = identifiers[0]
        .strip_prefix("0x")
        .or_else(|| identifiers[0].strip_prefix("0X"))
        .unwrap_or(identifiers[0]);
    let valid_mbr =
        identifier.len() == 8 && identifier.bytes().all(|byte| byte.is_ascii_hexdigit());
    let valid_gpt = identifier.len() == 36
        && identifier.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        });
    if valid_mbr || valid_gpt {
        Ok(identifier.to_owned())
    } else {
        Err(DiskError::InvalidPartitionTableId)
    }
}

pub fn discover_root_disks<R: CommandRunner>(runner: &mut R) -> Result<Vec<String>, DiskError> {
    let mount = run_capture(
        runner,
        "mount",
        std::iter::empty::<&str>(),
        "host-root-device",
    )?;
    let mount = String::from_utf8(mount).map_err(|_| DiskError::RootDeviceNotFound)?;
    let root_device = parse_root_device(&mount).ok_or(DiskError::RootDeviceNotFound)?;
    let lsblk = run_capture(
        runner,
        "lsblk",
        ["-rn", "--inverse", root_device],
        "host-root-disks",
    )?;
    let lsblk = String::from_utf8(lsblk).map_err(|_| DiskError::InvalidLsblkOutput)?;
    let disks = parse_root_disk_names(&lsblk)?;
    if disks.is_empty() {
        return Err(DiskError::NoRootDisks);
    }
    Ok(disks)
}

pub fn read_partition_table_id<R: CommandRunner>(
    runner: &mut R,
    disk_path: &str,
) -> Result<String, DiskError> {
    let output = run_capture(runner, "fdisk", ["-l", disk_path], "host-disk-identifier")?;
    let output = String::from_utf8(output).map_err(|_| DiskError::InvalidPartitionTableId)?;
    parse_partition_table_id(&output)
}

fn run_capture<R, I, S>(
    runner: &mut R,
    program: &'static str,
    args: I,
    phase: &'static str,
) -> Result<Vec<u8>, DiskError>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let command = CommandSpec::new(program, args)
        .phase(phase)
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Stream);
    let output = runner.run(&command).map_err(DiskError::Command)?;
    if output.status == Some(0) {
        Ok(output.stdout)
    } else {
        Err(DiskError::CommandFailed {
            program,
            status: output.status,
        })
    }
}

fn valid_disk_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}
