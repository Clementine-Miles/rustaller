//! Real target adapters, invoked only by the identity-bound Alpine Live driver.
//! Downloads and native package tools retain their signature verification.
use crate::command::{
    CommandError, CommandInput, CommandRunner, CommandSpec, OutputMode, run_checked,
};
use crate::deployment::DeploymentManifest;
use crate::fsutil::atomic_replace;
use crate::host::sources::{self, GentooStage3};
use crate::model::{BootMode, Target};
use crate::shared::configure::{ConfigurationError, validate_root};
use crate::shared::dns::{ensure_parent, validate_planned_file};
use crate::shared::storage::DiskLayout;
use crate::worker::live::ValidatedLivePlan;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use super::rolling;
pub const TTY_COMMAND_LINE: &str = "console=ttyS0,115200n8 console=tty0";
const GENTOO_RELEASE_PRIMARY: &str = "13EBBDBEDE7A12775DFDB1BABB572E0E2D182910";
const GENTOO_KEYS: &str = "https://qa-reports.gentoo.org/output/service-keys.gpg";

#[derive(Debug)]
pub enum InstallError {
    Invalid(&'static str),
    Filesystem(io::Error),
    Command(CommandError),
    Configuration(ConfigurationError),
    Debian(super::debian::DebianError),
    Rolling(rolling::RollingError),
    Cachyos(super::cachyos::CachyosError),
    Firstboot(crate::worker::firstboot::FirstbootError),
    Swap(crate::shared::swapfile::SwapfileError),
    Plan(crate::model::PlanError),
}
impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "Target installation cannot continue: {reason}"),
            Self::Filesystem(e) => write!(f, "Target installation filesystem error: {e}"),
            Self::Command(e) => e.fmt(f),
            Self::Configuration(e) => e.fmt(f),
            Self::Debian(e) => e.fmt(f),
            Self::Rolling(e) => e.fmt(f),
            Self::Cachyos(e) => e.fmt(f),
            Self::Firstboot(e) => e.fmt(f),
            Self::Swap(e) => e.fmt(f),
            Self::Plan(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for InstallError {}
impl From<io::Error> for InstallError {
    fn from(e: io::Error) -> Self {
        Self::Filesystem(e)
    }
}
impl From<CommandError> for InstallError {
    fn from(e: CommandError) -> Self {
        Self::Command(e)
    }
}
impl From<ConfigurationError> for InstallError {
    fn from(e: ConfigurationError) -> Self {
        Self::Configuration(e)
    }
}
impl From<super::debian::DebianError> for InstallError {
    fn from(e: super::debian::DebianError) -> Self {
        Self::Debian(e)
    }
}
impl From<rolling::RollingError> for InstallError {
    fn from(e: rolling::RollingError) -> Self {
        Self::Rolling(e)
    }
}
impl From<super::cachyos::CachyosError> for InstallError {
    fn from(e: super::cachyos::CachyosError) -> Self {
        Self::Cachyos(e)
    }
}
impl From<crate::worker::firstboot::FirstbootError> for InstallError {
    fn from(e: crate::worker::firstboot::FirstbootError) -> Self {
        Self::Firstboot(e)
    }
}
impl From<crate::shared::swapfile::SwapfileError> for InstallError {
    fn from(e: crate::shared::swapfile::SwapfileError) -> Self {
        Self::Swap(e)
    }
}
impl From<crate::model::PlanError> for InstallError {
    fn from(e: crate::model::PlanError) -> Self {
        Self::Plan(e)
    }
}

/// No public constructor: extraction requires successful OpenPGP verification
/// against the pinned release primary, payload SHA512 and selected size.
pub struct VerifiedStage3 {
    path: PathBuf,
}
impl VerifiedStage3 {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
pub struct PreparedAssets {
    pub(crate) stage3: Option<VerifiedStage3>,
    pub(crate) frp: Option<FrpPayload>,
    work: PathBuf,
}
pub(crate) struct FrpPayload {
    binary: Vec<u8>,
    loader: Option<Vec<u8>>,
    configuration: Vec<u8>,
    launch: FrpLaunch,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FrpLaunch {
    Static,
    Musl,
}
impl FrpLaunch {
    fn executable(self) -> &'static str {
        match self {
            Self::Static => "/opt/rustaller/frp/frpc",
            Self::Musl => "/opt/rustaller/frp/ld-musl-x86_64.so.1",
        }
    }
    fn arguments(self) -> Vec<&'static str> {
        match self {
            Self::Static => vec![],
            Self::Musl => vec!["/opt/rustaller/frp/frpc"],
        }
    }
    fn service_command(self) -> &'static str {
        match self {
            Self::Static => "/opt/rustaller/frp/frpc",
            Self::Musl => "/opt/rustaller/frp/ld-musl-x86_64.so.1 /opt/rustaller/frp/frpc",
        }
    }
}

/// Called before partitioning. Unsupported credentials, repository metadata,
/// signing tools and optional transport configuration fail before data loss.
pub fn prepare<R: CommandRunner>(
    runner: &mut R,
    plan: &ValidatedLivePlan,
    manifest: &DeploymentManifest,
    work: &Path,
) -> Result<PreparedAssets, InstallError> {
    validate_install_options(plan, manifest)?;
    validate_root(work)?;
    let keys = Path::new("/etc/apk/keys");
    if !keys.is_dir() || fs::read_dir(keys)?.next().is_none() {
        return Err(InstallError::Invalid(
            "the verified Alpine image contains no APK signing keys",
        ));
    }
    let repos = format!(
        "{}/{}/main\n{}/{}/community\n",
        sources::ALPINE_ARCHIVE,
        manifest.alpine_branch,
        sources::ALPINE_ARCHIVE,
        manifest.alpine_branch
    );
    // This is the in-memory Live system, never the original host root.
    atomic_replace(Path::new("/etc/apk/repositories"), repos.as_bytes(), 0o644)?;
    let mut packages = vec![
        "ca-certificates",
        "curl",
        "gnupg",
        "util-linux",
        "findmnt",
        "lsblk",
        "blkid",
        "mount",
        "umount",
        "util-linux-misc",
        "parted",
        match plan.plan().system.root_filesystem {
            crate::config::RootFilesystem::Xfs => "xfsprogs",
            crate::config::RootFilesystem::Ext4 => "e2fsprogs",
        },
        "coreutils",
        "dosfstools",
        "tar",
        "xz",
        "openssh",
    ];
    match plan.plan().target {
        Target::Alpine => packages.extend(["apk-tools", "alpine-base"]),
        Target::Debian => packages.extend(["debootstrap", "debian-archive-keyring"]),
        Target::Arch => packages.extend(["arch-install-scripts", "archlinux-keyring"]),
        Target::Gentoo => {}
        _ => return Err(InstallError::Invalid("automatic target required")),
    }
    if plan.plan().web_port.is_some() {
        packages.push("websocketd");
    }
    if plan.plan().frpc_config.is_some() {
        packages.push("frp");
    }
    let mut args = vec!["add", "--no-cache"];
    args.extend(packages);
    checked(runner, "apk", args, "live-authenticated-prerequisites")?;
    // Fail before partitioning if the Live kernel cannot mount the new root.
    let filesystem = plan.plan().system.root_filesystem.as_str();
    checked(
        runner,
        "modprobe",
        [filesystem],
        "live-root-filesystem-support",
    )?;
    if plan.plan().post_install.kernel == crate::postinstall::KernelChoice::Cachyos {
        super::cachyos::prepare_assets(runner, work)?;
    }
    let stage3 = match &manifest.gentoo_stage3 {
        Some(stage) if plan.plan().target == Target::Gentoo => {
            Some(verify_gentoo_stage3(runner, stage, work)?)
        }
        None if plan.plan().target == Target::Gentoo => {
            return Err(InstallError::Invalid("Gentoo release metadata is missing"));
        }
        _ => None,
    };
    let frp = if plan.plan().frpc_config.is_some() {
        Some(prepare_frpc(runner, work)?)
    } else {
        None
    };
    Ok(PreparedAssets {
        stage3,
        frp,
        work: work.to_owned(),
    })
}

pub fn validate_install_options(
    plan: &ValidatedLivePlan,
    manifest: &DeploymentManifest,
) -> Result<(), InstallError> {
    let plan = plan.plan();
    plan.validate()?;
    plan.validate_install_credentials()?;
    manifest
        .validate()
        .map_err(|_| InstallError::Invalid("invalid deployment manifest"))?;
    if !plan.target.is_automatic() {
        return Err(InstallError::Invalid("automatic target required"));
    }
    let username = plan.username.as_deref().unwrap_or("root");
    if !valid_username(username) {
        return Err(InstallError::Invalid("invalid target account name"));
    }
    if plan.password.is_none() && plan.ssh_keys.is_empty() {
        return Err(InstallError::Invalid(
            "set a password or resolved public keys before installation",
        ));
    }
    if plan.ssh_keys.iter().any(|key| !literal_public_key(key)) {
        return Err(InstallError::Invalid(
            "public-key sources must be resolved on the original host before handoff",
        ));
    }
    if plan.target == Target::Debian {
        super::debian::validate_codename(&manifest.debian_codename)?;
    }
    if plan.system.network.ipv6 == crate::config::Ipv6Policy::Disable
        && !manifest
            .network
            .iter()
            .any(|nic| (!nic.ipv4_addresses.is_empty() || nic.dhcp4) && nic.ipv4_gateway.is_some())
    {
        return Err(InstallError::Invalid(
            "disabling IPv6 would leave no inherited IPv4 default route",
        ));
    }
    if let Some(path) = &plan.frpc_config {
        if path != "/configs/frpc.toml" {
            return Err(InstallError::Invalid(
                "frpc configuration must be privately staged before handoff",
            ));
        }
        let meta = fs::symlink_metadata(path)?;
        if !meta.is_file()
            || meta.uid() != 0
            || meta.permissions().mode() & 0o077 != 0
            || meta.len() > 1024 * 1024
        {
            return Err(InstallError::Invalid(
                "frpc configuration must be a bounded private root-owned regular file",
            ));
        }
    }
    Ok(())
}

