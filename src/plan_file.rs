use crate::codec::{PlanCodecError, decode_plan, encode_plan};
use crate::model::InstallPlan;
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::raw::c_long;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct PrivatePlanFile {
    path: PathBuf,
    remove_on_drop: bool,
}

impl PrivatePlanFile {
    pub fn create(path: impl AsRef<Path>, plan: &InstallPlan) -> Result<Self, PlanFileError> {
        plan.validate()
            .map_err(|error| PlanFileError::Codec(PlanCodecError::InvalidPlan(error)))?;
        let path = path.as_ref().to_owned();
        if path.parent().is_none() {
            return Err(PlanFileError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "plan file path must have a parent directory",
            )));
        }
        let mut data = encode_plan(plan).map_err(PlanFileError::Codec)?;
        if data.is_empty() {
            return Err(PlanFileError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "refusing to write an empty install plan",
            )));
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| PlanFileError::Io(io::Error::other(error)))?
            .as_nanos();
        let mut temporary_name = path.as_os_str().to_owned();
        temporary_name.push(format!(".tmp.{}.{nonce}", std::process::id()));
        let temporary = PathBuf::from(temporary_name);

        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&data)?;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            drop(file);
            rename_noreplace(&temporary, &path)
        })();
        data.fill(0);
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(PlanFileError::Io(error));
        }
        Ok(Self {
            path,
            remove_on_drop: true,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, PlanFileError> {
        let mut file = Self {
            path: path.as_ref().to_owned(),
            remove_on_drop: false,
        };
        file.validate_metadata()?;
        file.remove_on_drop = true;
        Ok(file)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn persist(&mut self) -> &Path {
        self.remove_on_drop = false;
        &self.path
    }

    pub fn read(&self) -> Result<InstallPlan, PlanFileError> {
        self.validate_metadata()?;
        let mut bytes = fs::read(&self.path)?;
        let decoded = decode_plan(&bytes).map_err(PlanFileError::Codec);
        bytes.fill(0);
        decoded
    }

    fn validate_metadata(&self) -> Result<(), PlanFileError> {
        let metadata = fs::symlink_metadata(&self.path)?;
        if !metadata.file_type().is_file()
            || metadata.uid() != effective_uid()
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.len() > 4 * 1024 * 1024
        {
            return Err(PlanFileError::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "install plan file must be a private regular file",
            )));
        }
        Ok(())
    }
}

impl Drop for PrivatePlanFile {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}

unsafe extern "C" {
    fn geteuid() -> u32;
    fn syscall(number: c_long, ...) -> c_long;
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments, pointers or caller-side preconditions.
    unsafe { geteuid() }
}

fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let from_c = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in temporary path"))?;
    let to_c = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in plan path"))?;
    // Linux x86_64 renameat2(RENAME_NOREPLACE). Use the syscall directly because
    // the musl target does not expose a renameat2 libc wrapper.
    // SAFETY: the supported ABI is Linux x86_64 (SYS_renameat2 = 316).
    // Both CStrings remain alive and NUL-terminated throughout the call;
    // AT_FDCWD and RENAME_NOREPLACE are passed with their native integer types.
    let result = unsafe {
        syscall(
            316,
            -100_i32,
            from_c.as_ptr(),
            -100_i32,
            to_c.as_ptr(),
            1_u32,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if matches!(error.raw_os_error(), Some(22 | 38 | 95)) {
        // A hard link also publishes atomically without replacing an existing
        // file. Checking existence then renaming would permit a concurrent
        // writer's plan to be overwritten on older kernels.
        fs::hard_link(from, to)?;
        return fs::remove_file(from);
    }
    Err(error)
}

#[derive(Debug)]
pub enum PlanFileError {
    Io(io::Error),
    Codec(PlanCodecError),
}

impl std::fmt::Display for PlanFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "private install plan file error: {error}"),
            Self::Codec(error) => write!(f, "install plan encoding error: {error}"),
        }
    }
}

impl std::error::Error for PlanFileError {}

impl From<io::Error> for PlanFileError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
