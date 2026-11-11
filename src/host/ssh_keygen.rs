//! Host-only native ssh-keygen tool. Private key data never enters InstallPlan.
use crate::command::{CommandError, CommandRunner, CommandSpec, run_checked};
use crate::shared::dns::validate_planned_file;
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeyType {
    #[default]
    Ed25519,
    Rsa,
}
impl KeyType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ed25519 => "ed25519",
            Self::Rsa => "rsa",
        }
    }
}
pub const CIPHERS: &[&str] = &[
    "aes256-ctr",
    "aes256-gcm@openssh.com",
    "chacha20-poly1305@openssh.com",
];
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeygenOptions {
    pub key_type: KeyType,
    pub rounds: u32,
    pub cipher: String,
    pub rsa_bits: u16,
    pub comment: String,
    pub output: PathBuf,
}
impl KeygenOptions {
    pub fn validate(&self) -> Result<(), KeygenError> {
        if !(1..=65536).contains(&self.rounds)
            || !CIPHERS.contains(&self.cipher.as_str())
            || (self.key_type == KeyType::Rsa && ![2048, 3072, 4096, 8192].contains(&self.rsa_bits))
            || self.comment.len() > 256
            || self.comment.chars().any(char::is_control)
            || !self.output.is_absolute()
            || self.output.file_name().is_none()
            || self
                .output
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(KeygenError::InvalidOptions);
        }
        for path in [&self.output, &public_path(&self.output)] {
            validate_planned_file(path).map_err(|_| KeygenError::UnsafePath)?;
            match fs::symlink_metadata(path) {
                Ok(_) => return Err(KeygenError::AlreadyExists),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    /// No Shell or -N argument: ssh-keygen prompts for the passphrase itself.
    pub fn command(&self, output: &Path) -> CommandSpec {
        let mut args = vec![
            "-o".into(),
            "-a".into(),
            self.rounds.to_string(),
            "-Z".into(),
            self.cipher.clone(),
            "-t".into(),
            self.key_type.as_str().into(),
        ];
        if self.key_type == KeyType::Rsa {
            args.extend(["-b".into(), self.rsa_bits.to_string()]);
        }
        args.extend(["-C".into(), self.comment.clone(), "-f".into()]);
        let mut spec = CommandSpec::new("ssh-keygen", args).phase("host-ssh-keygen");
        spec.args.push(output.as_os_str().to_owned());
        spec
    }
}

#[derive(Debug)]
pub enum KeygenError {
    InvalidOptions,
    UnsafePath,
    AlreadyExists,
    InvalidPublicKey,
    Filesystem(io::Error),
    Command(CommandError),
}
impl std::fmt::Display for KeygenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOptions => f.write_str("Invalid ssh-keygen options: use an absolute output path, 1-65536 KDF rounds and a supported type/cipher"),
            Self::UnsafePath => f.write_str("Key output requires a real, caller-owned directory without group/other write permission; symlinks are rejected"),
            Self::AlreadyExists => f.write_str("Key/private .pub output already exists; choose another name"),
            Self::InvalidPublicKey => f.write_str("ssh-keygen did not produce the expected bounded public key"),
            Self::Filesystem(e) => write!(f,"SSH key file operation failed: {e}"),
            Self::Command(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for KeygenError {}
impl From<io::Error> for KeygenError {
    fn from(e: io::Error) -> Self {
        Self::Filesystem(e)
    }
}
impl From<CommandError> for KeygenError {
    fn from(e: CommandError) -> Self {
        Self::Command(e)
    }
}

pub struct GeneratedKey {
    pub public_key: String,
    pub private_path: PathBuf,
    pub public_path: PathBuf,
}

pub fn generate<R: CommandRunner>(
    runner: &mut R,
    options: &KeygenOptions,
) -> Result<GeneratedKey, KeygenError> {
    options.validate()?;
    let parent = options.output.parent().ok_or(KeygenError::UnsafePath)?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.uid() != effective_uid() || metadata.mode() & 0o022 != 0 {
        return Err(KeygenError::UnsafePath);
    }
    let directory = parent.join(format!(
        ".rustaller-keygen-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    DirBuilder::new().mode(0o700).create(&directory)?;
    let staging = directory.join("key");
    let result = (|| {
        run_checked(runner, &options.command(&staging))?;
        let private = fs::symlink_metadata(&staging)?;
        let public = public_path(&staging);
        let metadata = fs::symlink_metadata(&public)?;
        if !private.is_file()
            || private.nlink() != 1
            || !metadata.is_file()
            || metadata.len() > 8192
            || metadata.nlink() != 1
        {
            return Err(KeygenError::InvalidPublicKey);
        }
        // Read only the public key, never the private file or its passphrase.
        let text = fs::read_to_string(&public)?;
        let text = text.trim_end_matches(['\n', '\r']);
        let mut fields = text.split_whitespace();
        let expected = match options.key_type {
            KeyType::Ed25519 => "ssh-ed25519",
            KeyType::Rsa => "ssh-rsa",
        };
        if text.chars().any(char::is_control)
            || fields.next() != Some(expected)
            || fields.next().is_none_or(|s| {
                s.is_empty()
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
            })
        {
            return Err(KeygenError::InvalidPublicKey);
        }
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o600))?;
        fs::set_permissions(&public, fs::Permissions::from_mode(0o644))?;
        let destination_pub = public_path(&options.output);
        // A second, exclusive publication check avoids ssh-keygen overwrite prompts.
        fs::hard_link(&staging, &options.output)?;
        if let Err(e) = fs::hard_link(&public, &destination_pub) {
            let _ = fs::remove_file(&options.output);
            return Err(e.into());
        }
        Ok(GeneratedKey {
            public_key: text.to_owned(),
            private_path: options.output.clone(),
            public_path: destination_pub,
        })
    })();
    // Only our unique private staging directory is removed; never existing keys.
    let _ = fs::remove_dir_all(&directory);
    result
}

fn public_path(path: &Path) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(".pub");
    path.into()
}
unsafe extern "C" {
    fn geteuid() -> u32;
}
fn effective_uid() -> u32 {
    unsafe { geteuid() }
}

pub fn date_comment() -> String {
    // Same system-date behavior as date +%Y%m%d, without command substitution.
    std::process::Command::new("date")
        .arg("+%Y%m%d")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| text.len() == 8 && text.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or_else(|| "rustaller".into())
}
