use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use crate::model::IsoOptions;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

const GRUBENV_HELPER: &str = "if ! [ -s $prefix/grubenv ]; then\n    for dir in /boot/grub /boot/grub2 /grub /grub2; do\n        set grubenv=\"($root)$dir/grubenv\"\n        if [ -s $grubenv ]; then\n            load_env --file $grubenv\n            if [ \"${next_entry}\" ]; then\n                set default=\"${next_entry}\"\n                set next_entry=\n                save_env --file $grubenv next_entry\n            else\n                set default=\"0\"\n            fi\n            return\n        fi\n    done\nfi\n";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedIsoOptions {
    pub config: Option<String>,
    pub kernel: Option<String>,
    pub initrd: Option<String>,
    pub cmdline: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedIso {
    image: PathBuf,
    uuid: String,
    grub_path: String,
    config: Option<String>,
    kernel: Option<String>,
    initrd: Option<String>,
    cmdline: Option<String>,
}

impl PreparedIso {
    pub fn image(&self) -> &Path {
        &self.image
    }

    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    pub fn grub_path(&self) -> &str {
        &self.grub_path
    }
}

#[derive(Debug)]
pub enum IsoError {
    InvalidPath,
    MissingSource,
    ProtectedSource,
    IncompleteCustomBoot,
    ConflictingBootModes,
    InvalidCmdline,
    InvalidUuid,
    EncryptedStorage,
    InvalidFilesystemLayout,
    MissingBootFiles,
    Command(io::Error),
    CommandFailed {
        program: String,
        status: Option<i32>,
    },
    Filesystem(io::Error),
}

impl fmt::Display for IsoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath => f.write_str(
                "ISO and internal paths must be absolute safe paths without dot segments",
            ),
            Self::MissingSource => {
                f.write_str("--iso must name a readable, nonempty local ISO file")
            }
            Self::ProtectedSource => {
                f.write_str("ISO source must not be inside managed temporary or boot files")
            }
            Self::IncompleteCustomBoot => f.write_str(
                "Specify both --iso-kernel and --iso-initrd; --iso-cmdline requires them",
            ),
            Self::ConflictingBootModes => {
                f.write_str("--iso-config conflicts with explicit kernel/initrd")
            }
            Self::InvalidCmdline => {
                f.write_str("ISO cmdline contains unsupported quoting or GRUB metacharacters")
            }
            Self::InvalidUuid => f.write_str("GRUB returned an invalid filesystem UUID"),
            Self::EncryptedStorage => {
                f.write_str("Local ISO boot from encrypted storage is not supported")
            }
            Self::InvalidFilesystemLayout => {
                f.write_str("This interface is for Linux installation media only")
            }
            Self::MissingBootFiles => f.write_str(
                "ISO loopback configuration or requested kernel/initrd files were not found",
            ),
            Self::Command(error) => write!(f, "Local ISO command failed to start: {error}"),
            Self::CommandFailed { program, status } => write!(
                f,
                "Local ISO command {program} failed with status {status:?}"
            ),
            Self::Filesystem(error) => write!(f, "Local ISO filesystem operation failed: {error}"),
        }
    }
}

impl std::error::Error for IsoError {}

pub fn validate_iso_options(options: &IsoOptions) -> Result<ValidatedIsoOptions, IsoError> {
    if !safe_grub_path(&options.path) {
        return Err(IsoError::InvalidPath);
    }
    let custom_boot = options.kernel.is_some() || options.initrd.is_some();
    if custom_boot != (options.kernel.is_some() && options.initrd.is_some()) {
        return Err(IsoError::IncompleteCustomBoot);
    }
    if options.cmdline.is_some() && !custom_boot {
        return Err(IsoError::IncompleteCustomBoot);
    }
    if options.config.is_some() && custom_boot {
        return Err(IsoError::ConflictingBootModes);
    }
    if let Some(config) = options.config.as_deref()
        && !safe_grub_path(config)
    {
        return Err(IsoError::InvalidPath);
    }
    if let Some(kernel) = options.kernel.as_deref()
        && !safe_grub_path(kernel)
    {
        return Err(IsoError::InvalidPath);
    }
    if let Some(initrd) = options.initrd.as_deref()
        && !safe_grub_path(initrd)
    {
        return Err(IsoError::InvalidPath);
    }
    if options
        .cmdline
        .as_deref()
        .is_some_and(|value| !safe_cmdline(value))
    {
        return Err(IsoError::InvalidCmdline);
    }
    Ok(ValidatedIsoOptions {
        config: if custom_boot {
            None
        } else {
            Some(
                options
                    .config
                    .clone()
                    .unwrap_or_else(|| "/boot/grub/loopback.cfg".to_owned()),
            )
        },
        kernel: options.kernel.clone(),
        initrd: options.initrd.clone(),
        cmdline: options.cmdline.clone(),
    })
}