pub fn bootstrap<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
    manifest: &DeploymentManifest,
    assets: &PreparedAssets,
    layout: &DiskLayout,
) -> Result<(), InstallError> {
    validate_root(root)?;
    match plan.plan().target {
        Target::Alpine => {
            for name in ["etc/apk/keys", "dev", "proc", "sys", "run"] {
                fs::create_dir_all(root.join(name))?;
            }
            copy_apk_keys(root)?;
            let repositories = format!(
                "{}/{}/main\n{}/{}/community\n",
                sources::ALPINE_ARCHIVE,
                manifest.alpine_branch,
                sources::ALPINE_ARCHIVE,
                manifest.alpine_branch
            );
            write_target(root, "etc/apk/repositories", repositories.as_bytes(), 0o644)?;
            // Bootstrap only userland first. Kernel hooks run after pseudo mounts.
            let mut args = vec![
                OsString::from("--root"),
                root.as_os_str().to_owned(),
                "--initdb".into(),
            ];
            if !plan.plan().system.keep_package_cache {
                args.push("--no-cache".into());
            }
            args.push("add".into());
            args.extend(
                [
                    "alpine-base",
                    "openssh",
                    "ca-certificates",
                    "dhcpcd",
                    match plan.plan().system.root_filesystem {
                        crate::config::RootFilesystem::Xfs => "xfsprogs",
                        crate::config::RootFilesystem::Ext4 => "e2fsprogs",
                    },
                ]
                .map(Into::into),
            );
            checked(runner, "apk", args, "alpine-bootstrap-rootfs")?;
        }
        Target::Debian => {
            let keyring = "/usr/share/keyrings/debian-archive-keyring.gpg";
            if !Path::new(keyring).is_file() {
                return Err(InstallError::Invalid(
                    "authenticated Debian archive keyring is missing",
                ));
            }
            let args = vec![
                OsString::from("--arch=amd64"),
                "--variant=minbase".into(),
                "--include=ca-certificates,debconf-utils".into(),
                "--force-check-gpg".into(),
                format!("--keyring={keyring}").into(),
                manifest.debian_codename.clone().into(),
                root.as_os_str().to_owned(),
                sources::DEBIAN_ARCHIVE.into(),
            ];
            checked(runner, "debootstrap", args, "debian-verified-minbase")?;
        }
        Target::Arch => rolling::bootstrap_arch_rootfs(
            runner,
            root,
            plan,
            layout.boot_mode(),
            &[],
            &manifest.alpine_branch,
            &assets.work.join("pacman.conf"),
        )?,
        Target::Gentoo => rolling::extract_gentoo(
            runner,
            root,
            assets
                .stage3
                .as_ref()
                .ok_or(InstallError::Invalid("verified Gentoo stage3 is missing"))?,
        )?,
        _ => return Err(InstallError::Invalid("automatic target required")),
    }
    copy_live_resolver(root)?;
    copy_live_transport(root, plan.plan().target)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn configure<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
    manifest: &DeploymentManifest,
    assets: &PreparedAssets,
    layout: &DiskLayout,
    root_uuid: &str,
    efi_uuid: Option<&str>,
) -> Result<(), InstallError> {
    validate_root(root)?;
    let mut transport = TargetTransportRunner {
        inner: runner,
        root,
    };
    let runner = &mut transport;
    // Native kernel/initramfs hooks must see the target root and ESP UUIDs,
    // not infer the installed root from the Alpine Live environment.
    write_fstab(root, layout, root_uuid, efi_uuid)?;
    crate::targets::packages::apply_package_policy(root, plan)?;
    prevent_package_service_start(root, plan.plan().target)?;
    let result = (|| {
        match plan.plan().target {
            Target::Alpine => {
                let packages =
                    super::packages::package_plan(plan.plan(), layout.boot_mode(), true, &[])?;
                let mut args = vec![OsString::from("apk"), "add".into()];
                if !plan.plan().system.keep_package_cache {
                    args.push("--no-cache".into());
                }
                args.extend(packages.later_packages.into_iter().map(OsString::from));
                chroot_checked(runner, root, args, "alpine-target-packages-and-kernel")?;
                // Keep native mkinitfs features and their dependency resolution.
                // scsi includes virtio_scsi, VMware PVSCSI and Hyper-V storvsc.
                write_target(
                    root,
                    "etc/mkinitfs/mkinitfs.conf",
                    format!(
                        "features=\"ata base scsi virtio {} nvme xenpci rustaller\"\n",
                        layout.root_filesystem().as_str()
                    )
                    .as_bytes(),
                    0o644,
                )?;
                write_target(
                    root,
                    "etc/mkinitfs/features.d/rustaller.modules",
                    b"kernel/drivers/block/xen-blkfront.ko*\nkernel/drivers/hv/hv_vmbus.ko*\nkernel/drivers/pci/controller/pci-hyperv.ko*\n",
                    0o644,
                )?;
                for version in installed_module_versions(root)? {
                    chroot_checked(
                        runner,
                        root,
                        ["mkinitfs", &version],
                        "alpine-target-initramfs",
                    )?;
                }
            }
            Target::Debian => {
                super::debian::write_apt_sources(root, &manifest.debian_codename, None)?;
                super::debian::preseed_grub(
                    runner,
                    root,
                    layout.boot_mode(),
                    &layout.device()[5..],
                )?;
                let packages =
                    super::debian::apt_package_list_for_plan(plan.plan(), layout.boot_mode(), &[])?;
                super::debian::install_apt_packages(runner, root, &packages)?;
                write_target(
                    root,
                    "etc/initramfs-tools/conf.d/rustaller",
                    b"MODULES=most\n",
                    0o644,
                )?;
                chroot_checked(
                    runner,
                    root,
                    ["update-initramfs", "-u", "-k", "all"],
                    "debian-target-initramfs",
                )?;
            }
            Target::Arch => {
                if plan.plan().post_install.kernel == crate::postinstall::KernelChoice::Cachyos {
                    super::cachyos::configure_repositories(runner, root, &assets.work)?;
                }
                rolling::configure_arch(runner, root, plan)?;
            }
            Target::Gentoo => {
                rolling::configure_gentoo(runner, root, plan, layout.boot_mode(), &[], root_uuid)?
            }
            _ => return Err(InstallError::Invalid("automatic target required")),
        }
        write_network(root, manifest, plan.plan().target)?;
        configure_accounts(runner, root, plan)?;
        let hostname = target_hostname(plan.plan());
        if plan.plan().post_install.hostname.is_none() {
            write_target(
                root,
                "etc/hostname",
                format!("{hostname}\n").as_bytes(),
                0o644,
            )?;
        }
        crate::shared::configure::apply_basic_configuration(root, plan, true)?;
        write_target_hosts(root, &hostname)?;
        let needs_kernel_config = plan.plan().system.network.congestion
            != crate::config::CongestionControl::Default
            || plan.plan().system.network.qdisc != crate::config::QueueDiscipline::Default;
        let kernel_config = if needs_kernel_config {
            installed_kernel_config(runner, root, plan)?
        } else {
            String::new()
        };
        crate::shared::network::apply_network_configuration(root, plan, &kernel_config)?;
        if needs_kernel_config {
            verify_requested_modules(root, &kernel_config, plan)?;
        }
        crate::shared::swapfile::prepare_swapfile(runner, root, plan, |done, total| {
            if done == total || done % 128 == 0 {
                println!("Target swapfile: {done}/{total} MiB");
            }
        })?;
        enable_services(runner, root, plan.plan().target)?;
        if let Some(frp) = &assets.frp {
            install_frpc(root, plan.plan().target, frp)?;
        }
        match plan.plan().target {
            Target::Alpine => install_alpine_grub(runner, root, layout)?,
            Target::Debian => super::debian::install_grub(
                runner,
                root,
                layout.boot_mode(),
                &layout.device()[5..],
                TTY_COMMAND_LINE,
            )?,
            Target::Arch | Target::Gentoo => rolling::install_grub(
                runner,
                root,
                layout.boot_mode(),
                &layout.device(),
                TTY_COMMAND_LINE,
            )?,
            _ => unreachable!(),
        }
        let worker = fs::read("/rustaller-worker")?;
        crate::worker::firstboot::install_firstboot_service(
            runner,
            root,
            if plan.plan().target == Target::Alpine {
                crate::worker::firstboot::InitManager::OpenRc
            } else {
                crate::worker::firstboot::InitManager::Systemd
            },
            &worker,
        )?;
        cleanup_cache(runner, root, plan)?;
        finalize_resolver(root, plan)?;
        verify_installed_target(runner, root, plan, layout)?;
        remove_live_transport(root)?;
        Ok(())
    })();
    restore_package_service_start(root, plan.plan().target)?;
    result
}

fn finalize_resolver(root: &Path, plan: &ValidatedLivePlan) -> Result<(), InstallError> {
    validate_root(root)?;
    if plan.plan().hold == Some(1) {
        return Err(InstallError::Invalid(
            "rescue must not configure a target resolver",
        ));
    }
    if plan.plan().target == Target::Alpine
        || plan.plan().system.dns.mode != crate::config::DnsMode::Auto
    {
        return Ok(());
    }
    let path = root.join("etc/resolv.conf");
    crate::shared::dns::validate_planned_parent(&path)?;
    crate::shared::configure::validate_destination(&path, true)?;
    // Bootstrap/package operations used a regular copy of Live DNS. Only after
    // those finish, let resolved track DHCP renewals on the installed system.
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    symlink("/run/systemd/resolve/stub-resolv.conf", path)?;
    Ok(())
}

fn prevent_package_service_start(root: &Path, target: Target) -> Result<(), InstallError> {
    if target == Target::Debian {
        let path = root.join("usr/sbin/policy-rc.d");
        validate_planned_file(&path)?;
        if path.exists() {
            return Err(InstallError::Invalid(
                "unexpected existing policy-rc.d in fresh target",
            ));
        }
        // Native Debian policy configuration; it delegates no orchestration.
        write_target(
            root,
            "usr/sbin/policy-rc.d",
            b"#!/bin/sh\nexit 101\n",
            0o755,
        )?;
    }
    Ok(())
}
fn restore_package_service_start(root: &Path, target: Target) -> Result<(), InstallError> {
    if target == Target::Debian {
        fs::remove_file(root.join("usr/sbin/policy-rc.d"))?;
    }
    Ok(())
}

