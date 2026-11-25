//! Real target adapters, invoked only by the identity-bound Alpine Live driver.
//! Downloads and native package tools retain their signature verification.
use crate::command::{
    CommandError, CommandInput, CommandRunner, CommandSpec, OutputMode, run_checked,
};
use crate::deployment::DeploymentManifest;
use crate::fsutil::atomic_replace;
use crate::host::sources::{self, GentooStage3};
use crate::model::Target;
use crate::shared::configure::{ConfigurationError, validate_root};
use crate::shared::configure::{
    ensure_parent, public_key_check, valid_public_key, validate_planned_file,
};
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
    Boot(String),
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
            Self::Boot(error) => write!(f, "Target boot setup failed: {error}"),
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
    let packages = live_packages(plan.plan())?;
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
    if plan.plan().target == Target::Arch && plan.plan().hold != Some(1) {
        super::cachyos::prepare_assets(runner, work)?;
    }
    if plan.plan().hold != Some(1) {
        super::boot::prepare(manifest.boot_mode, work).map_err(InstallError::Boot)?;
    }
    let stage3 = match &manifest.gentoo_stage3 {
        _ if plan.plan().hold == Some(1) => None,
        Some(stage) if plan.plan().target == Target::Gentoo => {
            stage.validate().map_err(|_| {
                InstallError::Invalid("Gentoo stage3 must be official amd64 nomultilib OpenRC")
            })?;
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

fn live_packages(plan: &crate::model::InstallPlan) -> Result<Vec<&'static str>, InstallError> {
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
        "wipefs",
        plan.system.root_filesystem.maintenance_package(),
        "coreutils",
        "dosfstools",
        "tar",
        "xz",
        "openssh",
    ];
    match plan.target {
        Target::Alpine => packages.extend(["apk-tools", "alpine-base"]),
        Target::Debian => packages.extend(["debootstrap", "debian-archive-keyring", "wget"]),
        Target::Arch => packages.extend(["arch-install-scripts", "archlinux-keyring"]),
        Target::Gentoo => {}
        _ => return Err(InstallError::Invalid("automatic target required")),
    }
    if plan.frpc_config.is_some() {
        packages.push("frp");
    }
    Ok(packages)
}

