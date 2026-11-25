use crate::config::{GentooInit, SystemConfig, SystemConfigError};
use crate::postinstall::{PostInstallConfig, PostInstallError};
use std::fmt;

// Schema 10 removes the Live web port. Reject older bodies before decoding
// positional fields (including the Gentoo init selection introduced in v9).
pub const CURRENT_SCHEMA_VERSION: u16 = 11;

pub fn valid_account_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && !value.eq_ignore_ascii_case("none")
        && value
            .as_bytes()
            .first()
            .is_some_and(|first| first.is_ascii_alphabetic() || *first == b'_')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Alpine,
    Debian,
    Arch,
    Gentoo,
    Reset,
}

impl Target {
    pub const fn minimum_ram_mb(self) -> u64 {
        match self {
            Self::Alpine | Self::Debian => 512,
            Self::Arch => 1024,
            Self::Gentoo => 2048,
            Self::Reset => 0,
        }
    }

    pub const fn is_automatic(self) -> bool {
        matches!(
            self,
            Self::Alpine | Self::Debian | Self::Arch | Self::Gentoo
        )
    }
}

pub fn validate_host_arch(machine: &str) -> Result<(), ArchitectureError> {
    if machine == "x86_64" {
        Ok(())
    } else {
        Err(ArchitectureError(machine.to_owned()))
    }
}

pub fn validate_ram(target: Target, available_mb: u64) -> Result<(), RamError> {
    let required_mb = target.minimum_ram_mb();
    if available_mb >= required_mb {
        Ok(())
    } else {
        Err(RamError {
            target,
            required_mb,
            available_mb,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchitectureError(pub String);

impl fmt::Display for ArchitectureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Only Linux x86_64 is supported (reported: {})", self.0)
    }
}

impl std::error::Error for ArchitectureError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RamError {
    pub target: Target,
    pub required_mb: u64,
    pub available_mb: u64,
}

impl fmt::Display for RamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} requires at least {} MB RAM (available: {} MB)",
            self.target, self.required_mb, self.available_mb
        )
    }
}

impl std::error::Error for RamError {}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Alpine => "Alpine",
            Self::Debian => "Debian",
            Self::Arch => "Arch",
            Self::Gentoo => "Gentoo",
            Self::Reset => "reset",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootMode {
    Bios,
    Efi,
}

impl BootMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bios => "bios",
            Self::Efi => "efi",
        }
    }
}

pub struct SecretString(Vec<u8>);

impl SecretString {
    pub fn new(value: impl AsRef<str>) -> Self {
        Self(value.as_ref().as_bytes().to_vec())
    }

    pub fn expose(&self) -> &str {
        std::str::from_utf8(&self.0).expect("SecretString only accepts UTF-8 input")
    }

    pub fn from_bytes(value: Vec<u8>) -> Result<Self, std::str::Utf8Error> {
        std::str::from_utf8(&value)?;
        Ok(Self(value))
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString([REDACTED])")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub struct InstallPlan {
    pub schema_version: u16,
    pub target: Target,
    pub username: Option<String>,
    pub password: Option<SecretString>,
    pub root_password: Option<SecretString>,
    pub ssh_keys: Vec<String>,
    pub ssh_port: Option<u16>,
    pub frpc_config: Option<String>,
    pub target_disk: Option<String>,
    pub no_cloud_kernel: bool,
    pub post_install: PostInstallConfig,
    pub system: SystemConfig,
    pub hold: Option<u8>,
    pub force_boot_mode: Option<BootMode>,
    pub debug: bool,
}

impl fmt::Debug for InstallPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstallPlan")
            .field("schema_version", &self.schema_version)
            .field("target", &self.target)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field(
                "root_password",
                &self.root_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("ssh_keys", &self.ssh_keys)
            .field("ssh_port", &self.ssh_port)
            .field(
                "frpc_config",
                &self.frpc_config.as_ref().map(|_| "[REDACTED]"),
            )
            .field("target_disk", &self.target_disk)
            .field("no_cloud_kernel", &self.no_cloud_kernel)
            .field("post_install", &self.post_install)
            .field("system", &self.system)
            .field("hold", &self.hold)
            .field("force_boot_mode", &self.force_boot_mode)
            .field("debug", &self.debug)
            .finish()
    }
}