pub fn write_network(
    root: &Path,
    manifest: &DeploymentManifest,
    target: Target,
) -> Result<(), InstallError> {
    validate_root(root)?;
    manifest
        .validate()
        .map_err(|_| InstallError::Invalid("invalid network manifest"))?;
    if target == Target::Alpine {
        let mut interfaces = "auto lo\niface lo inet loopback\n\n".to_owned();
        for nic in &manifest.network {
            interfaces.push_str(&format!("# mac {}\nauto {}\n", nic.mac, nic.name));
            if nic.dhcp4 {
                interfaces.push_str(&format!("iface {} inet dhcp\n", nic.name));
            } else if let Some(first) = nic.ipv4_addresses.first() {
                interfaces.push_str(&format!(
                    "iface {} inet static\n    address {first}\n",
                    nic.name
                ));
                for address in nic.ipv4_addresses.iter().skip(1) {
                    interfaces.push_str(&format!(
                        "    post-up ip -4 addr replace {address} dev {}\n",
                        nic.name
                    ));
                }
                if let Some(gateway) = &nic.ipv4_gateway {
                    interfaces.push_str(&format!(
                        "    post-up ip -4 route replace default via {gateway} onlink dev {}\n",
                        nic.name
                    ));
                }
            }
            if nic.dhcp6 {
                interfaces.push_str(&format!("iface {} inet6 dhcp\n", nic.name));
            } else if nic.accept_ra {
                interfaces.push_str(&format!("iface {} inet6 auto\n", nic.name));
            } else if let Some(first) = nic.ipv6_addresses.first() {
                interfaces.push_str(&format!(
                    "iface {} inet6 static\n    address {first}\n",
                    nic.name
                ));
                for address in nic.ipv6_addresses.iter().skip(1) {
                    interfaces.push_str(&format!(
                        "    post-up ip -6 addr replace {address} dev {}\n",
                        nic.name
                    ));
                }
                if let Some(gateway) = &nic.ipv6_gateway {
                    interfaces.push_str(&format!(
                        "    post-up ip -6 route replace default via {gateway} onlink dev {}\n",
                        nic.name
                    ));
                }
            }
            interfaces.push('\n');
        }
        write_target(root, "etc/network/interfaces", interfaces.as_bytes(), 0o644)?;
        // No interface-name binding: the firstboot MAC repair renames ifupdown
        // stanzas; dhcpcd remains valid if ens3 becomes eth0 after rustaller.
        write_target(
            root,
            "etc/dhcpcd.conf",
            b"# rustaller: deterministic SLAAC, native ifupdown selects interfaces\nslaac hwaddr\n",
            0o644,
        )?;
    } else {
        for (index, nic) in manifest.network.iter().enumerate() {
            let dhcp = match (nic.dhcp4, nic.dhcp6) {
                (true, true) => "yes",
                (true, false) => "ipv4",
                (false, true) => "ipv6",
                (false, false) => "no",
            };
            let mut text = format!(
                "[Match]\nMACAddress={}\n\n[Network]\nDHCP={dhcp}\nIPv6AcceptRA={}\n",
                nic.mac,
                if nic.accept_ra { "yes" } else { "no" }
            );
            if !nic.dhcp4 {
                for address in &nic.ipv4_addresses {
                    text.push_str(&format!("Address={address}\n"));
                }
            }
            if !nic.dhcp6 && !nic.accept_ra {
                for address in &nic.ipv6_addresses {
                    text.push_str(&format!("Address={address}\n"));
                }
            }
            for gateway in nic.ipv4_gateway.iter().chain(&nic.ipv6_gateway) {
                text.push_str(&format!(
                    "\n[Route]\nGateway={gateway}\nGatewayOnLink=yes\n"
                ));
            }
            write_target(
                root,
                &format!("etc/systemd/network/{:02}-{}.network", 10 + index, nic.name),
                text.as_bytes(),
                0o644,
            )?;
        }
    }
    Ok(())
}

fn configure_accounts<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
) -> Result<(), InstallError> {
    let plan = plan.plan();
    let username = plan.username.as_deref().unwrap_or("root");
    let root_password = if username != "root" {
        Some(plan.root_password.as_ref().ok_or(InstallError::Invalid(
            "a separate root password is required for a non-root installation account",
        ))?)
    } else {
        None
    };
    if username != "root" {
        match plan.target {
            Target::Alpine => chroot_checked(
                runner,
                root,
                ["adduser", "-D", "-s", "/bin/ash", username],
                "target-create-account",
            )?,
            _ => chroot_checked(
                runner,
                root,
                ["useradd", "--create-home", "--shell", "/bin/bash", username],
                "target-create-account",
            )?,
        }
        write_target(
            root,
            "etc/sudoers.d/90-rustaller",
            format!(
                "{username} ALL=(ALL:ALL) {}ALL\n",
                if plan.password.is_none() {
                    "NOPASSWD: "
                } else {
                    ""
                }
            )
            .as_bytes(),
            0o440,
        )?;
        let root_password =
            root_password.expect("non-root credentials checked before account creation");
        let spec = chroot_command(root, ["chpasswd"], "target-set-root-password")
            .stdin(CommandInput::Bytes(
                format!("root:{}\n", root_password.expose()).into_bytes(),
            ))
            .stdout_mode(OutputMode::Discard)
            .stderr_mode(OutputMode::Capture);
        run_checked(runner, &spec)?;
    }
    if let Some(password) = &plan.password {
        let spec = chroot_command(root, ["chpasswd"], "target-set-account-password")
            .stdin(CommandInput::Bytes(
                format!("{username}:{}\n", password.expose()).into_bytes(),
            ))
            .stdout_mode(OutputMode::Discard)
            .stderr_mode(OutputMode::Capture);
        run_checked(runner, &spec)?;
    } else {
        let home = if username == "root" {
            "root".into()
        } else {
            format!("home/{username}")
        };
        let path = root.join(&home).join(".ssh/authorized_keys");
        validate_planned_file(&path)?;
        ensure_parent(&path)?;
        fs::set_permissions(
            path.parent().expect("SSH parent"),
            fs::Permissions::from_mode(0o700),
        )?;
        atomic_replace(
            &path,
            format!("{}\n", plan.ssh_keys.join("\n")).as_bytes(),
            0o600,
        )?;
        chroot_checked(
            runner,
            root,
            ["passwd", "-d", username],
            "target-unlock-public-key-account",
        )?;
        let owner = format!("{username}:{username}");
        let inside = format!("/{home}/.ssh");
        chroot_checked(
            runner,
            root,
            ["chown", "-R", &owner, &inside],
            "target-ssh-key-owner",
        )?;
    }
    let config = format!(
        "# rustaller target SSH\nPort {}\nPermitRootLogin {}\nPubkeyAuthentication yes\nPasswordAuthentication {}\nKbdInteractiveAuthentication no\nPermitEmptyPasswords no\n",
        plan.ssh_port.unwrap_or(22),
        if username == "root" {
            if plan.password.is_some() {
                "yes"
            } else {
                "prohibit-password"
            }
        } else {
            "no"
        },
        if plan.password.is_some() { "yes" } else { "no" }
    );
    let sshd = root.join("etc/ssh/sshd_config");
    validate_planned_file(&sshd)?;
    let original = crate::shared::dns::read_config(&sshd)?;
    // OpenSSH uses first-obtained values; prepend managed settings before Include/Match.
    let text = crate::shared::dns::replace_managed_block(
        &original,
        &config,
        "# BEGIN rustaller SSH",
        "# END rustaller SSH",
        true,
    )?;
    write_target(root, "etc/ssh/sshd_config", text.as_bytes(), 0o600)?;
    chroot_checked(
        runner,
        root,
        ["ssh-keygen", "-A"],
        "target-generate-server-host-keys",
    )?;
    fs::create_dir_all(root.join("run/sshd"))?;
    chroot_checked(
        runner,
        root,
        ["/usr/sbin/sshd", "-t"],
        "target-verify-ssh-configuration",
    )?;
    Ok(())
}

fn enable_services<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    target: Target,
) -> Result<(), InstallError> {
    if target == Target::Alpine {
        for (name, level) in [
            ("devfs", "sysinit"),
            ("dmesg", "sysinit"),
            ("mdev", "sysinit"),
            ("hwdrivers", "sysinit"),
            ("modules", "boot"),
            ("sysctl", "boot"),
            ("hostname", "boot"),
            ("bootmisc", "boot"),
            ("syslog", "boot"),
            ("networking", "boot"),
            ("sshd", "default"),
            ("local", "default"),
        ] {
            chroot_checked(
                runner,
                root,
                ["rc-update", "add", name, level],
                "alpine-enable-native-services",
            )?;
        }
    } else {
        chroot_checked(
            runner,
            root,
            [
                "systemctl",
                "enable",
                "systemd-networkd.service",
                "systemd-resolved.service",
            ],
            "target-enable-networkd",
        )?;
        chroot_checked(
            runner,
            root,
            [
                "systemctl",
                "enable",
                if target == Target::Debian {
                    "ssh.service"
                } else {
                    "sshd.service"
                },
            ],
            "target-enable-ssh",
        )?;
    }
    Ok(())
}

fn write_fstab(
    root: &Path,
    layout: &DiskLayout,
    root_uuid: &str,
    efi_uuid: Option<&str>,
) -> Result<(), InstallError> {
    if !valid_uuid(root_uuid) || efi_uuid.is_some_and(|value| !valid_uuid(value)) {
        return Err(InstallError::Invalid("invalid target filesystem UUID"));
    }
    if (layout.boot_mode() == BootMode::Efi) != efi_uuid.is_some() {
        return Err(InstallError::Invalid("EFI layout/UUID mismatch"));
    }
    let filesystem = layout.root_filesystem();
    let pass = if filesystem == crate::config::RootFilesystem::Ext4 {
        1
    } else {
        0
    };
    let mut text = format!(
        "UUID={root_uuid} / {} defaults 0 {pass}\n",
        filesystem.as_str()
    );
    if let Some(uuid) = efi_uuid {
        text.push_str(&format!(
            "UUID={uuid} {} vfat umask=0077 0 2\n",
            if matches!(layout.target(), Target::Alpine | Target::Debian) {
                "/boot/efi"
            } else {
                "/efi"
            }
        ));
    }
    write_target(root, "etc/fstab", text.as_bytes(), 0o644)
}

fn install_alpine_grub<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    layout: &DiskLayout,
) -> Result<(), InstallError> {
    write_target(
        root,
        "etc/default/grub",
        format!("GRUB_TIMEOUT=2\nGRUB_CMDLINE_LINUX=\"{TTY_COMMAND_LINE}\"\n").as_bytes(),
        0o644,
    )?;
    match layout.boot_mode() {
        BootMode::Bios => chroot_checked(
            runner,
            root,
            vec![
                "grub-install".into(),
                "--target=i386-pc".into(),
                "--recheck".into(),
                layout.device(),
            ],
            "alpine-install-grub-bios",
        )?,
        BootMode::Efi => chroot_checked(
            runner,
            root,
            [
                "grub-install",
                "--target=x86_64-efi",
                "--efi-directory=/boot/efi",
                "--bootloader-id=alpine",
                "--no-nvram",
                "--removable",
            ],
            "alpine-install-grub-efi",
        )?,
    }
    chroot_checked(
        runner,
        root,
        ["grub-mkconfig", "-o", "/boot/grub/grub.cfg"],
        "alpine-generate-grub",
    )
}

