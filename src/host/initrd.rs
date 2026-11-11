use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode, PipelineOutput};
use crate::model::InstallPlan;
use crate::plan_file::{PlanFileError, PrivatePlanFile};
use crate::worker::initrd_network::InitrdNetworkConfig;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const START_MARKER: &str = "# BEGIN PERSONAL-LINUX initrd network worker";
const END_MARKER: &str = "# END PERSONAL-LINUX initrd network worker";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InitrdError {
    ConfigureIpAnchorNotUnique,
    AlreadyPatched,
    WorkerVersionMismatch,
    InvalidWorker,
    UnsafeStagingPath,
    InitScriptNotRegular,
    ArchiveNotRegular,
    EmptyArchive,
    CommandStart(String),
    CommandFailed {
        program: String,
        status: Option<i32>,
    },
    Filesystem(String),
    Plan(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitrdPayloadPaths {
    pub worker: PathBuf,
    pub plan: PathBuf,
}

impl fmt::Display for InitrdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigureIpAnchorNotUnique => {
                f.write_str("Alpine initrd configure_ip() anchor is missing or ambiguous")
            }
            Self::AlreadyPatched => {
                f.write_str("Alpine initrd already contains the Rust network hook")
            }
            Self::WorkerVersionMismatch => {
                f.write_str("Embedded worker does not match the initrd runtime version")
            }
            Self::InvalidWorker => f.write_str("Embedded worker is not an x86_64 ELF executable"),
            Self::UnsafeStagingPath => {
                f.write_str("Initrd staging root/configs must be real directories, not symlinks")
            }
            Self::InitScriptNotRegular => f.write_str("Alpine initrd /init must be a regular file"),
            Self::ArchiveNotRegular => f.write_str("Alpine initrd archive must be a regular file"),
            Self::EmptyArchive => f.write_str("Alpine initrd archive output is empty"),
            Self::CommandStart(error) => write!(f, "Cannot start initrd archive command: {error}"),
            Self::CommandFailed { program, status } => {
                write!(
                    f,
                    "Initrd archive command {program} failed with status {status:?}"
                )
            }
            Self::Filesystem(error) => write!(f, "Cannot stage Alpine initrd payload: {error}"),
            Self::Plan(error) => write!(f, "Cannot stage private install plan: {error}"),
        }
    }
}

impl std::error::Error for InitrdError {}

impl From<PlanFileError> for InitrdError {
    fn from(error: PlanFileError) -> Self {
        Self::Plan(error.to_string())
    }
}

impl From<io::Error> for InitrdError {
    fn from(error: io::Error) -> Self {
        Self::Filesystem(error.to_string())
    }
}

pub fn unpack_initrd<R: CommandRunner>(
    runner: &mut R,
    archive: &Path,
    staging: &Path,
) -> Result<(), InitrdError> {
    let archive_metadata = fs::symlink_metadata(archive)?;
    if !archive_metadata.file_type().is_file() || archive_metadata.len() == 0 {
        return Err(InitrdError::ArchiveNotRegular);
    }
    if fs::symlink_metadata(staging).is_ok() {
        return Err(InitrdError::UnsafeStagingPath);
    }
    fs::create_dir(staging)?;
    if let Err(error) = fs::set_permissions(staging, fs::Permissions::from_mode(0o700)) {
        let _ = fs::remove_dir_all(staging);
        return Err(InitrdError::Filesystem(error.to_string()));
    }
    let commands = [
        CommandSpec::new("zcat", [archive.as_os_str()])
            .phase("alpine-initrd-unpack")
            .cwd(staging)
            .stdin(CommandInput::Null)
            .stdout_mode(OutputMode::Stream)
            .stderr_mode(OutputMode::Stream),
        CommandSpec::new("cpio", ["-idm"])
            .phase("alpine-initrd-unpack")
            .cwd(staging)
            .stdin(CommandInput::Null)
            .stdout_mode(OutputMode::Stream)
            .stderr_mode(OutputMode::Stream),
    ];
    let output = match runner.run_pipeline(&commands) {
        Ok(output) => output,
        Err(error) => {
            let _ = fs::remove_dir_all(staging);
            return Err(InitrdError::CommandStart(error.to_string()));
        }
    };
    if let Err(error) = check_pipeline(&output, &["zcat", "cpio"]) {
        let _ = fs::remove_dir_all(staging);
        return Err(error);
    }
    Ok(())
}