pub fn canonicalize_iso_source(path: &Path, managed_tmp: &Path) -> Result<PathBuf, IsoError> {
    let canonical = fs::canonicalize(path).map_err(|_| IsoError::MissingSource)?;
    let metadata = fs::metadata(&canonical).map_err(|_| IsoError::MissingSource)?;
    if !metadata.is_file() || metadata.len() == 0 || fs::File::open(&canonical).is_err() {
        return Err(IsoError::MissingSource);
    }
    let canonical_text = canonical.to_str().ok_or(IsoError::InvalidPath)?;
    if !safe_grub_path(canonical_text) {
        return Err(IsoError::InvalidPath);
    }
    let managed = fs::canonicalize(managed_tmp).unwrap_or_else(|_| lexical_absolute(managed_tmp));
    if canonical.starts_with(&managed)
        || canonical == Path::new("/reinstall-vmlinuz")
        || canonical == Path::new("/reinstall-initrd")
    {
        return Err(IsoError::ProtectedSource);
    }
    Ok(canonical)
}

pub fn prepare_local_iso<R: CommandRunner>(
    runner: &mut R,
    options: &IsoOptions,
    managed_tmp: &Path,
    check_dir: &Path,
) -> Result<PreparedIso, IsoError> {
    let validated = validate_iso_options(options)?;
    let image = canonicalize_iso_source(Path::new(&options.path), managed_tmp)?;
    let image_arg = image.as_os_str();
    let (prefix, abstraction) = choose_grub_prefix(runner, image_arg)?;
    if String::from_utf8_lossy(&abstraction).contains("cryptodisk") {
        return Err(IsoError::EncryptedStorage);
    }

    let uuid_bytes = run_capture(
        runner,
        &format!("{prefix}-probe"),
        [
            "--target=fs_uuid",
            image.to_str().ok_or(IsoError::InvalidPath)?,
        ],
    )?;
    let uuid = trim_command_output(&uuid_bytes)?;
    if uuid.is_empty()
        || !uuid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(IsoError::InvalidUuid);
    }

    let grub_path_bytes = run_capture(
        runner,
        &format!("{prefix}-mkrelpath"),
        [image.to_str().ok_or(IsoError::InvalidPath)?],
    )?;
    let grub_path = trim_command_output(&grub_path_bytes)?;
    if !safe_grub_path(&grub_path) {
        return Err(IsoError::InvalidPath);
    }

    if !check_dir.starts_with(managed_tmp) || has_symlink(check_dir) || has_symlink(managed_tmp) {
        return Err(IsoError::ProtectedSource);
    }
    fs::create_dir_all(check_dir).map_err(IsoError::Filesystem)?;
    let mount = CommandSpec::new(
        "mount",
        [
            "-o",
            "loop,ro",
            image.to_str().ok_or(IsoError::InvalidPath)?,
            check_dir.to_str().ok_or(IsoError::InvalidPath)?,
        ],
    )
    .phase("local-iso-mount-readonly")
    .stdin(CommandInput::Null)
    .stdout_mode(OutputMode::Stream)
    .stderr_mode(OutputMode::Stream);
    run_checked(runner, &mount, "mount")?;

    let inspection = inspect_iso_contents(check_dir, &validated);
    let umount = CommandSpec::new("umount", [check_dir.to_str().ok_or(IsoError::InvalidPath)?])
        .phase("local-iso-unmount")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream);
    let unmount_result = run_checked(runner, &umount, "umount");
    let _ = fs::remove_dir(check_dir);
    unmount_result?;
    inspection?;

    let cmdline = validated.cmdline.map(|value| {
        value
            .replace("%ISO_PATH%", &grub_path)
            .replace("%ISO_UUID%", &uuid)
    });
    Ok(PreparedIso {
        image,
        uuid,
        grub_path,
        config: validated.config,
        kernel: validated.kernel,
        initrd: validated.initrd,
        cmdline,
    })
}