fn installed_kernel_config<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
) -> Result<String, InstallError> {
    let boot = root.join("boot");
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&boot)? {
        let path = entry?.path();
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        if name.starts_with("config-") && fs::symlink_metadata(&path)?.is_file() {
            candidates.push(path);
        }
    }
    if candidates.is_empty() {
        for base in ["usr/lib/modules", "lib/modules"] {
            let base = root.join(base);
            let Ok(entries) = fs::read_dir(&base) else {
                continue;
            };
            for entry in entries {
                let path = entry?.path().join("config");
                if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                    candidates.push(path);
                }
            }
        }
    }
    candidates.sort();
    if candidates.is_empty() && plan.plan().target == Target::Arch {
        return arch_module_capability_config(root, &plan.plan().system.network);
    }
    let path = candidates.last().ok_or(InstallError::Invalid(
        "installed kernel configuration is missing",
    ))?;
    let data = fs::read(path)?;
    if data.len() > 4 * 1024 * 1024 {
        return Err(InstallError::Invalid(
            "installed kernel configuration is oversized",
        ));
    }
    if data.starts_with(&[0x1f, 0x8b]) {
        capture(
            runner,
            &command(
                "gzip",
                [OsStr::new("-dc"), path.as_os_str()],
                "read-installed-kernel-config",
            ),
        )
    } else {
        String::from_utf8(data)
            .map_err(|_| InstallError::Invalid("invalid installed kernel configuration"))
    }
}

/// Arch ships its authenticated capability inventory with the installed kernel
/// even when the full build .config is absent. Admit only one release tree and
/// exact built-in or regular module files for the explicitly requested knobs.
fn arch_module_capability_config(
    root: &Path,
    network: &crate::config::NetworkConfig,
) -> Result<String, InstallError> {
    use crate::config::{CongestionControl, QueueDiscipline};
    validate_root(root)?;
    let canonical = fs::canonicalize(root)?;
    let device = fs::metadata(&canonical)?.dev();
    let mut bases = std::collections::BTreeSet::new();
    for relative in ["usr/lib/modules", "lib/modules"] {
        let path = match fs::canonicalize(root.join(relative)) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !path.starts_with(&canonical) {
            return Err(InstallError::Invalid(
                "installed module path escapes the target",
            ));
        }
        bases.insert(path);
    }
    let mut releases = Vec::new();
    for base in bases {
        for entry in fs::read_dir(base)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| InstallError::Invalid("invalid installed kernel release"))?;
            if entry.file_type()?.is_dir()
                && name.as_bytes().first().is_some_and(u8::is_ascii_digit)
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
            {
                releases.push(entry.path());
            }
        }
    }
    if releases.len() != 1 {
        return Err(InstallError::Invalid(
            "network capability fallback requires exactly one installed kernel release",
        ));
    }
    let release = &releases[0];
    let builtin_path = release.join("modules.builtin");
    let mut builtins = std::collections::BTreeSet::new();
    match fs::symlink_metadata(&builtin_path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 || metadata.dev() != device {
                return Err(InstallError::Invalid(
                    "invalid installed built-in module inventory",
                ));
            }
            for line in fs::read_to_string(&builtin_path)?
                .lines()
                .filter(|line| !line.is_empty())
            {
                if line.starts_with('/')
                    || line.split('/').any(|part| part == "..")
                    || line.chars().any(char::is_control)
                {
                    return Err(InstallError::Invalid(
                        "unsafe installed built-in module path",
                    ));
                }
                if let Some(name) = Path::new(line)
                    .file_name()
                    .and_then(OsStr::to_str)
                    .and_then(module_name)
                {
                    builtins.insert(name.to_owned());
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut modules = std::collections::BTreeMap::<String, usize>::new();
    let mut directories = vec![release.clone()];
    let mut visited = 0;
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            visited += 1;
            if visited > 100000 {
                return Err(InstallError::Invalid(
                    "installed module inventory exceeds limit",
                ));
            }
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.dev() != device {
                return Err(InstallError::Invalid(
                    "installed module inventory crosses filesystem boundary",
                ));
            }
            if metadata.is_dir() {
                directories.push(entry.path());
            } else if metadata.is_file()
                && let Some(name) = entry.file_name().to_str().and_then(module_name)
            {
                *modules.entry(name.to_owned()).or_default() += 1;
            }
        }
    }
    let mut needed = Vec::new();
    match network.congestion {
        CongestionControl::Default => {}
        CongestionControl::Bbr => needed.push(("CONFIG_TCP_CONG_BBR", "tcp_bbr")),
        CongestionControl::Cubic => needed.push(("CONFIG_TCP_CONG_CUBIC", "tcp_cubic")),
    }
    match network.qdisc {
        QueueDiscipline::Default => {}
        QueueDiscipline::Fq => needed.push(("CONFIG_NET_SCH_FQ", "sch_fq")),
        QueueDiscipline::FqCodel => needed.push(("CONFIG_NET_SCH_FQ_CODEL", "sch_fq_codel")),
    }
    let mut text = String::from(
        "# rustaller: capabilities from the authenticated installed kernel module inventory\n",
    );
    for (symbol, module) in needed {
        let builtin = builtins.contains(module);
        let count = modules.get(module).copied().unwrap_or(0);
        let state = match (builtin, count) {
            (true, 0) => "y",
            (false, 1) => "m",
            _ => {
                return Err(InstallError::Invalid(
                    "requested network module capability is missing or ambiguous",
                ));
            }
        };
        text.push_str(&format!("{symbol}={state}\n"));
    }
    Ok(text)
}
fn module_name(name: &str) -> Option<&str> {
    let name = name
        .strip_suffix(".xz")
        .or_else(|| name.strip_suffix(".zst"))
        .or_else(|| name.strip_suffix(".gz"))
        .unwrap_or(name);
    name.strip_suffix(".ko")
}
fn verify_requested_modules(
    root: &Path,
    config: &str,
    plan: &ValidatedLivePlan,
) -> Result<(), InstallError> {
    let modules = crate::shared::network::required_modules(&plan.plan().system.network, config)?;
    if modules.is_empty() {
        return Ok(());
    }
    let mut remaining = modules;
    for directory in ["usr/lib/modules", "lib/modules"] {
        let base = root.join(directory);
        let Ok(base) = fs::canonicalize(&base) else {
            continue;
        };
        if !base.starts_with(root) {
            return Err(InstallError::Invalid("target modules escape target root"));
        }
        let mut directories = vec![base];
        let mut count = 0;
        while let Some(path) = directories.pop() {
            for entry in fs::read_dir(path)? {
                count += 1;
                if count > 100000 {
                    return Err(InstallError::Invalid(
                        "target module inventory exceeds limit",
                    ));
                }
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    directories.push(entry.path());
                } else if kind.is_file() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    remaining.retain(|module| !name.starts_with(&format!("{module}.ko")));
                }
            }
        }
    }
    if !remaining.is_empty() {
        return Err(InstallError::Invalid(
            "requested target network kernel modules are missing",
        ));
    }
    Ok(())
}

fn cleanup_cache<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
) -> Result<(), InstallError> {
    if plan.plan().system.clear_package_cache() {
        match plan.plan().target {
            Target::Debian => chroot_checked(
                runner,
                root,
                ["apt-get", "clean"],
                "target-clean-package-cache",
            )?,
            Target::Arch => chroot_checked(
                runner,
                root,
                ["pacman", "-Scc", "--noconfirm"],
                "target-clean-package-cache",
            )?,
            Target::Alpine => {}
            Target::Gentoo => {}
            _ => unreachable!(),
        }
        let cleanup = crate::targets::packages::cleanup_plan(plan)?;
        for relative in cleanup.package_cache_roots {
            clean_target_tree(root, relative, false)?;
        }
    }
    if plan.plan().system.strip_docs() {
        for relative in [
            "usr/share/doc",
            "usr/share/man",
            "usr/share/info",
            "usr/share/locale",
        ] {
            clean_target_tree(root, relative, true)?;
        }
    }
    Ok(())
}
fn verify_installed_target<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
    layout: &DiskLayout,
) -> Result<(), InstallError> {
    for path in ["etc/os-release", "etc/fstab", "boot/grub/grub.cfg"] {
        let path = root.join(path);
        if !path.is_file() || fs::metadata(path)?.len() == 0 {
            return Err(InstallError::Invalid(
                "installed OS, fstab or GRUB configuration is missing",
            ));
        }
    }
    let mut kernel = false;
    let mut initramfs = false;
    for entry in fs::read_dir(root.join("boot"))? {
        let entry = entry?;
        if !entry.file_type()?.is_file() || entry.metadata()?.len() == 0 {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        kernel |= name.starts_with("vmlinuz") || name.starts_with("kernel-");
        initramfs |= name.starts_with("initramfs") || name.starts_with("initrd");
    }
    if !kernel || !initramfs {
        return Err(InstallError::Invalid(
            "installed kernel or initramfs is missing",
        ));
    }
    verify_grub_root(runner, root, layout.root_filesystem())?;
    if layout.boot_mode() == BootMode::Efi {
        let path = root.join(
            if matches!(plan.plan().target, Target::Alpine | Target::Debian) {
                "boot/efi/EFI/BOOT/BOOTX64.EFI"
            } else {
                "efi/EFI/BOOT/BOOTX64.EFI"
            },
        );
        verify_efi_loader(&path)?;
    }
    chroot_checked(
        runner,
        root,
        ["/usr/sbin/sshd", "-t"],
        "target-final-ssh-check",
    )?;
    Ok(())
}

fn verify_grub_root<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    root_filesystem: crate::config::RootFilesystem,
) -> Result<(), InstallError> {
    let filesystem = capture(
        runner,
        &command(
            "chroot",
            [
                root.as_os_str(),
                "grub-probe".as_ref(),
                "--target=fs".as_ref(),
                "/".as_ref(),
            ],
            "target-grub-root-probe",
        ),
    )?;
    // GRUB's ext2 filesystem module reads ext2, ext3 and ext4 and reports ext2.
    let expected = match root_filesystem {
        crate::config::RootFilesystem::Xfs => "xfs",
        crate::config::RootFilesystem::Ext4 => "ext2",
    };
    if filesystem.trim() != expected {
        return Err(InstallError::Invalid(
            "GRUB cannot identify the selected target root filesystem",
        ));
    }
    chroot_checked(
        runner,
        root,
        ["grub-script-check", "/boot/grub/grub.cfg"],
        "target-grub-config-check",
    )
}

// An existing filename is not sufficient: reject empty/truncated, IA32 and
// non-application PE files before requesting the sole final reboot.
fn verify_efi_loader(path: &Path) -> Result<(), InstallError> {
    let invalid =
        || InstallError::Invalid("EFI fallback loader is not a valid x86_64 PE32+ application");
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid());
    }
    let mut header = Vec::new();
    File::open(path)?.take(64 * 1024).read_to_end(&mut header)?;
    if header.len() < 64 || &header[..2] != b"MZ" {
        return Err(invalid());
    }
    let offset = u32::from_le_bytes(header[60..64].try_into().unwrap()) as usize;
    let pe = header.get(offset..).ok_or_else(invalid)?;
    if pe.len() < 94
        || &pe[..6] != b"PE\0\0\x64\x86"
        || u16::from_le_bytes([pe[20], pe[21]]) < 112
        || &pe[24..26] != b"\x0b\x02"
        || &pe[92..94] != b"\x0a\0"
        || pe.len() < 24 + usize::from(u16::from_le_bytes([pe[20], pe[21]]))
    {
        return Err(invalid());
    }
    Ok(())
}

