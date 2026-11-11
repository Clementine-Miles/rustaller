use crate::bundle::BundleArtifacts;
use crate::{contains_bytes, verify_x86_64_elf};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const ENTRY_NAME: &str = "reinstall-x86_64-linux-musl";

pub struct SourceRevision {
    pub commit: String,
    pub dirty: bool,
}

pub fn validate_tag(tag: &str) -> Result<(), String> {
    let expected = format!("v{}", reinstall_rs::PACKAGE_VERSION);
    let valid = tag == expected
        || tag
            .strip_prefix(&format!("{expected}-"))
            .is_some_and(|suffix| {
                !suffix.is_empty()
                    && suffix.split('.').all(|part| {
                        !part.is_empty()
                            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                            && !(part.bytes().all(|b| b.is_ascii_digit())
                                && part.len() > 1
                                && part.starts_with('0'))
                    })
            });
    if valid && tag.len() <= 96 {
        Ok(())
    } else {
        Err(format!(
            "tag must be {expected} or {expected}-PRERELEASE, using SemVer identifiers"
        ))
    }
}

pub fn source_revision(root: &Path, allow_dirty: bool) -> Result<SourceRevision, String> {
    let read = |args: &[&str]| -> Result<String, String> {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("cannot read Git source revision/status".into());
        }
        String::from_utf8(output.stdout).map_err(|e| e.to_string())
    };
    let commit = read(&["rev-parse", "HEAD"])?.trim().to_owned();
    let dirty = !read(&["status", "--porcelain", "--untracked-files=normal"])?
        .trim()
        .is_empty();
    if dirty && !allow_dirty {
        return Err("source tree is dirty; commit your reviewed changes before a release, or use --allow-dirty for a local preview".into());
    }
    Ok(SourceRevision { commit, dirty })
}

