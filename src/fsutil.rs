//! Atomic replacement in a caller-owned directory. Path confinement belongs to the caller.
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn replace_config(path: &Path, contents: &[u8]) -> io::Result<()> {
    if contents.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to write empty configuration",
        ));
    }
    let mode = fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o7777)
        .unwrap_or(0o644);
    atomic_replace(path, contents, mode)
}

pub(crate) fn atomic_replace(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("file has no parent directory"))?;
    for _ in 0..16 {
        let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".rustaller.tmp.{}.{sequence}", std::process::id()));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let result = (|| {
            file.write_all(contents)?;
            file.set_permissions(Permissions::from_mode(mode))?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot allocate a unique temporary file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "rustaller-fs-{}-{}",
                std::process::id(),
                NEXT_FILE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn configuration_replacement_retains_private_mode_and_rejects_empty_content() {
        let directory = Directory::new();
        let path = directory.0.join("config");
        atomic_replace(&path, b"old", 0o600).unwrap();
        replace_config(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(replace_config(&path, b"").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn replacing_a_symlink_does_not_write_its_target() {
        let directory = Directory::new();
        let outside = directory.0.join("outside");
        let path = directory.0.join("config");
        fs::write(&outside, b"untouched").unwrap();
        symlink(&outside, &path).unwrap();
        replace_config(&path, b"replacement").unwrap();
        assert_eq!(fs::read(&outside).unwrap(), b"untouched");
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn failed_publication_cleans_only_its_own_temporary_file() {
        let directory = Directory::new();
        let destination = directory.0.join("existing-directory");
        fs::create_dir(&destination).unwrap();
        let unrelated = directory.0.join("config.tmp");
        fs::write(&unrelated, b"keep").unwrap();
        assert!(atomic_replace(&destination, b"configuration", 0o644).is_err());
        assert!(destination.is_dir());
        assert_eq!(fs::read(&unrelated).unwrap(), b"keep");
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 2);
    }
}