pub fn validate_install_options(
    plan: &ValidatedLivePlan,
    manifest: &DeploymentManifest,
) -> Result<(), InstallError> {
    let plan = plan.plan();
    plan.validate_install_credentials()?;
    manifest
        .validate()
        .map_err(|_| InstallError::Invalid("invalid deployment manifest"))?;
    if !plan.target.is_automatic() {
        return Err(InstallError::Invalid("automatic target required"));
    }
    if plan.ssh_keys.iter().any(|key| !valid_public_key(key)) {
        return Err(InstallError::Invalid(
            "public-key sources must be resolved on the original host before handoff",
        ));
    }
    if plan.target == Target::Debian && plan.hold != Some(1) {
        super::debian::validate_codename(&manifest.debian_codename)?;
    }
    if plan.target == Target::Gentoo && plan.hold != Some(1) {
        manifest
            .gentoo_stage3
            .as_ref()
            .ok_or(InstallError::Invalid("Gentoo release metadata is missing"))?
            .validate()
            .map_err(|_| {
                InstallError::Invalid("Gentoo stage3 must be official amd64 nomultilib OpenRC")
            })?;
    }
    if plan.hold != Some(1) {
        crate::shared::network::validate_network_policy(plan, manifest)?;
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
            let profile = super::packages::package_plan(plan.plan(), layout.boot_mode(), &[])?;
            args.extend(profile.bootstrap_packages.into_iter().map(Into::into));
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
                "--include=ca-certificates".into(),
                "--force-check-gpg".into(),
                format!("--keyring={keyring}").into(),
                manifest.debian_codename.clone().into(),
                root.as_os_str().to_owned(),
                sources::DEBIAN_ARCHIVE.into(),
            ];
            // GNU wget handles HTTPS CONNECT proxies used by debootstrap. Its
            // CA setting is independent of curl's and affects only Alpine Live.
            let mut bootstrap = command("debootstrap", args, "debian-verified-minbase");
            if Path::new(LIVE_TRANSPORT_CA).is_file() {
                let wgetrc = Path::new("/run/rustaller-debootstrap-wgetrc");
                atomic_replace(
                    wgetrc,
                    format!("ca_certificate = {LIVE_TRANSPORT_CA}\n").as_bytes(),
                    0o600,
                )?;
                bootstrap = bootstrap.env("WGETRC", wgetrc);
            }
            run_checked(runner, &bootstrap)?;
        }
        Target::Arch => rolling::bootstrap_arch_rootfs(
            runner,
            root,
            plan,
            &[],
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
    efi_uuid: &str,
) -> Result<(), InstallError> {
    validate_root(root)?;
    let mut transport = TargetTransportRunner {
        inner: runner,
        root,
    };
    let runner = &mut transport;
    // Native kernel/initramfs hooks must see the target root and ESP UUIDs,
    // not infer the installed root from the Alpine Live environment.
    super::boot::verify_mount(runner, root, efi_uuid).map_err(InstallError::Boot)?;
    write_fstab(root, layout, root_uuid, efi_uuid)?;
    crate::targets::packages::apply_package_policy(root, plan)?;
    prevent_package_service_start(root, plan.plan().target)?;
    let result = (|| {
        match plan.plan().target {
            Target::Alpine => {
                let packages = super::packages::package_plan(plan.plan(), layout.boot_mode(), &[])?;
                super::packages::verify_kernel_available(runner, root, plan.plan()).map_err(
                    |_| {
                        InstallError::Invalid("selected kernel package or dependencies unavailable")
                    },
                )?;
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
                let packages = super::debian::apt_package_list_for_plan(plan.plan(), &[])?;
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
                super::cachyos::configure_repositories(runner, root, &assets.work)?;
                rolling::configure_arch(runner, root, plan)?;
            }
            Target::Gentoo => rolling::configure_gentoo(runner, root, plan, &[], root_uuid)?,
            _ => return Err(InstallError::Invalid("automatic target required")),
        }
        super::packages::verify_boot_driver_support(
            root,
            layout.root_filesystem().as_str(),
            &manifest.disk.device,
            &manifest.network,
        )
        .map_err(|_| {
            InstallError::Invalid(
                "selected target kernel lacks required filesystem or guest storage/network drivers",
            )
        })?;
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
        crate::shared::configure::apply_basic_configuration(root, plan)?;
        write_target_hosts(root, &hostname)?;
        let needs_kernel_config =
            crate::shared::network::needs_kernel_config(&plan.plan().system.network);
        let kernel_config = if needs_kernel_config {
            installed_kernel_config(runner, root, plan)?
        } else {
            String::new()
        };
        if needs_kernel_config {
            verify_requested_modules(root, &kernel_config, plan)?;
        }
        crate::shared::swapfile::prepare_swapfile(runner, root, plan, |done, total| {
            if done == total || done % 128 == 0 {
                println!("Target swapfile: {done}/{total} MiB");
            }
        })?;
        enable_services(runner, root, plan.plan(), manifest)?;
        if let Some(frp) = &assets.frp {
            install_frpc(root, plan.plan(), frp)?;
        }
        let selected_kernel = crate::postinstall::required_kernel(plan.plan().target)
            .ok_or(InstallError::Invalid("automatic target required"))?;
        super::boot::record_kernel_family(root, selected_kernel).map_err(InstallError::Boot)?;
        super::boot::install(runner, root, layout, &assets.work, root_uuid, efi_uuid)
            .map_err(InstallError::Boot)?;
        if plan.plan().target == Target::Alpine {
            let worker = fs::read("/rustaller-worker")?;
            crate::worker::firstboot::install_firstboot_service(runner, root, &worker)?;
        }
        cleanup_cache(runner, root, plan)?;
        crate::shared::network::apply_network_configuration(root, plan, manifest, &kernel_config)?;
        verify_installed_target(runner, root, plan.plan())?;
        if plan.plan().target == Target::Arch {
            rolling::stop_arch_keyring_daemons(runner, root, plan)?;
        }
        remove_live_transport(root)?;
        Ok(())
    })();
    restore_package_service_start(root, plan.plan().target)?;
    result
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

fn configure_accounts<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
) -> Result<(), InstallError> {
    configure_key_only_ssh(
        runner,
        root,
        plan.plan(),
        plan.plan().target == Target::Alpine,
    )
}