pub fn package(
    root: &Path,
    artifacts: &BundleArtifacts,
    output: &Path,
    tag: &str,
    source: &SourceRevision,
) -> Result<(), String> {
    validate_tag(tag)?;
    if source.commit.len() != 40 || !source.commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("source commit must be a full Git SHA-1".into());
    }
    if fs::symlink_metadata(output).is_ok() || output.file_name().is_none() {
        return Err("output must be a new directory".into());
    }
    let host = fs::read(&artifacts.host).map_err(|e| e.to_string())?;
    let worker = fs::read(&artifacts.worker).map_err(|e| e.to_string())?;
    for bytes in [&host, &worker] {
        verify_x86_64_elf(bytes, true, reinstall_rs::RUNTIME_VERSION).map_err(|e| e.to_string())?;
    }
    if host.len() <= worker.len() || !contains_bytes(&host, &worker) {
        return Err("release host does not embed the exact matching worker".into());
    }
    let license = fs::read(root.join("LICENSE")).map_err(|e| e.to_string())?;
    if license.is_empty() {
        return Err("release LICENSE is empty".into());
    }
    let info = format!(
        "{{\n  \"schema_version\": 1,\n  \"tag\": \"{tag}\",\n  \"package_version\": \"{}\",\n  \"runtime_version\": \"{}\",\n  \"plan_schema_version\": {},\n  \"target\": \"{}\",\n  \"installer_complete\": false,\n  \"tui\": {},\n  \"source_commit\": \"{}\",\n  \"source_dirty\": {},\n  \"host_bytes\": {},\n  \"worker_bytes\": {},\n  \"worker_sha256\": \"{}\"\n}}\n",
        reinstall_rs::PACKAGE_VERSION,
        reinstall_rs::RUNTIME_VERSION,
        reinstall_rs::model::CURRENT_SCHEMA_VERSION,
        crate::bundle::TARGET,
        artifacts.tui,
        source.commit,
        source.dirty,
        host.len(),
        worker.len(),
        sha256(&worker),
    );
    let notes = format!(
        "# {tag} — development preview\n\nThis build contains real kexec handoff and disk-erasing installation actions. Full four-target BIOS/UEFI acceptance is pending; installer_complete remains false. Use disposable test machines with console access.\n\n- Linux x86_64 static musl entry binary; matching worker embedded.\n- TUI: {}.\n- Automatic targets: Alpine, Debian, Arch and Gentoo through Alpine Live. Source PID 1 must be systemd or openrc-init.\n- --inspect is read-only; an installation requires the complete identity-bound confirmation token.\n- The native ssh-keygen TUI tool writes local key files; back up the private key before erasing its disk.\n- Current-system configuration can persist hostname, timezone, DNS, network and SSH changes.\n- Rust ISO, netboot.xyz and reset execution are not connected.\n- Source commit: `{}`; uncommitted changes: {}.\n- Runtime compatibility: `{}`.\n- Keep this release as a draft/prerelease until acceptance is complete.\n\nDownload `{ENTRY_NAME}` and verify it against `SHA256SUMS`. Source and the GPL-3.0 license are provided by this repository.\n",
        artifacts.tui,
        source.commit,
        source.dirty,
        reinstall_rs::RUNTIME_VERSION
    );
    let files = [
        (ENTRY_NAME, host.as_slice(), 0o755),
        ("LICENSE", license.as_slice(), 0o644),
        ("build-info.json", info.as_bytes(), 0o644),
    ];
    let sums = files
        .iter()
        .map(|(name, bytes, _)| format!("{}  {name}\n", sha256(bytes)))
        .collect::<String>();
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let staging = parent.join(format!(".reinstall-dist-{}-{nonce}", std::process::id()));
    fs::create_dir(&staging).map_err(|e| e.to_string())?;
    let result = (|| -> std::io::Result<()> {
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))?;
        for (name, bytes, mode) in files.into_iter().chain([
            ("SHA256SUMS", sums.as_bytes(), 0o644),
            ("RELEASE-NOTES.md", notes.as_bytes(), 0o644),
        ]) {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .open(staging.join(name))?;
            file.write_all(bytes)?;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
            file.sync_all()?;
        }
        if fs::symlink_metadata(output).is_ok() {
            return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
        }
        fs::rename(&staging, output)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result.map_err(|e| e.to_string())
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "reinstall-dist-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            fs::write(path.join("LICENSE"), b"license fixture\n").unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn elf() -> Vec<u8> {
        let mut bytes = vec![0; 120];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(reinstall_rs::RUNTIME_VERSION.as_bytes());
        bytes
    }
    fn artifacts(scratch: &Scratch) -> BundleArtifacts {
        let worker = elf();
        let mut host = elf();
        host.extend_from_slice(&worker);
        fs::write(scratch.0.join("worker"), worker).unwrap();
        fs::write(scratch.0.join("host"), host).unwrap();
        BundleArtifacts {
            host: scratch.0.join("host"),
            worker: scratch.0.join("worker"),
            tui: true,
        }
    }
    fn source() -> SourceRevision {
        SourceRevision {
            commit: "a".repeat(40),
            dirty: false,
        }
    }
    #[test]
    fn release_tag_is_bound_to_the_package_version_and_cannot_inject_paths() {
        assert!(validate_tag("v0.1.0-preview.1").is_ok());
        for tag in [
            "../v0.1.0",
            "v0.1.0;ls",
            "v0.1.0-dev/1",
            "v0.1.0-dev..1",
            "v9.0.0",
            "v0.1.0-01",
        ] {
            assert!(validate_tag(tag).is_err(), "{tag}");
        }
    }
    #[test]
    fn package_contains_only_one_entry_binary_and_integrity_metadata() {
        let scratch = Scratch::new();
        let artifacts = artifacts(&scratch);
        let output = scratch.0.join("dist");
        package(
            &scratch.0,
            &artifacts,
            &output,
            "v0.1.0-preview.1",
            &source(),
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join(ENTRY_NAME)).unwrap(),
            fs::read(artifacts.host).unwrap()
        );
        assert!(!output.join("worker").exists());
        assert!(!output.join("trans.sh").exists());
        let metadata = fs::read_to_string(output.join("build-info.json")).unwrap();
        assert!(metadata.contains("\"installer_complete\": false"));
        assert!(metadata.contains("\"tui\": true"));
        let sums = fs::read_to_string(output.join("SHA256SUMS")).unwrap();
        assert_eq!(sums.lines().count(), 3);
        for line in sums.lines() {
            let (hash, filename) = line.split_once("  ").unwrap();
            assert_eq!(hash, sha256(&fs::read(output.join(filename)).unwrap()));
        }
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(output.join(ENTRY_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
    #[test]
    fn tampered_host_and_existing_output_are_rejected_without_overwriting() {
        let scratch = Scratch::new();
        let artifacts = artifacts(&scratch);
        fs::write(&artifacts.host, elf()).unwrap();
        let output = scratch.0.join("dist");
        assert!(
            package(
                &scratch.0,
                &artifacts,
                &output,
                "v0.1.0-preview.1",
                &source()
            )
            .is_err()
        );
        assert!(!output.exists());
        let artifacts = self::artifacts(&scratch);
        fs::create_dir(&output).unwrap();
        fs::write(output.join("keep"), b"keep").unwrap();
        assert!(
            package(
                &scratch.0,
                &artifacts,
                &output,
                "v0.1.0-preview.1",
                &source()
            )
            .is_err()
        );
        assert_eq!(fs::read(output.join("keep")).unwrap(), b"keep");
    }
}