pub fn repack_initrd<R: CommandRunner>(
    runner: &mut R,
    staging: &Path,
    archive: &Path,
) -> Result<(), InitrdError> {
    ensure_real_directory(staging, false)?;
    let commands = [
        CommandSpec::new("find", ["."])
            .phase("alpine-initrd-repack")
            .cwd(staging)
            .stdin(CommandInput::Null)
            .stdout_mode(OutputMode::Stream)
            .stderr_mode(OutputMode::Stream),
        CommandSpec::new("cpio", ["--quiet", "-o", "-H", "newc", "-R", "0:0"])
            .phase("alpine-initrd-repack")
            .cwd(staging)
            .stdin(CommandInput::Null)
            .stdout_mode(OutputMode::Stream)
            .stderr_mode(OutputMode::Stream),
        CommandSpec::new("gzip", ["-1"])
            .phase("alpine-initrd-repack")
            .cwd(staging)
            .stdin(CommandInput::Null)
            .stdout_mode(OutputMode::Capture)
            .stderr_mode(OutputMode::Stream),
    ];
    let output = runner
        .run_pipeline(&commands)
        .map_err(|error| InitrdError::CommandStart(error.to_string()))?;
    check_pipeline(&output, &["find", "cpio", "gzip"])?;
    if output.stdout.is_empty() {
        return Err(InitrdError::EmptyArchive);
    }
    if fs::symlink_metadata(archive).is_ok_and(|metadata| !metadata.file_type().is_file()) {
        return Err(InitrdError::ArchiveNotRegular);
    }
    atomic_replace(archive, &output.stdout, 0o644)
}

fn check_pipeline(output: &PipelineOutput, programs: &[&str]) -> Result<(), InitrdError> {
    let Some(program) = programs.last() else {
        return Err(InitrdError::CommandStart("empty pipeline".to_owned()));
    };
    let status = output.statuses.last().copied().flatten();
    if status != Some(0) {
        return Err(InitrdError::CommandFailed {
            program: (*program).to_owned(),
            status,
        });
    }
    Ok(())
}

pub fn stage_alpine_initrd(
    initrd_root: &Path,
    plan: &InstallPlan,
    worker_elf: &[u8],
    network_configs: &[InitrdNetworkConfig],
) -> Result<InitrdPayloadPaths, InitrdError> {
    ensure_real_directory(initrd_root, false)?;
    let init_path = initrd_root.join("init");
    let metadata = fs::symlink_metadata(&init_path)?;
    if !metadata.file_type().is_file() {
        return Err(InitrdError::InitScriptNotRegular);
    }
    let mode = metadata.permissions().mode() & 0o777;
    let source = fs::read_to_string(&init_path)?;
    let patched = inject_network_bootstrap(&source, network_configs)?;
    let payload = install_worker_and_plan(initrd_root, plan, worker_elf)?;
    atomic_replace(&init_path, patched.as_bytes(), mode)?;
    Ok(payload)
}