fn configure_key_only_ssh<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &crate::model::InstallPlan,
    busybox_accounts: bool,
) -> Result<(), InstallError> {
    plan.validate_install_credentials()?;
    if plan.ssh_keys.iter().any(|key| !valid_public_key(key)) {
        return Err(InstallError::Invalid("invalid resolved SSH public key"));
    }
    let username = plan.account_name();
    let home = if username == "root" {
        "/root".into()
    } else {
        format!("/home/{username}")
    };
    let live = root == Path::new("/");
    if live {
        for key in &plan.ssh_keys {
            run_checked(
                runner,
                &public_key_check(key, "live-verify-resolved-public-key"),
            )
            .map_err(|_| InstallError::Invalid("Live ssh-keygen rejected a resolved public key"))?;
        }
    }
    if username != "root" {
        let args = if busybox_accounts {
            ["adduser", "-D", "-h", &home, "-s", "/bin/ash", username]
        } else {
            ["useradd", "-m", "-d", &home, "-s", "/bin/bash", username]
        };
        run_checked(
            runner,
            &account_command(root, &args, "ssh-create-admin-account"),
        )?;
        write_target(
            root,
            "etc/sudoers.d/90-rustaller",
            format!("{username} ALL=(ALL:ALL) NOPASSWD: ALL\n").as_bytes(),
            0o440,
        )?;
        run_checked(
            runner,
            &account_command(root, &["visudo", "-c"], "ssh-verify-sudoers"),
        )?;
    }
    // NP is deliberately not a usable crypt(3) hash. Non-PAM OpenSSH treats
    // native shadow lock markers such as '!' as disabling public-key login as
    // well. A nonempty invalid hash keeps the account usable without a password.
    let shadow = if username == "root" {
        "root:NP\n".into()
    } else {
        format!("root:NP\n{username}:NP\n")
    };
    run_checked(
        runner,
        &account_command(root, &["chpasswd", "-e"], "ssh-disable-account-passwords")
            .stdin(CommandInput::Bytes(shadow.into_bytes()))
            .stdout_mode(OutputMode::Discard)
            .stderr_mode(OutputMode::Capture),
    )?;

    if username != "root" {
        let administrator = capture(
            runner,
            &account_command(
                root,
                &[
                    "sudo", "-n", "-u", username, "--", "sudo", "-n", "-u", "root", "--", "id",
                    "-u",
                ],
                "ssh-verify-sudo-account",
            ),
        )?;
        if administrator.trim() != "0" {
            return Err(InstallError::Invalid(
                "selected SSH account has no effective passwordless sudo",
            ));
        }
    }

    let directory = format!("{home}/.ssh");
    let authorized = format!("{directory}/authorized_keys");
    let path = root.join(authorized.trim_start_matches('/'));
    let home_path = root.join(home.trim_start_matches('/'));
    crate::shared::configure::validate_directory_tree(&home_path)?;
    validate_planned_file(&path)?;
    ensure_parent(&path)?;
    fs::set_permissions(&home_path, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(
        path.parent().expect("SSH parent"),
        fs::Permissions::from_mode(0o700),
    )?;
    atomic_replace(
        &path,
        format!("{}\n", plan.ssh_keys.join("\n")).as_bytes(),
        0o600,
    )?;
    run_checked(
        runner,
        &account_command(
            root,
            &[
                "chown",
                "-h",
                "--",
                &format!("{username}:"),
                &home,
                &directory,
                &authorized,
            ],
            "ssh-set-account-file-owners",
        ),
    )?;
    run_checked(
        runner,
        &account_command(root, &["ssh-keygen", "-A"], "ssh-generate-server-host-keys"),
    )?;
    ensure_parent(&root.join("run/sshd/.privsep"))?;
    let native = capture(
        runner,
        &account_command(root, &["/usr/sbin/sshd", "-T"], "ssh-read-native-defaults"),
    )?;
    let config = key_only_ssh_config(plan, &native, live);
    write_target(root, "etc/ssh/sshd_config", config.as_bytes(), 0o600)?;
    verify_key_only_ssh(runner, root, plan)?;
    Ok(())
}

fn account_command(root: &Path, args: &[&str], phase: &str) -> CommandSpec {
    let (program, rest) = args.split_first().expect("native account command");
    if root == Path::new("/") {
        command(program, rest, phase)
    } else {
        chroot_command(root, args, phase)
    }
}

const NATIVE_SSH_SETTINGS: &[&str] = &[
    "usepam",
    "disableforwarding",
    "allowtcpforwarding",
    "allowagentforwarding",
    "allowstreamlocalforwarding",
    "x11forwarding",
    "x11uselocalhost",
    "x11displayoffset",
    "gatewayports",
    "permittunnel",
    "permitopen",
    "permitlisten",
    "subsystem",
];

fn key_only_ssh_config(plan: &crate::model::InstallPlan, native: &str, live: bool) -> String {
    let username = plan.account_name();
    let root_login = if username == "root" {
        "prohibit-password"
    } else {
        "no"
    };
    let port = plan.ssh_port.unwrap_or(22);
    let mut config = format!(
        "# rustaller SSH configuration\nPort {port}\n\
         AllowUsers {username}\nPermitRootLogin {root_login}\n\
         PubkeyAuthentication yes\nPasswordAuthentication no\n\
         KbdInteractiveAuthentication no\nPermitEmptyPasswords no\n\
         AuthenticationMethods publickey\nAuthorizedKeysFile .ssh/authorized_keys\n\
         StrictModes yes\n"
    );
    // A single authoritative file excludes native Include and Match rules,
    // whose per-connection semantics can override a prepended global policy.
    // Carry forward the native PAM choice, subsystems and forwarding behavior.
    for line in native.lines() {
        if let Some((key, value)) = line.split_once(' ')
            && NATIVE_SSH_SETTINGS.contains(&key)
            && !value.is_empty()
            && !value.chars().any(char::is_control)
            && (key != "usepam" || value == "yes")
            // Recovery formerly used only the packaged SSH defaults.
            && (!live || key != "subsystem")
        {
            config.push_str(line);
            config.push('\n');
        }
    }
    config
}

fn verify_key_only_ssh<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &crate::model::InstallPlan,
) -> Result<(), InstallError> {
    run_checked(
        runner,
        &account_command(root, &["/usr/sbin/sshd", "-t"], "ssh-verify-syntax"),
    )?;
    let facts = capture(
        runner,
        &account_command(
            root,
            &["/usr/sbin/sshd", "-T"],
            "ssh-verify-effective-policy",
        ),
    )?;
    if !valid_effective_ssh_policy(plan, &facts) {
        return Err(InstallError::Invalid(
            "OpenSSH effective key-only policy differs from the plan",
        ));
    }
    Ok(())
}