fn prepare_frpc<R: CommandRunner>(runner: &mut R, work: &Path) -> Result<FrpPayload, InstallError> {
    let configuration = fs::read("/configs/frpc.toml")?;
    run_checked(
        runner,
        &command(
            "frpc",
            ["verify", "-c", "/configs/frpc.toml"],
            "verify-private-frpc-configuration",
        )
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Capture),
    )?;
    let binary = fs::read("/usr/bin/frpc")?;
    let launch = frpc_launch(&binary)?;
    let loader = if launch == FrpLaunch::Musl {
        Some(fs::read("/lib/ld-musl-x86_64.so.1")?)
    } else {
        None
    };
    if loader
        .as_ref()
        .is_some_and(|bytes| bytes.is_empty() || bytes.len() > 4 * 1024 * 1024)
    {
        return Err(InstallError::Invalid(
            "authenticated native frpc/loader payload is invalid",
        ));
    }
    let payload = FrpPayload {
        binary,
        loader,
        configuration,
        launch,
    };
    probe_frpc_portability(runner, work, &payload)?;
    Ok(payload)
}
fn install_frpc(root: &Path, target: Target, frp: &FrpPayload) -> Result<(), InstallError> {
    write_target(root, "opt/rustaller/frp/frpc", &frp.binary, 0o755)?;
    if let Some(loader) = &frp.loader {
        write_target(root, "opt/rustaller/frp/ld-musl-x86_64.so.1", loader, 0o755)?;
    }
    write_target(root, "etc/frp/frpc.toml", &frp.configuration, 0o600)?;
    if target == Target::Alpine {
        let mut arguments = frp.launch.arguments();
        arguments.extend(["-c", "/etc/frp/frpc.toml"]);
        let service = format!(
            "#!/sbin/openrc-run\nname=rustaller-frpc\ncommand={}\ncommand_args=\"{}\"\ncommand_background=yes\npidfile=/run/rustaller-frpc.pid\ndepend() {{ need net; }}\n",
            frp.launch.executable(),
            arguments.join(" ")
        );
        write_target(root, "etc/init.d/rustaller-frpc", service.as_bytes(), 0o755)?;
        let link = root.join("etc/runlevels/default/rustaller-frpc");
        ensure_parent(&link)?;
        symlink("/etc/init.d/rustaller-frpc", link)?;
    } else {
        let service = format!(
            "[Unit]\nDescription=Rustaller frpc tunnel\nWants=network-online.target\nAfter=network-online.target\n[Service]\nExecStart={} -c /etc/frp/frpc.toml\nRestart=on-failure\nRestartSec=5\n[Install]\nWantedBy=multi-user.target\n",
            frp.launch.service_command()
        );
        write_target(
            root,
            "etc/systemd/system/rustaller-frpc.service",
            service.as_bytes(),
            0o644,
        )?;
        let link = root.join("etc/systemd/system/multi-user.target.wants/rustaller-frpc.service");
        ensure_parent(&link)?;
        symlink("/etc/systemd/system/rustaller-frpc.service", link)?;
    }
    Ok(())
}

/// APK authenticates the file; this parser admits only supported ELF launch
/// modes. A minimal-chroot native probe below proves loader independence.
fn frpc_launch(bytes: &[u8]) -> Result<FrpLaunch, InstallError> {
    let bad =
        || InstallError::Invalid("authenticated frpc is not a supported x86_64 ELF executable");
    if bytes.len() < 64
        || bytes.len() > 64 * 1024 * 1024
        || bytes.get(..7) != Some(b"\x7fELF\x02\x01\x01")
        || !matches!(u16::from_le_bytes([bytes[16], bytes[17]]), 2 | 3)
        || u16::from_le_bytes([bytes[18], bytes[19]]) != 62
    {
        return Err(bad());
    }
    let phoff = usize::try_from(u64::from_le_bytes(
        bytes[32..40].try_into().expect("ELF offset"),
    ))
    .map_err(|_| bad())?;
    let entry = usize::from(u16::from_le_bytes([bytes[54], bytes[55]]));
    let count = usize::from(u16::from_le_bytes([bytes[56], bytes[57]]));
    if entry != 56
        || count == 0
        || count > 1024
        || phoff < 64
        || phoff
            .checked_add(entry * count)
            .is_none_or(|end| end > bytes.len())
    {
        return Err(bad());
    }
    let mut interpreter = None;
    for index in 0..count {
        let header = &bytes[phoff + index * entry..phoff + (index + 1) * entry];
        if u32::from_le_bytes(header[..4].try_into().expect("ELF type")) != 3 {
            continue;
        }
        if interpreter.is_some() {
            return Err(bad());
        }
        let offset = usize::try_from(u64::from_le_bytes(
            header[8..16].try_into().expect("ELF offset"),
        ))
        .map_err(|_| bad())?;
        let size = usize::try_from(u64::from_le_bytes(
            header[32..40].try_into().expect("ELF size"),
        ))
        .map_err(|_| bad())?;
        if !(2..=4096).contains(&size) {
            return Err(bad());
        }
        let end = offset.checked_add(size).ok_or_else(bad)?;
        let text = bytes.get(offset..end).ok_or_else(bad)?;
        if text.last() != Some(&0) || text[..text.len() - 1].contains(&0) {
            return Err(bad());
        }
        interpreter = Some(&text[..text.len() - 1]);
    }
    match interpreter {
        None => Ok(FrpLaunch::Static),
        Some(b"/lib/ld-musl-x86_64.so.1") => Ok(FrpLaunch::Musl),
        _ => Err(InstallError::Invalid(
            "frpc requires an unsupported dynamic interpreter",
        )),
    }
}
fn probe_frpc_portability<R: CommandRunner>(
    runner: &mut R,
    work: &Path,
    frp: &FrpPayload,
) -> Result<(), InstallError> {
    validate_root(work)?;
    let root = work.join("frpc-portability-check");
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let result = (|| {
        write_target(&root, "opt/rustaller/frp/frpc", &frp.binary, 0o755)?;
        if let Some(loader) = &frp.loader {
            write_target(
                &root,
                "opt/rustaller/frp/ld-musl-x86_64.so.1",
                loader,
                0o755,
            )?;
        }
        write_target(&root, "etc/frp/frpc.toml", &frp.configuration, 0o600)?;
        for options in [
            vec!["--version"],
            vec!["verify", "-c", "/etc/frp/frpc.toml"],
        ] {
            let mut args = vec![frp.launch.executable()];
            args.extend(frp.launch.arguments());
            args.extend(options);
            run_checked(
                runner,
                &chroot_command(&root, args, "prewipe-frpc-portability-check")
                    .env("LD_LIBRARY_PATH", "")
                    .env("LD_PRELOAD", "")
                    .stdout_mode(OutputMode::Capture)
                    .stderr_mode(OutputMode::Capture),
            )?;
        }
        Ok(())
    })();
    // Only this create-new, private probe tree belongs to this operation.
    let _ = fs::remove_dir_all(root);
    result
}

pub fn start_live_services<R: CommandRunner>(
    runner: &mut R,
    plan: &ValidatedLivePlan,
    work: &Path,
) -> Result<(), InstallError> {
    if let Some(port) = plan.plan().web_port {
        let mut random = [0_u8; 8];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let alphabet = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let token = random
            .into_iter()
            .map(|b| alphabet[usize::from(b) % alphabet.len()] as char)
            .collect::<String>();
        let web_path = format!("/{token}");
        let web = work.join("web");
        fs::create_dir(&web)?;
        atomic_replace(
            &web.join(&token),
            include_bytes!("../../logviewer.html"),
            0o644,
        )?;
        let log = Path::new("/rustaller.log");
        if !log.exists() {
            symlink("/var/log/rustaller.log", log)?;
        }
        let service = format!(
            "#!/sbin/openrc-run\nname=rustaller-web\ncommand=/usr/bin/websocketd\ncommand_args=\"--port={port} --staticdir={} /rustaller-worker log-stream {web_path}\"\ncommand_background=yes\npidfile=/run/rustaller-web.pid\n",
            web.display()
        );
        atomic_replace(
            Path::new("/etc/init.d/rustaller-web"),
            service.as_bytes(),
            0o755,
        )?;
        checked(
            runner,
            "rc-service",
            ["rustaller-web", "start"],
            "live-start-websocketd",
        )?;
        println!("Installation log URL path: {web_path} (HTTP port {port})");
    }
    if plan.plan().frpc_config.is_some() {
        atomic_replace(Path::new("/etc/init.d/rustaller-frpc"),b"#!/sbin/openrc-run\nname=rustaller-frpc\ncommand=/usr/bin/frpc\ncommand_args=\"-c /configs/frpc.toml\"\ncommand_background=yes\npidfile=/run/rustaller-frpc.pid\n",0o755)?;
        checked(
            runner,
            "rc-service",
            ["rustaller-frpc", "start"],
            "live-start-frpc",
        )?;
    }
    Ok(())
}
pub(crate) fn start_recovery_ssh<R: CommandRunner>(
    runner: &mut R,
    plan: &ValidatedLivePlan,
) -> Result<(), InstallError> {
    // Rescue/recovery login remains available if a later install stage fails.
    let plan = plan.plan();
    let name = plan.username.as_deref().unwrap_or("root");
    if name != "root" {
        checked(
            runner,
            "adduser",
            ["-D", "-s", "/bin/ash", name],
            "live-create-recovery-account",
        )?;
        fs::create_dir_all("/etc/sudoers.d")?;
        atomic_replace(
            Path::new("/etc/sudoers.d/90-rustaller"),
            format!(
                "{name} ALL=(ALL:ALL) {}ALL\n",
                if plan.password.is_none() {
                    "NOPASSWD: "
                } else {
                    ""
                }
            )
            .as_bytes(),
            0o440,
        )?;
        checked(
            runner,
            "visudo",
            ["-cf", "/etc/sudoers.d/90-rustaller"],
            "live-verify-recovery-sudo",
        )?;
    }
    if let Some(password) = &plan.password {
        run_checked(
            runner,
            &command("chpasswd", [] as [&str; 0], "live-set-recovery-password")
                .stdin(CommandInput::Bytes(
                    format!("{name}:{}\n", password.expose()).into_bytes(),
                ))
                .stdout_mode(OutputMode::Discard)
                .stderr_mode(OutputMode::Capture),
        )?;
    } else {
        let home = if name == "root" {
            PathBuf::from("/root")
        } else {
            PathBuf::from(format!("/home/{name}"))
        };
        fs::create_dir_all(home.join(".ssh"))?;
        fs::set_permissions(home.join(".ssh"), fs::Permissions::from_mode(0o700))?;
        atomic_replace(
            &home.join(".ssh/authorized_keys"),
            format!("{}\n", plan.ssh_keys.join("\n")).as_bytes(),
            0o600,
        )?;
        checked(
            runner,
            "passwd",
            ["-d", name],
            "live-unlock-public-key-account",
        )?;
        checked(
            runner,
            "chown",
            ["-R", &format!("{name}:{name}"), &home.to_string_lossy()],
            "live-recovery-key-owner",
        )?;
    }
    let config = format!(
        "Port {}\nPermitRootLogin {}\nPasswordAuthentication {}\nPermitEmptyPasswords no\nPubkeyAuthentication yes\n",
        plan.ssh_port.unwrap_or(22),
        if name == "root" { "yes" } else { "no" },
        if plan.password.is_some() { "yes" } else { "no" }
    );
    atomic_replace(Path::new("/etc/ssh/sshd_config"), config.as_bytes(), 0o600)?;
    checked(runner, "ssh-keygen", ["-A"], "live-recovery-host-keys")?;
    checked(
        runner,
        "rc-service",
        ["sshd", "restart"],
        "live-start-recovery-ssh",
    )
}

