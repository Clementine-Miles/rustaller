use crate::config::{SystemConfig, SystemConfigError};
use crate::postinstall::{PostInstallConfig, PostInstallError};
use std::fmt;

pub const CURRENT_SCHEMA_VERSION: u16 = 5;

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
    LocalIso,
    NetbootXyz,
    Reset,
}

impl Target {
    pub const fn minimum_ram_mb(self) -> u64 {
        match self {
            Self::Alpine | Self::Debian => 512,
            Self::Arch => 1024,
            Self::Gentoo => 2048,
            Self::LocalIso | Self::NetbootXyz | Self::Reset => 0,
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
            Self::LocalIso => "local ISO",
            Self::NetbootXyz => "netboot.xyz",
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsoOptions {
    pub path: String,
    pub config: Option<String>,
    pub kernel: Option<String>,
    pub initrd: Option<String>,
    pub cmdline: Option<String>,
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
    pub ssh_keys: Vec<String>,
    pub ssh_port: Option<u16>,
    pub web_port: Option<u16>,
    pub frpc_config: Option<String>,
    pub target_disk: Option<String>,
    pub no_cloud_kernel: bool,
    pub post_install: PostInstallConfig,
    pub system: SystemConfig,
    pub hold: Option<u8>,
    pub force_boot_mode: Option<BootMode>,
    pub confhome: Option<String>,
    pub iso: Option<IsoOptions>,
    pub debug: bool,
}

impl fmt::Debug for InstallPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstallPlan")
            .field("schema_version", &self.schema_version)
            .field("target", &self.target)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("ssh_keys", &self.ssh_keys)
            .field("ssh_port", &self.ssh_port)
            .field("web_port", &self.web_port)
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
            .field("confhome", &self.confhome.as_ref().map(|_| "[CONFIGURED]"))
            .field("iso", &self.iso)
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
            ssh_keys: Vec::new(),
            ssh_port: if target.is_automatic() {
                Some(22)
            } else {
                None
            },
            web_port: if target.is_automatic() {
                Some(80)
            } else {
                None
            },
            frpc_config: None,
            target_disk: None,
            no_cloud_kernel: false,
            post_install: PostInstallConfig::default(),
            system: SystemConfig::default(),
            hold: None,
            force_boot_mode: None,
            confhome: None,
            iso: None,
            debug: false,
        }
    }

