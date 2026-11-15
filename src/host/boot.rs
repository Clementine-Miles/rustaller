//! Alpine Live kernel command line; target boot configuration belongs to Limine.
use crate::targets::install::TTY_COMMAND_LINE;
use std::fmt;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootError {
    InvalidAlpineRepository,
    InvalidModloopPath,
}
impl fmt::Display for BootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidAlpineRepository => "Invalid Alpine repository",
            Self::InvalidModloopPath => "Invalid Alpine modloop path",
        })
    }
}
impl std::error::Error for BootError {}
pub fn build_nextos_cmdline(repo: &str, modloop: &str) -> Result<String, BootError> {
    let valid_repo = repo.strip_prefix("https://").is_some_and(|remainder| {
        let host = remainder.split('/').next().unwrap_or_default();
        !host.is_empty()
            && host.bytes().any(|byte| byte.is_ascii_alphanumeric())
            && repo.bytes().all(is_path_byte)
    });
    if !valid_repo {
        return Err(BootError::InvalidAlpineRepository);
    }
    if !modloop.starts_with('/')
        || !modloop.bytes().all(is_path_byte)
        || modloop.split('/').any(|segment| segment == "..")
    {
        return Err(BootError::InvalidModloopPath);
    }
    Ok(format!(
        "alpine_repo={repo} modloop={modloop} {TTY_COMMAND_LINE}"
    ))
}

fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"._~:/-".contains(&byte)
}
