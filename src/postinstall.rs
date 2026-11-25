//! Typed target configuration prepared before the target's first boot.
//!
//! This module only validates and plans. It never runs a package manager or
//! writes to the host. Applying the plan belongs to the validated Live workflow.

use crate::model::Target;
use std::fmt;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PostInstallConfig {
    pub hostname: Option<String>,
    pub timezone: Option<String>,
}

/// The required target package is independent of the Live kernel and firmware.
pub const fn required_kernel(target: Target) -> Option<&'static str> {
    let package = match target {
        Target::Alpine => "linux-virt",
        Target::Debian => "linux-image-cloud-amd64",
        Target::Arch => "linux-cachyos-server",
        Target::Gentoo => "sys-kernel/gentoo-kernel-bin",
        Target::Reset => return None,
    };
    Some(package)
}

/// Configuration data for a target root; paths are relative to that root.
/// The zonefile must be checked in the installed target after tzdata is present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigurationPlan {
    pub kernel: &'static str,
    pub hostname_contents: Option<String>,
    pub timezone_contents: Option<String>,
    pub zoneinfo_relative_path: Option<String>,
    pub extra_packages: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostInstallError {
    InvalidHostname,
    InvalidTimezone,
    UnexpectedRoute,
}

impl fmt::Display for PostInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHostname => {
                f.write_str("Hostname must be an ASCII label of 1-63 letters, digits or hyphens")
            }
            Self::InvalidTimezone => f.write_str("Timezone must be a relative IANA zone name"),
            Self::UnexpectedRoute => {
                f.write_str("Target configuration is only available for automatic installation")
            }
        }
    }
}

impl std::error::Error for PostInstallError {}

impl PostInstallConfig {
    pub fn is_default(&self) -> bool {
        self.hostname.is_none() && self.timezone.is_none()
    }

    pub fn validate(&self, target: Target, hold: Option<u8>) -> Result<(), PostInstallError> {
        if !self.is_default() && (!target.is_automatic() || hold == Some(1)) {
            return Err(PostInstallError::UnexpectedRoute);
        }
        if let Some(hostname) = self.hostname.as_deref()
            && !valid_hostname(hostname)
        {
            return Err(PostInstallError::InvalidHostname);
        }
        if let Some(timezone) = self.timezone.as_deref()
            && !valid_timezone(timezone)
        {
            return Err(PostInstallError::InvalidTimezone);
        }
        Ok(())
    }

    pub fn prepare(&self, target: Target) -> Result<ConfigurationPlan, PostInstallError> {
        self.validate(target, None)?;
        let kernel = required_kernel(target).ok_or(PostInstallError::UnexpectedRoute)?;
        Ok(ConfigurationPlan {
            kernel,
            hostname_contents: self.hostname.as_ref().map(|name| format!("{name}\n")),
            timezone_contents: self.timezone.as_ref().map(|zone| format!("{zone}\n")),
            zoneinfo_relative_path: self
                .timezone
                .as_ref()
                .map(|zone| format!("usr/share/zoneinfo/{zone}")),
            extra_packages: if self.timezone.is_some() {
                vec![if target == Target::Gentoo {
                    "sys-libs/timezone-data"
                } else {
                    "tzdata"
                }]
            } else {
                Vec::new()
            },
        })
    }
}

fn valid_hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_timezone(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_+-".contains(&byte))
        })
}