fn valid_effective_ssh_policy(plan: &crate::model::InstallPlan, facts: &str) -> bool {
    let setting = |key| {
        let mut values = facts.lines().filter_map(|line| {
            let (name, value) = line.split_once(' ')?;
            (key == name).then_some(value)
        });
        let first = values.next()?;
        values.next().is_none().then_some(first)
    };
    let username = plan.account_name();
    let root_logins = setting("permitrootlogin");
    setting("port").and_then(|port| port.parse().ok()) == Some(plan.ssh_port.unwrap_or(22))
        && setting("allowusers") == Some(username)
        && (if username == "root" {
            matches!(root_logins, Some("prohibit-password" | "without-password"))
        } else {
            root_logins == Some("no")
        })
        && setting("pubkeyauthentication") == Some("yes")
        && setting("passwordauthentication") == Some("no")
        && setting("kbdinteractiveauthentication") == Some("no")
        && setting("permitemptypasswords") == Some("no")
        && setting("authenticationmethods") == Some("publickey")
        && setting("authorizedkeysfile") == Some(".ssh/authorized_keys")
        && setting("strictmodes") == Some("yes")
}

fn enable_services<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &crate::model::InstallPlan,
    manifest: &DeploymentManifest,
) -> Result<(), InstallError> {
    let target = plan.target;
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
    } else if target == Target::Gentoo {
        let hostname = target_hostname(plan);
        write_target(
            root,
            "etc/conf.d/hostname",
            format!("hostname=\"{hostname}\"\n").as_bytes(),
            0o644,
        )?;
        for (name, level) in [
            ("udev", "sysinit"),
            ("udev-trigger", "sysinit"),
            ("hostname", "boot"),
            ("hwclock", "boot"),
            ("modules", "boot"),
            ("sysctl", "boot"),
            ("sshd", "default"),
            ("local", "default"),
        ] {
            chroot_checked(
                runner,
                root,
                ["rc-update", "add", name, level],
                "gentoo-enable-openrc-services",
            )?;
        }
        let net_lo = root.join("etc/init.d/net.lo");
        validate_planned_file(&net_lo)?;
        let net_meta = fs::symlink_metadata(&net_lo)?;
        if !net_meta.is_file() || net_meta.permissions().mode() & 0o111 == 0 {
            return Err(InstallError::Invalid(
                "Gentoo netifrc init script is missing",
            ));
        }
        for nic in &manifest.network {
            let service = format!("net.{}", nic.name);
            let path = root.join("etc/init.d").join(&service);
            crate::shared::configure::validate_planned_parent(&path)?;
            if let Ok(metadata) = fs::symlink_metadata(&path) {
                if !metadata.is_symlink() || fs::read_link(&path)? != Path::new("net.lo") {
                    return Err(InstallError::Invalid(
                        "unexpected existing Gentoo network service",
                    ));
                }
            } else {
                symlink("net.lo", &path)?;
            }
            chroot_checked(
                runner,
                root,
                ["rc-update", "add", &service, "default"],
                "gentoo-enable-netifrc-interface",
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
    efi_uuid: &str,
) -> Result<(), InstallError> {
    let boot_uuid = efi_uuid;
    let text = crate::shared::storage::render_fstab(layout, root_uuid, boot_uuid)
        .map_err(|_| InstallError::Invalid("invalid filesystem UUID"))?;
    write_target(root, "etc/fstab", text.as_bytes(), 0o644)
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
    let target_root = fs::canonicalize(root)?;
    for path in &mut candidates {
        *path = fs::canonicalize(&path)?;
        if !path.starts_with(&target_root) {
            return Err(InstallError::Invalid(
                "installed kernel configuration escapes target",
            ));
        }
        validate_planned_file(path)?;
    }
    candidates.sort();
    candidates.dedup();
    if candidates.is_empty() && plan.plan().target == Target::Arch {
        return arch_module_capability_config(root, &plan.plan().system.network);
    }
    if candidates.len() != 1 {
        return Err(InstallError::Invalid(
            "installed kernel configuration is missing or ambiguous",
        ));
    }
    let path = &candidates[0];
    let mut data = Vec::new();
    File::open(path)?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut data)?;
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
    let needed = crate::shared::network::kernel_requirements(network);
    let mut text = String::from(
        "# rustaller: capabilities from the authenticated installed kernel module inventory\n",
    );
    for (symbol, module) in needed {
        if module.is_empty() {
            return Err(ConfigurationError::UnsupportedNetworkKernel.into());
        }
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
    plan: &crate::model::InstallPlan,
) -> Result<(), InstallError> {
    for path in ["etc/os-release", "etc/fstab"] {
        let path = root.join(path);
        if !path.is_file() || fs::metadata(path)?.len() == 0 {
            return Err(InstallError::Invalid("installed OS or fstab is missing"));
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
    super::boot::verify(root).map_err(InstallError::Boot)?;

    verify_key_only_ssh(runner, root, plan)
}

// An existing filename is not sufficient: reject empty/truncated, IA32 and
// non-application PE files before requesting the sole final reboot.
pub(super) fn verify_efi_loader(path: &Path) -> Result<(), InstallError> {
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
fn install_frpc(
    root: &Path,
    plan: &crate::model::InstallPlan,
    frp: &FrpPayload,
) -> Result<(), InstallError> {
    write_target(root, "opt/rustaller/frp/frpc", &frp.binary, 0o755)?;
    if let Some(loader) = &frp.loader {
        write_target(root, "opt/rustaller/frp/ld-musl-x86_64.so.1", loader, 0o755)?;
    }
    write_target(root, "etc/frp/frpc.toml", &frp.configuration, 0o600)?;
    if plan.target.uses_openrc() {
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
) -> Result<(), InstallError> {
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
    // Recovery becomes reachable only after the native daemon verifies the
    // effective policy and the selected account's SSH keys.
    configure_key_only_ssh(runner, Path::new("/"), plan.plan(), true)?;
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
    source
        .validate()
        .map_err(|_| InstallError::Invalid("invalid Gentoo no-multilib source identity"))?;
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
    let verify = |runner: &mut R, inputs: &[&Path], phase, failure| {
        let spec = command(
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
            ]
            .into_iter()
            .chain(inputs.iter().map(|path| path.as_os_str())),
            phase,
        )
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Capture);
        if !valid_gentoo_signature(&capture(runner, &spec)?) {
            return Err(InstallError::Invalid(failure));
        }
        Ok(())
    };

    // The host previews the release identity. Confirm the exact same archive
    // against the release key before fetching it or changing the disk.
    let index = work.join("gentoo-stage3-index.txt");
    download(runner, sources::GENTOO_STAGE3_INDEX_URL, &index, 64 * 1024)?;
    verify(
        runner,
        &[&index],
        "gentoo-verify-signed-index",
        "Gentoo release index is not signed by the pinned release key",
    )?;
    let published = sources::parse_gentoo_stage3(&fs::read_to_string(&index)?, "x86_64")
        .map_err(|_| InstallError::Invalid("invalid signed Gentoo OpenRC release index"))?;
    if &published != source {
        return Err(InstallError::Invalid(
            "Gentoo release index changed since preparation; regenerate the installation plan",
        ));
    }

    let digests = work.join("gentoo-stage3.DIGESTS");
    download(runner, &source.digests_url, &digests, 1024 * 1024)?;
    verify(
        runner,
        &[&digests],
        "gentoo-verify-signed-digests",
        "Gentoo digest signature is not bound to the pinned release primary key",
    )?;
    let hash = sources::parse_gentoo_sha512(source, &fs::read_to_string(&digests)?)
        .map_err(|_| InstallError::Invalid("invalid signed Gentoo stage3 digest metadata"))?;

    let signature = work.join("gentoo-stage3.asc");
    let archive = work.join(&source.filename);
    download(runner, &source.signature_url, &signature, 64 * 1024)?;
    download(runner, &source.url, &archive, source.size)?;
    if fs::metadata(&archive)?.len() != source.size {
        return Err(InstallError::Invalid(
            "Gentoo stage3 size differs from selected release",
        ));
    }
    verify(
        runner,
        &[&signature, &archive],
        "gentoo-verify-stage3-signature",
        "Gentoo archive signature is not bound to the pinned release primary key",
    )?;
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
pub(super) const LIVE_TRANSPORT_CA: &str = "/configs/transport-ca.pem";
pub(super) const TARGET_TRANSPORT_CA: &str = "/etc/ssl/certs/rustaller-install.pem";
const TARGET_PACMAN_TRANSPORT: &str = "etc/pacman.rustaller-install.conf";
fn copy_live_transport(root: &Path, target: Target) -> Result<(), InstallError> {
    let source = Path::new(LIVE_TRANSPORT_CA);
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
        TARGET_PACMAN_TRANSPORT,
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
            if spec.args.get(1).is_some_and(|program| program == "pacman") {
                let config =
                    crate::shared::configure::read_config(&self.root.join("etc/pacman.conf"))
                        .map_err(io::Error::other)?;
                let config = rolling::arch_transport_config(&config, TARGET_TRANSPORT_CA)
                    .map_err(io::Error::other)?;
                write_target(self.root, TARGET_PACMAN_TRANSPORT, config.as_bytes(), 0o600)
                    .map_err(io::Error::other)?;
                spec.args.splice(
                    2..2,
                    [
                        "--config".into(),
                        format!("/{TARGET_PACMAN_TRANSPORT}").into(),
                    ],
                );
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
    // Portage uses a root-owned cache with group write enabled. The stage3
    // can ship it with group root, and Portage can assign its own group later.
    // Admit these modes only on this exact cache leaf.
    let portage_cache = relative == "var/cache/distfiles"
        && metadata.is_dir()
        && metadata.uid() == 0
        && matches!(metadata.gid(), 0 | 250)
        && metadata.mode() & 0o002 == 0;
    if portage_cache {
        crate::shared::configure::validate_planned_parent(&directory)?;
    } else {
        crate::shared::configure::validate_directory_tree(&directory)?;
    }
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
fn target_hostname(plan: &crate::model::InstallPlan) -> String {
    plan.post_install
        .hostname
        .clone()
        .unwrap_or_else(|| plan.target.to_string().to_ascii_lowercase())
}

fn write_target_hosts(root: &Path, hostname: &str) -> Result<(), InstallError> {
    // These entries belong to a freshly installed target.
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
    crate::shared::configure::write_config(&root.join(name), bytes, mode)
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
    #[test]
    fn rescue_keeps_access_packages_without_boot_compiler_requirements() {
        let mut plan = crate::model::InstallPlan::new(Target::Alpine);
        plan.hold = Some(1);
        let packages = live_packages(&plan).unwrap();
        assert!(packages.contains(&"openssh"));
        assert!(!packages.contains(&"gcc") && !packages.contains(&"musl-dev"));
        plan.hold = None;
        let packages = live_packages(&plan).unwrap();
        assert!(!packages.contains(&"gcc") && !packages.contains(&"musl-dev"));
    }

    use crate::command::{CommandOutput, MockRunner};
    use crate::config::{CongestionControl, NetworkConfig, QueueDiscipline};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn ssh_policy_facts(plan: &crate::model::InstallPlan) -> String {
        let native = key_only_ssh_config(plan, "", false);
        native
            .lines()
            .filter_map(|line| {
                let (key, value) = line.split_once(' ')?;
                (!key.starts_with('#')).then(|| format!("{} {value}\n", key.to_ascii_lowercase()))
            })
            .collect()
    }

    #[test]
    fn key_only_ssh_keeps_native_pam_and_forwarding_policy_but_ignores_dropins() {
        for (username, root_login) in [("root", "prohibit-password"), ("alice", "no")] {
            let mut plan = crate::model::InstallPlan::new(Target::Debian);
            plan.username = Some(username.into());
            plan.ssh_port = Some(2222);
            let config = key_only_ssh_config(
                &plan,
                "passwordauthentication yes\nusepam yes\nallowtcpforwarding no\n\
                 disableforwarding yes\ninclude sshd_config.d/test.conf\nmatch all\n\
                 subsystem sftp /usr/lib/openssh/sftp-server\n",
                false,
            );
            assert!(config.contains("Port 2222\n"));
            assert!(config.contains(&format!("AllowUsers {username}\n")));
            assert!(config.contains(&format!("PermitRootLogin {root_login}\n")));
            assert!(config.contains("PasswordAuthentication no\n"));
            assert!(config.contains("AuthenticationMethods publickey\n"));
            assert!(config.contains("usepam yes\n"));
            assert!(config.contains("allowtcpforwarding no\n"));
            assert!(config.contains("disableforwarding yes\n"));
            assert!(config.contains("subsystem sftp /usr/lib/openssh/sftp-server\n"));
            assert!(!config.contains("sshd_config.d"));
            assert!(!config.contains("match all"));
            let facts = ssh_policy_facts(&plan);
            assert!(valid_effective_ssh_policy(&plan, &facts));
            for (previous, altered) in [
                ("passwordauthentication no", "passwordauthentication yes"),
                (
                    "kbdinteractiveauthentication no",
                    "kbdinteractiveauthentication yes",
                ),
                (
                    "authenticationmethods publickey",
                    "authenticationmethods any",
                ),
                ("port 2222", "port 2222\nport 22"),
            ] {
                assert!(!valid_effective_ssh_policy(
                    &plan,
                    &facts.replace(previous, altered)
                ));
            }
        }
    }

    #[test]
    fn non_root_provisioning_disables_passwords_and_verifies_effective_sudo() {
        let root = Scratch::new();
        fs::create_dir_all(root.0.join("home/alice")).unwrap();
        let mut plan = crate::model::InstallPlan::new(Target::Debian);
        plan.username = Some("alice".into());
        plan.ssh_keys.push(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f alice".into(),
        );
        let effective = ssh_policy_facts(&plan);
        let mut runner = MockRunner::new(
            ["", "", "", "0\n", "", "", "usepam yes\n", "", &effective]
                .map(|stdout| CommandOutput::new(Some(0), stdout.as_bytes().to_vec(), vec![])),
        );
        configure_key_only_ssh(&mut runner, &root.0, &plan, false).unwrap();
        let calls = runner.calls();
        assert_eq!(calls[2].phase, "ssh-disable-account-passwords");
        assert_eq!(
            calls[2].stdin,
            CommandInput::Bytes(b"root:NP\nalice:NP\n".to_vec())
        );
        assert_eq!(calls[3].phase, "ssh-verify-sudo-account");
        for (path, mode) in [
            ("home/alice", 0o700),
            ("home/alice/.ssh", 0o700),
            ("home/alice/.ssh/authorized_keys", 0o600),
            ("etc/sudoers.d/90-rustaller", 0o440),
            ("etc/ssh/sshd_config", 0o600),
        ] {
            assert_eq!(
                fs::metadata(root.0.join(path)).unwrap().mode() & 0o777,
                mode
            );
        }
        assert_eq!(
            fs::read_to_string(root.0.join("etc/sudoers.d/90-rustaller")).unwrap(),
            "alice ALL=(ALL:ALL) NOPASSWD: ALL\n",
        );
    }

    #[test]
    fn missing_and_unresolved_keys_stop_before_account_mutations() {
        let root = Scratch::new();
        let mut plan = crate::model::InstallPlan::new(Target::Debian);
        plan.username = Some("alice".into());
        let mut runner = MockRunner::empty();
        assert!(configure_key_only_ssh(&mut runner, &root.0, &plan, false).is_err());
        plan.ssh_keys.push("/client.pub".into());
        assert!(configure_key_only_ssh(&mut runner, &root.0, &plan, false).is_err());
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
    fn portage_cache_cleanup_still_refuses_symlinks_and_world_writable_leaves() {
        const CACHE: &str = "var/cache/distfiles";
        let root = Scratch::new();
        let outside = Scratch::new();
        outside.write("keep", b"outside the installer root");
        let cache = root.0.join(CACHE);
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        symlink(&outside.0, &cache).unwrap();
        assert!(clean_target_tree(&root.0, CACHE, false).is_err());
        assert!(outside.0.join("keep").is_file());

        fs::remove_file(&cache).unwrap();
        root.write("var/cache/distfiles/archive", b"package cache");
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(clean_target_tree(&root.0, CACHE, false).is_err());
        assert!(cache.join("archive").is_file());
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
            (Target::Gentoo, FrpLaunch::Musl),
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
            install_frpc(
                &scratch.0,
                &crate::model::InstallPlan::new(target),
                &payload,
            )
            .unwrap();
            let service = fs::read_to_string(scratch.0.join(
                if matches!(target, Target::Alpine | Target::Gentoo) {
                    "etc/init.d/rustaller-frpc"
                } else {
                    "etc/systemd/system/rustaller-frpc.service"
                },
            ))
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
    fn gentoo_enables_native_openrc_services() {
        let manifest = DeploymentManifest {
            disk: crate::deployment::DiskIdentity {
                device: "/dev/vda".into(),
                size_bytes: 8 * 1024u64.pow(3),
                serial: "fixture".into(),
                wwn: String::new(),
                partition_table_id: String::new(),
            },
            boot_mode: crate::model::BootMode::Bios,
            alpine_branch: "v3.24".into(),
            debian_codename: String::new(),
            gentoo_stage3: None,
            network: vec![crate::deployment::NetworkInterface {
                name: "ens3".into(),
                mac: "52:54:00:11:22:33".into(),
                ipv4_addresses: vec![],
                ipv4_gateway: None,
                ipv6_addresses: vec![],
                ipv6_gateway: None,
                dhcp4: true,
                dhcp6: false,
                accept_ra: false,
            }],
        };
        let scratch = Scratch::new();
        scratch.write("etc/init.d/net.lo", b"#!/sbin/openrc-run\n");
        fs::set_permissions(
            scratch.0.join("etc/init.d/net.lo"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let plan = crate::model::InstallPlan::new(Target::Gentoo);
        let mut runner = crate::command::MockRunner::new(
            (0..16).map(|_| crate::command::CommandOutput::new(Some(0), vec![], vec![])),
        );
        enable_services(&mut runner, &scratch.0, &plan, &manifest).unwrap();
        assert!(
            runner
                .calls()
                .iter()
                .all(|call| call.args.iter().any(|arg| arg == "rc-update"))
        );
        assert_eq!(
            fs::read_link(scratch.0.join("etc/init.d/net.ens3")).unwrap(),
            Path::new("net.lo")
        );
        assert!(
            runner
                .calls()
                .iter()
                .any(|call| call.args.iter().any(|arg| arg == "sshd"))
        );
        assert_eq!(
            fs::read_to_string(scratch.0.join("etc/conf.d/hostname")).unwrap(),
            "hostname=\"gentoo\"\n"
        );
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