pub fn verify_gentoo_stage3<R: CommandRunner>(
    runner: &mut R,
    source: &GentooStage3,
    work: &Path,
) -> Result<VerifiedStage3, InstallError> {
    let gpg = work.join("gpg");
    fs::create_dir(&gpg)?;
    fs::set_permissions(&gpg, fs::Permissions::from_mode(0o700))?;
    let bundle = work.join("gentoo-service-keys.gpg");
    download(runner, GENTOO_KEYS, &bundle, 2 * 1024 * 1024)?;
    checked(
        runner,
        "gpg",
        [
            OsStr::new("--batch"),
            "--homedir".as_ref(),
            gpg.as_os_str(),
            "--import".as_ref(),
            bundle.as_os_str(),
        ],
        "gentoo-import-release-public-keys",
    )?;
    let keyring = work.join("gentoo-weekly.gpg");
    checked(
        runner,
        "gpg",
        [
            OsStr::new("--batch"),
            "--homedir".as_ref(),
            gpg.as_os_str(),
            "--output".as_ref(),
            keyring.as_os_str(),
            "--export".as_ref(),
            GENTOO_RELEASE_PRIMARY.as_ref(),
        ],
        "gentoo-export-pinned-release-key",
    )?;
    let key_meta = fs::metadata(&keyring)?;
    if key_meta.len() == 0 || key_meta.len() > 2 * 1024 * 1024 {
        return Err(InstallError::Invalid("pinned Gentoo release key is absent"));
    }
    let signature = work.join("gentoo-stage3.asc");
    let digests = work.join("gentoo-stage3.DIGESTS");
    let archive = work.join(&source.filename);
    download(runner, &source.signature_url, &signature, 64 * 1024)?;
    download(runner, &source.digests_url, &digests, 1024 * 1024)?;
    download(runner, &source.url, &archive, source.size)?;
    if fs::metadata(&archive)?.len() != source.size {
        return Err(InstallError::Invalid(
            "Gentoo stage3 size differs from selected release",
        ));
    }
    let verify = command(
        "gpg",
        [
            OsStr::new("--batch"),
            "--homedir".as_ref(),
            gpg.as_os_str(),
            "--no-default-keyring".as_ref(),
            "--keyring".as_ref(),
            keyring.as_os_str(),
            "--status-fd=1".as_ref(),
            "--verify".as_ref(),
            signature.as_os_str(),
            archive.as_os_str(),
        ],
        "gentoo-verify-stage3-signature",
    )
    .stdout_mode(OutputMode::Capture)
    .stderr_mode(OutputMode::Capture);
    let status = capture(runner, &verify)?;
    if !valid_gentoo_signature(&status) {
        return Err(InstallError::Invalid(
            "Gentoo archive signature is not bound to the pinned release primary key",
        ));
    }
    let digest_text = fs::read_to_string(&digests)?;
    let hash = gentoo_sha512(source, &digest_text)?;
    let checksum = command(
        "sha512sum",
        ["--check", "--status"],
        "gentoo-check-stage3-payload",
    )
    .stdin(CommandInput::Bytes(
        format!("{hash}  {}\n", archive.display()).into_bytes(),
    ));
    run_checked(runner, &checksum)?;
    let members = capture(
        runner,
        &command(
            "tar",
            [OsStr::new("-tJf"), archive.as_os_str()],
            "gentoo-check-archive-paths",
        ),
    )?;
    if members
        .lines()
        .any(|path| path.starts_with('/') || path.split('/').any(|part| part == ".."))
    {
        return Err(InstallError::Invalid(
            "Gentoo stage3 contains unsafe archive paths",
        ));
    }
    Ok(VerifiedStage3 { path: archive })
}
pub fn valid_gentoo_signature(status: &str) -> bool {
    if status.lines().any(|line| {
        [
            "BADSIG",
            "ERRSIG",
            "EXPKEYSIG",
            "EXPSIG",
            "REVKEYSIG",
            "KEYEXPIRED",
        ]
        .iter()
        .any(|bad| line.starts_with(&format!("[GNUPG:] {bad} ")))
    }) {
        return false;
    }
    let signatures = status
        .lines()
        .filter_map(|line| line.strip_prefix("[GNUPG:] VALIDSIG "))
        .collect::<Vec<_>>();
    signatures.len() == 1 && signatures[0].split_whitespace().last() == Some(GENTOO_RELEASE_PRIMARY)
}
pub fn gentoo_sha512(source: &GentooStage3, metadata: &str) -> Result<String, InstallError> {
    let body = if metadata.starts_with("-----BEGIN PGP SIGNED MESSAGE-----") {
        sources::clearsigned_payload(metadata)
            .map_err(|_| InstallError::Invalid("invalid Gentoo digest metadata"))?
    } else {
        metadata
    };
    let mut sha = false;
    let mut result = None;
    for line in body.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            sha = line == "# SHA512 HASH";
            continue;
        }
        if !sha {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 2 {
            return Err(InstallError::Invalid("invalid SHA512 digest entry"));
        }
        if fields[1] == source.filename
            && (fields[0].len() != 128
                || !fields[0].bytes().all(|b| b.is_ascii_hexdigit())
                || result.replace(fields[0].to_owned()).is_some())
        {
            return Err(InstallError::Invalid(
                "invalid or duplicate Gentoo stage3 digest",
            ));
        }
    }
    result.ok_or(InstallError::Invalid(
        "selected Gentoo stage3 digest is missing",
    ))
}

fn installed_module_versions(root: &Path) -> Result<Vec<String>, InstallError> {
    let directory = fs::canonicalize(root.join("lib/modules"))?;
    if !directory.starts_with(root) {
        return Err(InstallError::Invalid(
            "target module directory escapes target root",
        ));
    }
    let mut versions = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| InstallError::Invalid("non-UTF-8 kernel release"))?;
        if entry.file_type()?.is_dir()
            && name.as_bytes().first().is_some_and(u8::is_ascii_digit)
            && name.len() <= 128
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        {
            versions.push(name);
        }
    }
    if versions.is_empty() {
        return Err(InstallError::Invalid(
            "installed target has no kernel modules",
        ));
    }
    versions.sort();
    Ok(versions)
}
const TARGET_TRANSPORT_CA: &str = "/etc/ssl/certs/rustaller-install.pem";
fn copy_live_transport(root: &Path, target: Target) -> Result<(), InstallError> {
    let source = Path::new("/configs/transport-ca.pem");
    if !source.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(source)?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.len() > 4 * 1024 * 1024 {
        return Err(InstallError::Invalid("invalid private transport CA file"));
    }
    let destination = root.join(TARGET_TRANSPORT_CA.trim_start_matches('/'));
    if destination.exists() {
        return Err(InstallError::Invalid(
            "target already contains an installer transport CA",
        ));
    }
    validate_planned_file(&destination)?;
    ensure_parent(&destination)?;
    // Public certificate material must remain readable to APT's _apt sandbox.
    for directory in ["etc/ssl", "etc/ssl/certs"] {
        fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o755))?;
    }
    atomic_replace(&destination, &fs::read(source)?, 0o644)?;
    if target == Target::Debian {
        write_target(
            root,
            "etc/apt/apt.conf.d/99rustaller-install-transport",
            format!("Acquire::https::CaInfo \"{TARGET_TRANSPORT_CA}\";\n").as_bytes(),
            0o644,
        )?;
    }
    Ok(())
}
fn remove_live_transport(root: &Path) -> Result<(), InstallError> {
    for path in [
        TARGET_TRANSPORT_CA.trim_start_matches('/'),
        "etc/apt/apt.conf.d/99rustaller-install-transport",
    ] {
        let path = root.join(path);
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}
struct TargetTransportRunner<'a, R> {
    inner: &'a mut R,
    root: &'a Path,
}
impl<R: CommandRunner> CommandRunner for TargetTransportRunner<'_, R> {
    fn run(&mut self, spec: &CommandSpec) -> io::Result<crate::command::CommandOutput> {
        let mut spec = spec.clone();
        if spec.program == "chroot"
            && spec
                .args
                .first()
                .is_some_and(|arg| arg == self.root.as_os_str())
            && self
                .root
                .join(TARGET_TRANSPORT_CA.trim_start_matches('/'))
                .is_file()
        {
            for name in [
                "SSL_CERT_FILE",
                "CURL_CA_BUNDLE",
                "SSL_CA_CERT_FILE",
                "REQUESTS_CA_BUNDLE",
            ] {
                spec = spec.env(name, TARGET_TRANSPORT_CA);
            }
        }
        self.inner.run(&spec)
    }
    fn run_pipeline(
        &mut self,
        specs: &[CommandSpec],
    ) -> io::Result<crate::command::PipelineOutput> {
        self.inner.run_pipeline(specs)
    }
}
fn clean_target_tree(
    root: &Path,
    relative: &str,
    preserve_licenses: bool,
) -> Result<(), InstallError> {
    let approved = [
        "usr/share/doc",
        "usr/share/man",
        "usr/share/info",
        "usr/share/locale",
        "var/cache/apk",
        "var/cache/apt/archives",
        "var/lib/apt/lists",
        "var/cache/pacman/pkg",
        "var/cache/distfiles",
        "var/cache/binpkgs",
    ];
    if !approved.contains(&relative) {
        return Err(InstallError::Invalid("unapproved target cleanup path"));
    }
    let directory = root.join(relative);
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    crate::shared::configure::validate_directory_tree(&directory)?;
    let root_device = fs::metadata(root)?.dev();
    if metadata.dev() != root_device {
        return Err(InstallError::Invalid(
            "cleanup path is a different mounted filesystem",
        ));
    }
    fn clean(
        directory: &Path,
        device: u64,
        preserve: bool,
        visited: &mut usize,
    ) -> Result<(), InstallError> {
        for entry in fs::read_dir(directory)? {
            *visited += 1;
            if *visited > 1000000 {
                return Err(InstallError::Invalid(
                    "target cleanup inventory exceeds limit",
                ));
            }
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.dev() != device {
                return Err(InstallError::Invalid(
                    "cleanup would cross a filesystem boundary",
                ));
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if preserve
                && (name.starts_with("copyright")
                    || name.starts_with("license")
                    || name.starts_with("copying")
                    || name == "locale.alias")
            {
                continue;
            }
            if metadata.is_dir() {
                clean(&path, device, preserve, visited)?;
                match fs::remove_dir(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {}
                    Err(error) => return Err(error.into()),
                }
            } else if metadata.is_file() || metadata.is_symlink() {
                fs::remove_file(path)?;
            } else {
                return Err(InstallError::Invalid(
                    "special file found in target cleanup tree",
                ));
            }
        }
        Ok(())
    }
    clean(&directory, root_device, preserve_licenses, &mut 0)
}
fn copy_apk_keys(root: &Path) -> Result<(), InstallError> {
    for entry in fs::read_dir("/etc/apk/keys")? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            write_target(
                root,
                &format!("etc/apk/keys/{}", entry.file_name().to_string_lossy()),
                &fs::read(entry.path())?,
                0o644,
            )?;
        }
    }
    Ok(())
}
fn copy_live_resolver(root: &Path) -> Result<(), InstallError> {
    let bytes = fs::read("/etc/resolv.conf")?;
    if bytes.len() > 1024 * 1024 || bytes.contains(&0) {
        return Err(InstallError::Invalid("invalid Live resolver configuration"));
    }
    let path = root.join("etc/resolv.conf");
    crate::shared::configure::validate_destination(&path, true)?;
    ensure_parent(&path)?;
    atomic_replace(&path, &bytes, 0o644)?;
    Ok(())
}
fn valid_username(value: &str) -> bool {
    crate::model::valid_account_name(value)
}
fn literal_public_key(key: &str) -> bool {
    let mut fields = key.split_whitespace();
    matches!(
        fields.next(),
        Some(
            "ssh-ed25519"
                | "ssh-rsa"
                | "ecdsa-sha2-nistp256"
                | "ecdsa-sha2-nistp384"
                | "ecdsa-sha2-nistp521"
                | "sk-ssh-ed25519@openssh.com"
                | "sk-ecdsa-sha2-nistp256@openssh.com"
        )
    ) && fields.next().is_some_and(|value| {
        value.len() >= 4
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    }) && !key.chars().any(char::is_control)
}
fn valid_uuid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}
fn target_hostname(plan: &crate::model::InstallPlan) -> String {
    plan.post_install
        .hostname
        .clone()
        .unwrap_or_else(|| plan.target.to_string().to_ascii_lowercase())
}