pub fn install_worker_and_plan(
    initrd_root: &Path,
    plan: &InstallPlan,
    worker_elf: &[u8],
) -> Result<InitrdPayloadPaths, InitrdError> {
    if !is_x86_64_elf(worker_elf) {
        return Err(InitrdError::InvalidWorker);
    }
    if !worker_elf
        .windows(crate::RUNTIME_VERSION.len())
        .any(|window| window == crate::RUNTIME_VERSION.as_bytes())
    {
        return Err(InitrdError::WorkerVersionMismatch);
    }

    ensure_real_directory(initrd_root, false)?;
    let configs_dir = initrd_root.join("configs");
    ensure_real_directory(&configs_dir, true)?;
    let worker_path = initrd_root.join("rustaller-worker");
    let plan_path = configs_dir.join("install.plan");
    write_worker(&worker_path, worker_elf)?;
    let mut plan_file = match PrivatePlanFile::create(&plan_path, plan) {
        Ok(file) => file,
        Err(error) => {
            let _ = fs::remove_file(&worker_path);
            return Err(InitrdError::Plan(error.to_string()));
        }
    };
    plan_file.persist();
    Ok(InitrdPayloadPaths {
        worker: worker_path,
        plan: plan_path,
    })
}

fn atomic_replace(path: &Path, bytes: &[u8], mode: u32) -> Result<(), InitrdError> {
    crate::fsutil::atomic_replace(path, bytes, mode).map_err(Into::into)
}

fn ensure_real_directory(path: &Path, create: bool) -> Result<(), InitrdError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(InitrdError::UnsafeStagingPath),
        Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            Ok(())
        }
        Err(error) => Err(InitrdError::Filesystem(error.to_string())),
    }
}

fn write_worker(path: &Path, worker_elf: &[u8]) -> Result<(), InitrdError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(path)
        .map_err(|error| InitrdError::Filesystem(error.to_string()))?;
    let result = (|| {
        file.write_all(worker_elf)?;
        file.set_permissions(fs::Permissions::from_mode(0o700))?;
        file.sync_all()
    })();
    if let Err(error) = result {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(InitrdError::Filesystem(error.to_string()));
    }
    Ok(())
}

fn is_x86_64_elf(bytes: &[u8]) -> bool {
    bytes.len() >= 20
        && &bytes[..4] == b"\x7fELF"
        && bytes[4] == 2
        && bytes[5] == 1
        && bytes[18] == 0x3e
        && bytes[19] == 0
}

pub fn inject_network_bootstrap(
    init_script: &str,
    configs: &[InitrdNetworkConfig],
) -> Result<String, InitrdError> {
    if init_script.contains(START_MARKER) || init_script.contains(END_MARKER) {
        return Err(InitrdError::AlreadyPatched);
    }
    let anchors: Vec<&str> = init_script
        .lines()
        .filter(|line| line.contains("configure_ip()"))
        .collect();
    if anchors.len() != 1 {
        return Err(InitrdError::ConfigureIpAnchorNotUnique);
    }

    let hook = render_network_hook(configs);
    let mut output = String::with_capacity(init_script.len() + hook.len());
    let mut inserted = false;
    for line in init_script.split_inclusive('\n') {
        output.push_str(line);
        if !inserted && line.trim_end_matches('\n').contains("configure_ip()") {
            if !line.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&hook);
            inserted = true;
        }
    }
    if !inserted {
        return Err(InitrdError::ConfigureIpAnchorNotUnique);
    }
    Ok(output)
}

fn render_network_hook(configs: &[InitrdNetworkConfig]) -> String {
    let mut hook = format!(
        "        {START_MARKER}\n        depmod\n        [ -d /sys/module/ipv6 ] || modprobe ipv6\n"
    );
    for config in configs {
        hook.push_str("        /rustaller-worker initrd-network");
        for argument in [
            config.mac_addr(),
            config.ipv4_addr(),
            config.ipv4_gateway(),
            config.ipv6_addr(),
            config.ipv6_gateway(),
            config.ipv6_extra_addrs(),
        ] {
            hook.push(' ');
            hook.push_str(&shell_quote(argument));
        }
        hook.push('\n');
    }
    hook.push_str(&format!(
        "        MAC_ADDRESS=1\n        return\n        {END_MARKER}\n"
    ));
    hook
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