impl InstallPlan {
    pub fn new(target: Target) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            target,
            username: None,
            password: None,
            root_password: None,
            ssh_keys: Vec::new(),
            ssh_port: if target.is_automatic() {
                Some(22)
            } else {
                None
            },
            frpc_config: None,
            target_disk: None,
            no_cloud_kernel: false,
            post_install: PostInstallConfig::default(),
            system: SystemConfig {
                gentoo_init: (target == Target::Gentoo).then_some(GentooInit::Openrc),
                ..SystemConfig::default()
            },
            hold: None,
            force_boot_mode: None,
            debug: false,
        }
    }

    pub fn validate(&self) -> Result<(), PlanError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(PlanError::UnsupportedSchema(self.schema_version));
        }
        if self.target == Target::Gentoo
            && self.hold != Some(1)
            && self.system.gentoo_init.is_none()
        {
            return Err(PlanError::SystemConfig(
                SystemConfigError::MissingGentooInit,
            ));
        }
        self.post_install
            .validate(self.target, self.no_cloud_kernel, self.hold)
            .map_err(PlanError::PostInstall)?;
        self.system
            .validate(self.target, self.hold)
            .map_err(PlanError::SystemConfig)?;

        if self.ssh_port == Some(0) {
            return Err(PlanError::InvalidPort("SSH"));
        }

        if self.hold.is_some_and(|value| value > 2) {
            return Err(PlanError::InvalidHold);
        }

        if self.password.is_some() && !self.ssh_keys.is_empty() {
            return Err(PlanError::PasswordAndSshKey);
        }
        if [&self.password, &self.root_password].iter().any(|secret| {
            secret.as_ref().is_some_and(|value| {
                value.expose().is_empty()
                    || value.expose().len() > 8192
                    || value.expose().chars().any(char::is_control)
            })
        }) {
            return Err(PlanError::InvalidPassword);
        }

        if let Some(username) = self.username.as_deref()
            && !valid_account_name(username)
        {
            return Err(PlanError::InvalidUsername);
        }

        let non_root_install = self.target.is_automatic()
            && self.hold != Some(1)
            && self.username.as_deref().is_some_and(|name| name != "root");
        if !non_root_install && self.root_password.is_some() {
            return Err(PlanError::UnexpectedOption("root-password"));
        }

        Ok(())
    }

    /// Required only when dispatching an installation, not while inspecting a
    /// disk or editing an incomplete plan. No credential values enter errors.
    pub fn validate_install_credentials(&self) -> Result<(), PlanError> {
        self.validate()?;
        if self.target.is_automatic()
            && self.hold != Some(1)
            && self.username.as_deref().is_some_and(|name| name != "root")
            && self.root_password.is_none()
        {
            return Err(PlanError::MissingRootPassword);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanError {
    UnsupportedSchema(u16),
    InvalidPort(&'static str),
    InvalidHold,
    PasswordAndSshKey,
    InvalidUsername,
    InvalidPassword,
    MissingRootPassword,
    UnexpectedOption(&'static str),
    PostInstall(PostInstallError),
    SystemConfig(SystemConfigError),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(version) => {
                write!(f, "Unsupported install plan schema: {version}")
            }
            Self::InvalidPort(name) => write!(f, "Invalid {name} port"),
            Self::InvalidHold => f.write_str("Invalid hold value"),
            Self::PasswordAndSshKey => f.write_str("Password and SSH key are mutually exclusive"),
            Self::InvalidUsername => f.write_str("Invalid username"),
            Self::InvalidPassword => f.write_str("Password must be a nonempty single line of at most 8192 bytes without control characters"),
            Self::MissingRootPassword => f.write_str("A non-root installation account requires a separate root password"),
            Self::UnexpectedOption(name) => {
                write!(f, "Options are not valid for this route: {name}")
            }
            Self::PostInstall(error) => error.fmt(f),
            Self::SystemConfig(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for PlanError {}