fn write_target_hosts(root: &Path, hostname: &str) -> Result<(), InstallError> {
    // This is a freshly installed target, not the current-system editor.
    write_target(
        root,
        "etc/hosts",
        format!(
            "127.0.0.1 localhost\n127.0.1.1 {hostname}\n::1 localhost ip6-localhost ip6-loopback\n"
        )
        .as_bytes(),
        0o644,
    )
}
fn write_target(root: &Path, name: &str, bytes: &[u8], mode: u32) -> Result<(), InstallError> {
    crate::shared::dns::write_config(&root.join(name), bytes, mode)
}
pub(crate) fn command<I, S>(program: &str, args: I, phase: &str) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    crate::command::target_command(program, args, phase)
        .env("LC_ALL", "C")
        .env("DEBIAN_FRONTEND", "noninteractive")
}
pub(crate) fn checked<R, I, S>(
    runner: &mut R,
    program: &str,
    args: I,
    phase: &str,
) -> Result<(), InstallError>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_checked(runner, &command(program, args, phase))?;
    Ok(())
}
pub(crate) fn capture<R: CommandRunner>(
    runner: &mut R,
    spec: &CommandSpec,
) -> Result<String, InstallError> {
    let bytes = crate::command::capture_checked(runner, spec)?;
    String::from_utf8(bytes)
        .map_err(|_| InstallError::Invalid("native command returned invalid UTF-8"))
}
fn chroot_command<I, S>(root: &Path, args: I, phase: &str) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut values = vec![root.as_os_str().to_owned()];
    values.extend(args.into_iter().map(|s| s.as_ref().to_owned()));
    command("chroot", values, phase)
}
fn chroot_checked<R, I, S>(
    runner: &mut R,
    root: &Path,
    args: I,
    phase: &str,
) -> Result<(), InstallError>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_checked(runner, &chroot_command(root, args, phase))?;
    Ok(())
}
fn download<R: CommandRunner>(
    runner: &mut R,
    url: &str,
    path: &Path,
    limit: u64,
) -> Result<(), InstallError> {
    if !url.starts_with("https://")
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
        || path.exists()
    {
        return Err(InstallError::Invalid("unsafe download identity"));
    }
    let args = vec![
        OsString::from("--disable"),
        "--fail".into(),
        "--silent".into(),
        "--show-error".into(),
        "--location".into(),
        "--proto".into(),
        "=https".into(),
        "--proto-redir".into(),
        "=https".into(),
        "--connect-timeout".into(),
        "15".into(),
        "--max-time".into(),
        "3600".into(),
        "--retry".into(),
        "2".into(),
        "--max-filesize".into(),
        limit.to_string().into(),
        "--output".into(),
        path.as_os_str().to_owned(),
        url.into(),
    ];
    run_checked(
        runner,
        &command("curl", args, "live-verified-asset-download").stderr_mode(OutputMode::Capture),
    )?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > limit {
        return Err(InstallError::Invalid("downloaded payload is invalid"));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::command::{CommandOutput, MockRunner};
    use crate::config::{CongestionControl, NetworkConfig, QueueDiscipline};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    #[test]
    fn grub_validation_rejects_wrong_filesystem_and_invalid_config() {
        for (fs_type, status, accepted, count) in [
            ("xfs\n", 0, true, 2),
            ("ext2\n", 0, false, 1),
            ("xfs\n", 1, false, 2),
        ] {
            let mut runner = MockRunner::new([
                CommandOutput::new(Some(0), fs_type.as_bytes().to_vec(), vec![]),
                CommandOutput::new(Some(status), vec![], vec![]),
            ]);
            assert_eq!(
                verify_grub_root(
                    &mut runner,
                    Path::new("/isolated-target"),
                    crate::config::RootFilesystem::Xfs
                )
                .is_ok(),
                accepted
            );
            assert_eq!(runner.calls().len(), count);
        }
    }
    #[test]
    fn ext4_grub_uses_the_ext2_driver_and_fstab_fsck_pass() {
        let root = Scratch::new();
        let mut install = crate::model::InstallPlan::new(Target::Debian);
        install.system.root_filesystem = crate::config::RootFilesystem::Ext4;
        let plan = crate::worker::live::validate_live_plan_with_uid(
            install,
            "x86_64",
            4096,
            crate::RUNTIME_VERSION,
            0,
        )
        .unwrap();
        let layout = DiskLayout::new(&plan, "vda", BootMode::Bios, 8 * 1024_u64.pow(3)).unwrap();
        write_fstab(&root.0, &layout, "1234-ABCD", None).unwrap();
        assert_eq!(
            fs::read_to_string(root.0.join("etc/fstab")).unwrap(),
            "UUID=1234-ABCD / ext4 defaults 0 1\n"
        );
        let mut runner = MockRunner::new([
            CommandOutput::new(Some(0), b"ext2\n".to_vec(), vec![]),
            CommandOutput::new(Some(0), vec![], vec![]),
        ]);
        verify_grub_root(&mut runner, &root.0, crate::config::RootFilesystem::Ext4).unwrap();
        let mut wrong = MockRunner::new([CommandOutput::new(Some(0), b"xfs\n".to_vec(), vec![])]);
        assert!(
            verify_grub_root(&mut wrong, &root.0, crate::config::RootFilesystem::Ext4).is_err()
        );
    }
    #[test]
    fn non_root_accounts_set_root_password_privately_without_enabling_root_ssh() {
        for key_auth in [false, true] {
            let root = Scratch::new();
            root.write("etc/ssh/sshd_config", b"# base\n");
            let mut install = crate::model::InstallPlan::new(Target::Debian);
            install.username = Some("alice".into());
            install.root_password = Some(crate::model::SecretString::new("PRIVATE-ROOT-SENTINEL"));
            if key_auth {
                install
                    .ssh_keys
                    .push("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestfixture alice".into());
            } else {
                install.password = Some(crate::model::SecretString::new("PRIVATE-USER-SENTINEL"));
            }
            let plan = crate::worker::live::validate_live_plan_with_uid(
                install,
                "x86_64",
                4096,
                crate::RUNTIME_VERSION,
                0,
            )
            .unwrap();
            let mut runner =
                MockRunner::new((0..20).map(|_| CommandOutput::new(Some(0), vec![], vec![])));
            configure_accounts(&mut runner, &root.0, &plan).unwrap();
            let call = runner
                .calls()
                .iter()
                .find(|c| c.phase == "target-set-root-password")
                .unwrap();
            assert_eq!(
                call.stdin,
                CommandInput::Bytes(b"root:PRIVATE-ROOT-SENTINEL\n".to_vec())
            );
            assert_eq!(call.stdout_mode, OutputMode::Discard);
            assert_eq!(call.stderr_mode, OutputMode::Capture);
            assert!(!format!("{call:?}").contains("PRIVATE-ROOT-SENTINEL"));
            assert!(
                !call
                    .args
                    .iter()
                    .any(|arg| arg.to_string_lossy().contains("PRIVATE-ROOT-SENTINEL"))
            );
            let config = fs::read_to_string(root.0.join("etc/ssh/sshd_config")).unwrap();
            assert!(config.contains("PermitRootLogin no\n"));
            assert!(config.contains(if key_auth {
                "PasswordAuthentication no\n"
            } else {
                "PasswordAuthentication yes\n"
            }));
        }
    }
    #[test]
    fn missing_root_password_stops_before_creating_non_root_account() {
        let root = Scratch::new();
        let mut install = crate::model::InstallPlan::new(Target::Debian);
        install.username = Some("alice".into());
        install.password = Some(crate::model::SecretString::new("PRIVATE-USER-SENTINEL"));
        let plan = crate::worker::live::validate_live_plan_with_uid(
            install,
            "x86_64",
            4096,
            crate::RUNTIME_VERSION,
            0,
        )
        .unwrap();
        let mut runner = MockRunner::empty();
        assert!(configure_accounts(&mut runner, &root.0, &plan).is_err());
        assert!(runner.calls().is_empty());
        assert!(!root.0.join("etc/sudoers.d/90-rustaller").exists());
    }
    #[test]
    fn new_target_hostname_resolves_locally_and_hosts_write_is_repeatable() {
        let root = Scratch::new();
        root.write("etc/hosts", b"127.0.1.1 template-host\n");
        for _ in 0..2 {
            write_target_hosts(&root.0, "vps-node").unwrap();
        }
        let hosts = fs::read_to_string(root.0.join("etc/hosts")).unwrap();
        assert_eq!(hosts.matches("127.0.1.1 vps-node").count(), 1);
        assert!(!hosts.contains("template-host"));
        assert!(hosts.contains("::1 localhost"));
    }
    #[test]
    fn absent_hostname_uses_selected_system_name_and_explicit_hostname_wins() {
        for (target, expected) in [
            (Target::Alpine, "alpine"),
            (Target::Debian, "debian"),
            (Target::Arch, "arch"),
            (Target::Gentoo, "gentoo"),
        ] {
            let root = Scratch::new();
            let mut plan = crate::model::InstallPlan::new(target);
            let hostname = target_hostname(&plan);
            assert_eq!(hostname, expected);
            write_target_hosts(&root.0, &hostname).unwrap();
            assert!(
                fs::read_to_string(root.0.join("etc/hosts"))
                    .unwrap()
                    .contains(&format!("127.0.1.1 {expected}\n"))
            );
            plan.post_install.hostname = Some("previous-host".into());
            assert_eq!(target_hostname(&plan), "previous-host");
        }
    }
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "rustaller-runtime-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn write(&self, name: &str, bytes: &[u8]) {
            let path = self.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn efi_loader_rejects_ia32_non_efi_truncated_and_empty_payloads() {
        let scratch = Scratch::new();
        let path = scratch.0.join("BOOTX64.EFI");
        let mut good = vec![0; 512];
        good[..2].copy_from_slice(b"MZ");
        good[60..64].copy_from_slice(&64u32.to_le_bytes());
        good[64..70].copy_from_slice(b"PE\0\0\x64\x86");
        good[84..86].copy_from_slice(&240u16.to_le_bytes());
        good[88..90].copy_from_slice(b"\x0b\x02");
        good[156..158].copy_from_slice(&10u16.to_le_bytes());
        fs::write(&path, &good).unwrap();
        verify_efi_loader(&path).unwrap();
        for (at, value) in [(0, 0), (68, 0x4c), (89, 1), (156, 3), (63, 0xff)] {
            let mut bad = good.clone();
            bad[at] = value;
            fs::write(&path, bad).unwrap();
            assert!(verify_efi_loader(&path).is_err());
        }
        for length in [0, 63, 90, 157, 327] {
            fs::write(&path, &good[..length]).unwrap();
            assert!(verify_efi_loader(&path).is_err());
        }
    }
    fn elf(interpreter: Option<&str>) -> Vec<u8> {
        let count = usize::from(interpreter.is_some()) + 1;
        let mut bytes = vec![0; 64 + 56 * count];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&(count as u16).to_le_bytes());
        bytes[64..68].copy_from_slice(&1u32.to_le_bytes());
        if let Some(value) = interpreter {
            let offset = bytes.len();
            bytes[120..124].copy_from_slice(&3u32.to_le_bytes());
            bytes[128..136].copy_from_slice(&(offset as u64).to_le_bytes());
            bytes[152..160].copy_from_slice(&((value.len() + 1) as u64).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
            bytes.push(0);
        }
        bytes
    }
    fn network() -> NetworkConfig {
        NetworkConfig {
            congestion: CongestionControl::Bbr,
            qdisc: QueueDiscipline::Fq,
            ..Default::default()
        }
    }

    #[test]
    fn frpc_launch_admits_static_or_exact_musl_and_rejects_malformed_or_foreign_interpreters() {
        assert_eq!(frpc_launch(&elf(None)).unwrap(), FrpLaunch::Static);
        assert_eq!(
            frpc_launch(&elf(Some("/lib/ld-musl-x86_64.so.1"))).unwrap(),
            FrpLaunch::Musl
        );
        assert!(frpc_launch(&elf(Some("/lib64/ld-linux-x86-64.so.2"))).is_err());
        let mut bytes = elf(None);
        bytes[18..20].copy_from_slice(&183u16.to_le_bytes());
        assert!(frpc_launch(&bytes).is_err());
        let mut bytes = elf(Some("/lib/ld-musl-x86_64.so.1"));
        bytes.pop();
        assert!(frpc_launch(&bytes).is_err());
        assert!(frpc_launch(b"not an ELF").is_err());
    }
    #[test]
    fn frpc_probe_is_preinstall_native_chroot_without_live_libraries_and_cleans_private_files() {
        let scratch = Scratch::new();
        let payload = FrpPayload {
            binary: elf(None),
            loader: None,
            configuration: b"serverAddr = 'example.invalid'\n".to_vec(),
            launch: FrpLaunch::Static,
        };
        let mut runner = MockRunner::new([
            CommandOutput::new(Some(0), b"test version".to_vec(), vec![]),
            CommandOutput::new(Some(0), vec![], vec![]),
        ]);
        probe_frpc_portability(&mut runner, &scratch.0, &payload).unwrap();
        assert_eq!(runner.calls().len(), 2);
        assert_eq!(runner.calls()[0].program, "chroot");
        assert_eq!(runner.calls()[0].args[1], "/opt/rustaller/frp/frpc");
        assert_eq!(runner.calls()[0].args[2], "--version");
        assert!(!scratch.0.join("frpc-portability-check").exists());
        let mut failure = MockRunner::new([CommandOutput::new(Some(127), vec![], vec![])]);
        assert!(probe_frpc_portability(&mut failure, &scratch.0, &payload).is_err());
        assert!(!scratch.0.join("frpc-portability-check").exists());
    }
    #[test]
    fn target_frpc_services_use_direct_static_execution_or_the_staged_musl_loader() {
        for (target, launch) in [
            (Target::Alpine, FrpLaunch::Static),
            (Target::Debian, FrpLaunch::Static),
            (Target::Arch, FrpLaunch::Musl),
        ] {
            let scratch = Scratch::new();
            let payload = FrpPayload {
                binary: elf(if launch == FrpLaunch::Musl {
                    Some("/lib/ld-musl-x86_64.so.1")
                } else {
                    None
                }),
                loader: (launch == FrpLaunch::Musl).then(|| vec![1, 2, 3]),
                configuration: b"private test config".to_vec(),
                launch,
            };
            install_frpc(&scratch.0, target, &payload).unwrap();
            let service = fs::read_to_string(scratch.0.join(if target == Target::Alpine {
                "etc/init.d/rustaller-frpc"
            } else {
                "etc/systemd/system/rustaller-frpc.service"
            }))
            .unwrap();
            assert_eq!(
                service.contains("ld-musl-x86_64.so.1"),
                launch == FrpLaunch::Musl
            );
            assert_eq!(
                fs::metadata(scratch.0.join("etc/frp/frpc.toml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn arch_module_inventory_uses_exact_builtins_and_compressed_modules_and_deduplicates_usrmerge()
    {
        let scratch = Scratch::new();
        scratch.write(
            "usr/lib/modules/6.18-test/modules.builtin",
            b"kernel/net/ipv4/tcp_bbr.ko\n",
        );
        scratch.write(
            "usr/lib/modules/6.18-test/kernel/net/sched/sch_fq.ko.zst",
            b"authenticated package module fixture",
        );
        symlink("usr/lib", scratch.0.join("lib")).unwrap();
        let config = arch_module_capability_config(&scratch.0, &network()).unwrap();
        assert!(config.contains("CONFIG_TCP_CONG_BBR=y\n"));
        assert!(config.contains("CONFIG_NET_SCH_FQ=m\n"));
        scratch.write("usr/lib/modules/6.19-test/modules.builtin", b"");
        assert!(arch_module_capability_config(&scratch.0, &network()).is_err());
        let missing = Scratch::new();
        missing.write(
            "lib/modules/6.18-test/modules.builtin",
            b"kernel/net/ipv4/tcp_bbr_extra.ko\n",
        );
        missing.write(
            "lib/modules/6.18-test/kernel/net/sched/sch_fq.ko.gz",
            b"module fixture",
        );
        assert!(arch_module_capability_config(&missing.0, &network()).is_err());
        let outside = Scratch::new();
        symlink("/usr/lib", outside.0.join("lib")).unwrap();
        assert!(arch_module_capability_config(&outside.0, &network()).is_err());
    }
}

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "rustaller-resolver-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::create_dir(path.join("etc")).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn plan(target: Target) -> crate::model::InstallPlan {
        crate::model::InstallPlan::new(target)
    }
    fn validated(plan: crate::model::InstallPlan) -> ValidatedLivePlan {
        crate::worker::live::validate_live_plan_with_uid(
            plan,
            "x86_64",
            2048,
            crate::RUNTIME_VERSION,
            0,
        )
        .unwrap()
    }
    #[test]
    fn automatic_systemd_dns_returns_to_native_resolved_after_package_operations() {
        for target in [Target::Debian, Target::Arch, Target::Gentoo] {
            let root = Directory::new();
            let path = root.0.join("etc/resolv.conf");
            fs::write(&path, b"nameserver 192.0.2.53\n").unwrap();
            finalize_resolver(&root.0, &validated(plan(target))).unwrap();
            assert_eq!(
                fs::read_link(path).unwrap(),
                Path::new("/run/systemd/resolve/stub-resolv.conf")
            );
        }
    }
    #[test]
    fn explicit_dns_and_alpine_resolvers_are_preserved() {
        for target in [Target::Alpine, Target::Debian] {
            let root = Directory::new();
            let path = root.0.join("etc/resolv.conf");
            let bytes = b"nameserver 192.0.2.53\n";
            fs::write(&path, bytes).unwrap();
            let mut plan = plan(target);
            if target == Target::Debian {
                plan.system.dns.mode = crate::config::DnsMode::Static;
                plan.system.dns.servers.push("192.0.2.53".parse().unwrap());
            }
            finalize_resolver(&root.0, &validated(plan)).unwrap();
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}