    pub fn validate(&self) -> Result<(), PlanError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(PlanError::UnsupportedSchema(self.schema_version));
        }
        self.post_install
            .validate(self.target, self.no_cloud_kernel, self.hold)
            .map_err(PlanError::PostInstall)?;
        self.system
            .validate(self.target, self.hold)
            .map_err(PlanError::SystemConfig)?;

        for (name, port) in [("SSH", self.ssh_port), ("web", self.web_port)] {
            if port.is_some_and(|value| value == 0) {
                return Err(PlanError::InvalidPort(name));
            }
        }

        if self.hold.is_some_and(|value| value > 2) {
            return Err(PlanError::InvalidHold);
        }

        if self.password.is_some() && !self.ssh_keys.is_empty() {
            return Err(PlanError::PasswordAndSshKey);
        }
        if self.password.as_ref().is_some_and(|value| {
            value.expose().is_empty()
                || value.expose().len() > 8192
                || value.expose().chars().any(char::is_control)
        }) {
            return Err(PlanError::InvalidPassword);
        }

        if let Some(username) = self.username.as_deref()
            && !valid_account_name(username)
        {
            return Err(PlanError::InvalidUsername);
        }

        if self.target.is_automatic() {
            // Rust carries its matching worker/UI assets and authenticates
            // distribution repositories directly. A legacy resource override
            // is optional, but must remain safe whenever explicitly supplied.
            if self
                .confhome
                .as_deref()
                .is_some_and(|confhome| !valid_https_resource_root(confhome))
            {
                return Err(PlanError::MissingHttpsConfhome);
            }
            if self.iso.is_some() {
                return Err(PlanError::UnexpectedOption("ISO"));
            }
            return Ok(());
        }

        match self.target {
            Target::LocalIso => {
                let Some(iso) = self.iso.as_ref() else {
                    return Err(PlanError::MissingIso);
                };
                if iso.path.is_empty() {
                    return Err(PlanError::MissingIso);
                }
                let custom_boot = iso.kernel.is_some() || iso.initrd.is_some();
                if custom_boot != (iso.kernel.is_some() && iso.initrd.is_some()) {
                    return Err(PlanError::IncompleteIsoBoot);
                }
                if iso.cmdline.is_some() && !custom_boot {
                    return Err(PlanError::IncompleteIsoBoot);
                }
                if iso.config.is_some() && custom_boot {
                    return Err(PlanError::ConflictingIsoBoot);
                }
                if self.has_unsupported_interactive_options() {
                    return Err(PlanError::UnexpectedOption("automatic-install"));
                }
            }
            Target::NetbootXyz => {
                if self.iso.is_some() || self.has_unsupported_interactive_options() {
                    return Err(PlanError::UnexpectedOption("automatic-install"));
                }
            }
            Target::Reset => {
                if self.iso.is_some() {
                    return Err(PlanError::UnexpectedOption("ISO"));
                }
            }
            Target::Alpine | Target::Debian | Target::Arch | Target::Gentoo => unreachable!(),
        }
        Ok(())
    }

    fn has_unsupported_interactive_options(&self) -> bool {
        self.username.is_some()
            || self.password.is_some()
            || !self.ssh_keys.is_empty()
            || self.ssh_port.is_some()
            || self.web_port.is_some()
            || self.frpc_config.is_some()
            || self.target_disk.is_some()
            || self.no_cloud_kernel
            || self.hold.is_some()
    }
}

fn valid_https_resource_root(value: &str) -> bool {
    let base = value.trim_end_matches('/');
    let Some(remainder) = base.strip_prefix("https://") else {
        return false;
    };
    if base.len() <= "https://".len()
        || !base
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._~:/-".contains(&byte))
    {
        return false;
    }
    let host = remainder.split('/').next().unwrap_or_default();
    if host.is_empty() || !host.bytes().any(|byte| byte.is_ascii_alphanumeric()) {
        return false;
    }
    if remainder
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return false;
    }
    let lower = base.to_ascii_lowercase();
    [
        "https://raw.githubusercontent.com/bin456789/reinstall",
        "https://cnb.cool/bin456789/reinstall",
    ]
    .iter()
    .all(|upstream| lower != *upstream && !lower.starts_with(&format!("{upstream}/")))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanError {
    UnsupportedSchema(u16),
    MissingHttpsConfhome,
    InvalidPort(&'static str),
    InvalidHold,
    PasswordAndSshKey,
    InvalidUsername,
    InvalidPassword,
    MissingIso,
    IncompleteIsoBoot,
    ConflictingIsoBoot,
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
            Self::MissingHttpsConfhome => {
                f.write_str("An explicit resource base must be a supported HTTPS URL")
            }
            Self::InvalidPort(name) => write!(f, "Invalid {name} port"),
            Self::InvalidHold => f.write_str("Invalid hold value"),
            Self::PasswordAndSshKey => f.write_str("Password and SSH key are mutually exclusive"),
            Self::InvalidUsername => f.write_str("Invalid username"),
            Self::InvalidPassword => f.write_str("Password must be a nonempty single line of at most 8192 bytes without control characters"),
            Self::MissingIso => f.write_str("Local ISO mode requires an ISO path"),
            Self::IncompleteIsoBoot => {
                f.write_str("Custom ISO boot requires both kernel and initrd")
            }
            Self::ConflictingIsoBoot => {
                f.write_str("Use either ISO loopback config or custom kernel/initrd")
            }
            Self::UnexpectedOption(name) => {
                write!(f, "Options are not valid for this route: {name}")
            }
            Self::PostInstall(error) => error.fmt(f),
            Self::SystemConfig(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for PlanError {}