pub fn render_grub_entry(iso: &PreparedIso) -> String {
    let mut output =
        String::from("### BEGIN reinstall.sh ###\nset timeout_style=menu\nset timeout=5\n");
    output.push_str(GRUBENV_HELPER);
    output.push_str(
        "menuentry \"reinstall (iso)\" --unrestricted {\n    insmod lvm\n    insmod mdraid1x\n    insmod all_video\n    insmod loopback\n    set btrfs_relative_path=n\n",
    );
    output.push_str(&format!(
        "    search --no-floppy --fs-uuid --set=iso_device '{}'\n    set iso_path='{}'\n    set img_dev='/dev/disk/by-uuid/{}'\n    set img_loop='{}'\n    export iso_path img_dev img_loop\n    loopback loop ($iso_device)$iso_path\n    set root=(loop)\n",
        iso.uuid, iso.grub_path, iso.uuid, iso.grub_path
    ));
    match (&iso.kernel, &iso.initrd, &iso.cmdline, &iso.config) {
        (Some(kernel), Some(initrd), cmdline, _) => {
            output.push_str(&format!(
                "    linux (loop){kernel} {}\n    initrd (loop){initrd}\n",
                cmdline.as_deref().unwrap_or_default()
            ));
        }
        (_, _, _, Some(config)) => {
            output.push_str(&format!("    configfile (loop){config}\n"));
        }
        _ => unreachable!("validated ISO options have exactly one loopback boot mode"),
    }
    output.push_str("}\n### END reinstall.sh ###\n");
    output
}

fn choose_grub_prefix<R: CommandRunner>(
    runner: &mut R,
    image: &std::ffi::OsStr,
) -> Result<(&'static str, Vec<u8>), IsoError> {
    for prefix in ["grub", "grub2"] {
        let program = format!("{prefix}-probe");
        let command = CommandSpec::new(
            &program,
            [
                "--target=abstraction",
                image.to_str().ok_or(IsoError::InvalidPath)?,
            ],
        )
        .phase("local-iso-probe-abstraction")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Stream);
        match runner.run(&command) {
            Ok(output) if output.status == Some(0) => return Ok((prefix, output.stdout)),
            Ok(output) => {
                return Err(IsoError::CommandFailed {
                    program,
                    status: output.status,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && prefix == "grub" => continue,
            Err(error) => return Err(IsoError::Command(error)),
        }
    }
    Err(IsoError::CommandFailed {
        program: "grub-probe/grub2-probe".to_owned(),
        status: None,
    })
}

fn run_capture<R, I, S>(runner: &mut R, program: &str, args: I) -> Result<Vec<u8>, IsoError>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let command = CommandSpec::new(program, args)
        .phase("local-iso-inspect")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Stream);
    let output = runner.run(&command).map_err(IsoError::Command)?;
    if output.status == Some(0) {
        Ok(output.stdout)
    } else {
        Err(IsoError::CommandFailed {
            program: program.to_owned(),
            status: output.status,
        })
    }
}

fn run_checked<R: CommandRunner>(
    runner: &mut R,
    command: &CommandSpec,
    program: &str,
) -> Result<(), IsoError> {
    let output = runner.run(command).map_err(IsoError::Command)?;
    if output.status == Some(0) {
        Ok(())
    } else {
        Err(IsoError::CommandFailed {
            program: program.to_owned(),
            status: output.status,
        })
    }
}

fn inspect_iso_contents(check_dir: &Path, options: &ValidatedIsoOptions) -> Result<(), IsoError> {
    for path in [
        "sources/boot.wim",
        "sources/install.wim",
        "sources/install.esd",
        "SOURCES/BOOT.WIM",
        "SOURCES/INSTALL.WIM",
        "SOURCES/INSTALL.ESD",
    ] {
        if check_dir.join(path).is_file() {
            return Err(IsoError::InvalidFilesystemLayout);
        }
    }
    let boot_files = match (&options.kernel, &options.initrd) {
        (Some(kernel), Some(initrd)) => {
            check_dir.join(kernel.trim_start_matches('/')).is_file()
                && check_dir.join(initrd.trim_start_matches('/')).is_file()
        }
        _ => options
            .config
            .as_deref()
            .is_some_and(|config| check_dir.join(config.trim_start_matches('/')).is_file()),
    };
    if boot_files {
        Ok(())
    } else {
        Err(IsoError::MissingBootFiles)
    }
}

fn trim_command_output(bytes: &[u8]) -> Result<String, IsoError> {
    let value = std::str::from_utf8(bytes).map_err(|_| IsoError::InvalidPath)?;
    Ok(value.trim_end_matches(['\r', '\n']).to_owned())
}

fn safe_grub_path(path: &str) -> bool {
    path.starts_with('/')
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+@~/-".contains(&byte))
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
}

fn safe_cmdline(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._~:/=,+@%!?*- ".contains(&byte))
}

fn has_symlink(path: &Path) -> bool {
    let mut current = Some(path);
    while let Some(path) = current {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return true,
        }
        current = path.parent();
    }
    false
}

fn lexical_absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    }
}
