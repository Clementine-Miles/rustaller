//! Typed target configuration prepared before the target's first boot.
//!
//! This module only validates and plans. It never runs a package manager or
//! writes to the host. Applying the plan belongs to the validated Live workflow.

use crate::model::Target;
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KernelChoice {
    #[default]
    Auto,
    Cloud,
    Generic,
    Lts,
    Standard,
    Virt,
    Cachyos,
}

impl KernelChoice {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cloud => "cloud",
            Self::Generic => "generic",
            Self::Lts => "lts",
            Self::Standard => "standard",
            Self::Virt => "virt",
            Self::Cachyos => "cachyos",
        }
    }

    pub const fn choices_for(target: Target) -> &'static str {
        match target {
            Target::Alpine => "standard/virt/lts",
            Target::Debian => "standard/cloud/generic",
            Target::Arch => "standard/lts/cachyos",
            Target::Gentoo => "auto/standard",
            Target::LocalIso | Target::NetbootXyz | Target::Reset => "auto",
        }
    }
}

impl FromStr for KernelChoice {
    type Err = PostInstallError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "cloud" => Ok(Self::Cloud),
            "generic" => Ok(Self::Generic),
            "lts" => Ok(Self::Lts),
            "standard" => Ok(Self::Standard),
            "virt" => Ok(Self::Virt),
            "cachyos" | "linux-cachyos" => Ok(Self::Cachyos),
            _ => Err(PostInstallError::InvalidKernel),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PostInstallConfig {
    pub kernel: KernelChoice,
    pub hostname: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KernelPackage {
    pub package: &'static str,
    pub alpine_flavor: Option<&'static str>,
}

/// Configuration data for a target root; paths are relative to that root.
/// The zonefile must be checked in the installed target after tzdata is present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigurationPlan {
    pub kernel: KernelPackage,
    pub hostname_contents: Option<String>,
    pub timezone_contents: Option<String>,
    pub zoneinfo_relative_path: Option<String>,
    pub extra_packages: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostInstallError {
    InvalidKernel,
    UnsupportedKernel {
        target: Target,
        kernel: KernelChoice,
    },
    ConflictingKernelOptions,
    InvalidHostname,
    InvalidTimezone,
    UnexpectedRoute,
}

impl fmt::Display for PostInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKernel => f.write_str("Invalid kernel selection"),
            Self::UnsupportedKernel { target, kernel } => write!(
                f,
                "Kernel {} is unavailable for {target}; choose {}",
                kernel.as_str(),
                KernelChoice::choices_for(*target)
            ),
            Self::ConflictingKernelOptions => {
                f.write_str("Use either --kernel or --no-cloud-kernel")
            }
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
        self.kernel == KernelChoice::Auto && self.hostname.is_none() && self.timezone.is_none()
    }

    pub fn validate(
        &self,
        target: Target,
        no_cloud_kernel: bool,
        hold: Option<u8>,
    ) -> Result<(), PostInstallError> {
        if !self.is_default() && (!target.is_automatic() || hold == Some(1)) {
            return Err(PostInstallError::UnexpectedRoute);
        }
        if no_cloud_kernel && self.kernel != KernelChoice::Auto {
            return Err(PostInstallError::ConflictingKernelOptions);
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
        if target.is_automatic() {
            self.kernel_package(target, no_cloud_kernel, false)?;
        }
        Ok(())
    }

    /// Resolve distribution-standard defaults and explicit kernel packages.
    pub fn kernel_package(
        &self,
        target: Target,
        no_cloud_kernel: bool,
        _is_virtual_machine: bool,
    ) -> Result<KernelPackage, PostInstallError> {
        if no_cloud_kernel && self.kernel != KernelChoice::Auto {
            return Err(PostInstallError::ConflictingKernelOptions);
        }
        let (package, alpine_flavor) = match (target, self.kernel) {
            (
                Target::Alpine,
                KernelChoice::Auto
                | KernelChoice::Standard
                | KernelChoice::Generic
                | KernelChoice::Lts,
            ) => ("linux-lts", Some("lts")),
            (Target::Alpine, KernelChoice::Cloud | KernelChoice::Virt) => {
                ("linux-virt", Some("virt"))
            }
            (
                Target::Debian,
                KernelChoice::Auto | KernelChoice::Standard | KernelChoice::Generic,
            ) => ("linux-image-amd64", None),
            (Target::Debian, KernelChoice::Cloud) => ("linux-image-cloud-amd64", None),
            (Target::Arch, KernelChoice::Auto | KernelChoice::Standard) => ("linux", None),
            (Target::Arch, KernelChoice::Lts) => ("linux-lts", None),
            (Target::Arch, KernelChoice::Cachyos) => ("linux-cachyos", None),
            (Target::Gentoo, KernelChoice::Auto | KernelChoice::Standard) => {
                ("sys-kernel/gentoo-kernel-bin", None)
            }
            (Target::LocalIso | Target::NetbootXyz | Target::Reset, _) => {
                return Err(PostInstallError::UnexpectedRoute);
            }
            _ => {
                return Err(PostInstallError::UnsupportedKernel {
                    target,
                    kernel: self.kernel,
                });
            }
        };
        Ok(KernelPackage {
            package,
            alpine_flavor,
        })
    }

    pub fn prepare(
        &self,
        target: Target,
        no_cloud_kernel: bool,
        is_virtual_machine: bool,
    ) -> Result<ConfigurationPlan, PostInstallError> {
        self.validate(target, no_cloud_kernel, None)?;
        let kernel = self.kernel_package(target, no_cloud_kernel, is_virtual_machine)?;
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
