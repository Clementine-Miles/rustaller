//! Native rolling-release adapters used only by the validated Alpine Live driver.
//!
//! APK verifies the Arch bootstrap tooling/keyring against Alpine's installed
//! signing keys. pacman then keeps its own signature validation enabled. Gentoo
//! extraction requires the parent's OpenPGP-and-digest-verified stage3 token;
//! Portage verifies repository and binary-package signatures independently.
use super::install::VerifiedStage3;
use crate::command::{CommandError, CommandRunner, CommandSpec, OutputMode, run_checked};
use crate::config::GentooInit;
use crate::host::sources::{ALPINE_ARCHIVE, ARCH_MIRROR};
use crate::model::{BootMode, PlanError, Target};
use crate::shared::configure::{ConfigurationError, validate_root};
use crate::shared::configure::{read_config, replace_managed_block, validate_planned_file};
use crate::targets::packages;
use crate::targets::packages::arch_policy;
use crate::worker::live::ValidatedLivePlan;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path};

#[derive(Debug)]
pub enum RollingError {
    InvalidRoute,
    InvalidInput(&'static str),
    MissingGentooSigningKeys,
    MissingNomultilibProfile,
    Plan(PlanError),
    Configuration(ConfigurationError),
    Command(CommandError),
    Filesystem(io::Error),
}
impl fmt::Display for RollingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoute => {
                f.write_str("Rolling-release adapter requires the matching automatic Live plan")
            }
            Self::InvalidInput(field) => {
                write!(f, "Invalid rolling-release installation input: {field}")
            }
            Self::MissingGentooSigningKeys => f.write_str(
                "Verified Gentoo stage3 does not contain the native release signing-key package",
            ),
            Self::MissingNomultilibProfile => f.write_str(
                "Gentoo repository contains no matching stable amd64 no-multilib init profile",
            ),
            Self::Plan(e) => e.fmt(f),
            Self::Configuration(e) => e.fmt(f),
            Self::Command(e) => e.fmt(f),
            Self::Filesystem(e) => write!(f, "Cannot prepare rolling-release target: {e}"),
        }
    }
}
impl std::error::Error for RollingError {}
impl From<PlanError> for RollingError {
    fn from(e: PlanError) -> Self {
        Self::Plan(e)
    }
}
impl From<ConfigurationError> for RollingError {
    fn from(e: ConfigurationError) -> Self {
        Self::Configuration(e)
    }
}
impl From<CommandError> for RollingError {
    fn from(e: CommandError) -> Self {
        Self::Command(e)
    }
}
impl From<io::Error> for RollingError {
    fn from(e: io::Error) -> Self {
        Self::Filesystem(e)
    }
}

/// Install Arch using native Alpine packages and the official Arch keyring.
/// The Live driver must establish /etc/apk/keys from the verified Alpine image
/// and official repositories before this call. `pacman_config` is a caller-owned
/// work file outside the mounted target, never a path into the source system.
pub(super) fn bootstrap_arch_rootfs<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
    boot: BootMode,
    firmware: &[String],
    alpine_branch: &str,
    pacman_config: &Path,
) -> Result<(), RollingError> {
    validate_route(root, plan, Target::Arch)?;
    if !crate::deployment::valid_branch(alpine_branch) {
        return Err(RollingError::InvalidInput("Alpine stable branch"));
    }
    validate_work_file(pacman_config)?;
    if pacman_config.starts_with(root) {
        return Err(RollingError::InvalidInput(
            "Arch Live pacman configuration must be outside the target",
        ));
    }
    validate_packages(firmware)?;
    let config = arch_configuration(plan.plan().system.strip_docs())?;
    let live_config = if Path::new(super::install::LIVE_TRANSPORT_CA).is_file() {
        arch_transport_config(&config, super::install::LIVE_TRANSPORT_CA)?
    } else {
        config.clone()
    };
    write_file(pacman_config, live_config.as_bytes())?;

    // All packages, including Arch's trust root, are authenticated by APK. There
    // is no --allow-untrusted, unauthenticated bootstrap key or keyserver trust.
    let community = format!("{ALPINE_ARCHIVE}/{alpine_branch}/community");
    checked(
        runner,
        "apk",
        [
            "add",
            "--no-cache",
            "--repository",
            &community,
            "arch-install-scripts",
            "archlinux-keyring",
            "curl",
            "gnupg",
        ],
        "arch-live-signed-tools",
    )?;
    checked(runner, "pacman-key", ["--init"], "arch-live-keyring-init")?;
    checked(
        runner,
        "pacman-key",
        ["--populate", "archlinux"],
        "arch-live-keyring-populate",
    )?;

    let mut packages = packages::pacstrap_packages(
        boot,
        plan.plan().system.root_filesystem.as_str(),
        plan.plan().username.as_deref().unwrap_or("root"),
        firmware,
    );
    for package in ["archlinux-keyring", "ca-certificates", "gnupg"] {
        if !packages.iter().any(|old| old == package) {
            packages.push(package.into());
        }
    }
    let mut args = vec![
        OsStr::new("-K").to_owned(),
        // Alpine does not necessarily ship Arch's mirrorlist. This adapter
        // supplies explicit official repository URLs in the checked config.
        "-M".into(),
        "-C".into(),
        pacman_config.as_os_str().to_owned(),
        root.as_os_str().to_owned(),
    ];
    args.extend(packages.iter().map(Into::into));
    let spec = command("pacstrap", args, "arch-bootstrap-rootfs");
    for attempt in 1..=5 {
        let result = run_checked(runner, &spec);
        if result.is_ok() {
            break;
        }
        if attempt == 5 {
            result?;
        }
        checked(runner, "sleep", ["5"], "arch-bootstrap-retry")?;
    }
    write_target(root, "etc/pacman.conf", config.as_bytes())?;
    Ok(())
}

/// Stop only the agents belonging to the target pacman keyring. pacstrap can
/// launch an agent from Live with its homedir under the target, while later
/// chroot commands can launch a target-local agent. Both can pin the filesystem.
pub(super) fn stop_arch_keyring_daemons<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
) -> Result<(), RollingError> {
    validate_route(root, plan, Target::Arch)?;
    chroot_checked(
        runner,
        root,
        [
            "gpgconf",
            "--homedir",
            "/etc/pacman.d/gnupg",
            "--kill",
            "all",
        ],
        "arch-target-keyring-stop",
    )?;
    let home = root.join("etc/pacman.d/gnupg");
    checked(
        runner,
        "gpgconf",
        [
            OsStr::new("--homedir"),
            home.as_os_str(),
            OsStr::new("--kill"),
            OsStr::new("all"),
        ],
        "arch-live-target-keyring-stop",
    )
}

/// Run after the driver mounts /proc,/sys,/dev,/run inside the new root.
/// pacstrap owns and tears down its own temporary API mounts; kernel package
/// hooks need the driver's persistent mounts for the following phase.
pub(super) fn configure_arch<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
) -> Result<(), RollingError> {
    validate_route(root, plan, Target::Arch)?;
    // pacstrap -K initializes the new target trust database; refresh explicitly
    // from its signed archlinux-keyring package before any subsequent install.
    chroot_checked(
        runner,
        root,
        ["pacman-key", "--populate", "archlinux"],
        "arch-target-keyring",
    )?;
    write_target(root, "etc/locale.gen", b"en_US.UTF-8 UTF-8\n")?;
    write_target(root, "etc/locale.conf", b"LANG=C.UTF-8\n")?;
    chroot_checked(runner, root, ["locale-gen"], "arch-target-locale")?;
    packages::verify_kernel_available(runner, root, plan.plan()).map_err(|_| {
        RollingError::InvalidInput("selected kernel package or dependencies unavailable")
    })?;
    run_checked(
        runner,
        &packages::kernel_install_command_for_plan(plan.plan(), root)?.env("LANG", "C.UTF-8"),
    )?;
    // The native kernel package's hooks normally do this; the explicit rebuild
    // makes success depend on an actual initramfs, including the selected LTS.
    if plan.plan().system.root_filesystem == crate::config::RootFilesystem::Btrfs {
        write_target(
            root,
            "etc/mkinitcpio.conf.d/90-rustaller-root.conf",
            b"MODULES+=(btrfs)\n",
        )?;
    }
    chroot_checked(
        runner,
        root,
        ["mkinitcpio", "-P", "-S", "autodetect"],
        "arch-target-initramfs",
    )?;
    if plan.plan().post_install.kernel == crate::postinstall::KernelChoice::Cachyos {
        super::cachyos::install_firstboot_update(root)
            .map_err(|_| RollingError::InvalidInput("CachyOS first-boot update service"))?;
    }
    Ok(())
}

/// Extract before mounting target /proc,/sys,/dev,/run. The token is constructible
/// only by the parent's successful signed-digest/payload verification stage.
pub(super) fn extract_gentoo<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    stage3: &VerifiedStage3,
) -> Result<(), RollingError> {
    validate_root(root)?;
    let metadata = fs::symlink_metadata(stage3.path())?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(RollingError::InvalidInput("verified Gentoo stage3 file"));
    }
    checked(
        runner,
        "apk",
        ["add", "--no-cache", "tar", "xz"],
        "gentoo-live-extract-tools",
    )?;
    checked(
        runner,
        "tar",
        [
            OsStr::new("--extract"),
            "--xz".as_ref(),
            "--numeric-owner".as_ref(),
            "--same-owner".as_ref(),
            "--xattrs".as_ref(),
            "--xattrs-include=*.*".as_ref(),
            "--delay-directory-restore".as_ref(),
            "--file".as_ref(),
            stage3.path().as_os_str(),
            "--directory".as_ref(),
            root.as_os_str(),
        ],
        "gentoo-extract-verified-stage3",
    )
}

/// Configure the signed no-multilib stage3 after pseudo-filesystems and resolver are
/// mounted/provided by the Live driver. Keep Portage/Python/toolchain intact.
pub(super) fn configure_gentoo<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    plan: &ValidatedLivePlan,
    boot: BootMode,
    firmware: &[String],
    root_uuid: &str,
) -> Result<(), RollingError> {
    validate_route(root, plan, Target::Gentoo)?;
    // Gentoo's XFS/ext4 root requires a canonical long UUID.
    if !crate::shared::storage::valid_uuid(root_uuid)
        || root_uuid.len() != 36
        || root_uuid
            .bytes()
            .enumerate()
            .any(|(i, b)| [8, 13, 18, 23].contains(&i) != (b == b'-'))
    {
        return Err(RollingError::InvalidInput("root filesystem UUID"));
    }
    validate_packages(firmware)?;
    let release_keys = root.join("usr/share/openpgp-keys/gentoo-release.asc");
    validate_planned_file(&release_keys)?;
    if fs::symlink_metadata(&release_keys).is_err() {
        return Err(RollingError::MissingGentooSigningKeys);
    }
    write_target(root, "etc/portage/repos.conf/gentoo.conf", b"[gentoo]\nlocation = /var/db/repos/gentoo\nsync-type = webrsync\nsync-uri = https://distfiles.gentoo.org/snapshots\nsync-webrsync-verify-signature = true\nsync-openpgp-key-path = /usr/share/openpgp-keys/gentoo-release.asc\nsync-openpgp-key-refresh = false\n")?;
    let make_conf = root.join("etc/portage/make.conf");
    validate_planned_file(&make_conf)?;
    let init = plan
        .plan()
        .system
        .gentoo_init
        .ok_or(RollingError::InvalidInput("Gentoo init system"))?;
    let extra = "ABI_X86=\"64\"\nMAKEOPTS=\"-j1\"\nGENTOO_MIRRORS=\"https://distfiles.gentoo.org\"\nFEATURES=\"${FEATURES} getbinpkg binpkg-request-signature\"\n".to_owned();
    let text = replace_managed_block(
        &read_config(&make_conf)?,
        &extra,
        "# BEGIN rustaller Gentoo installer",
        "# END rustaller Gentoo installer",
        false,
    )?;
    write_file(&make_conf, text.as_bytes())?;
    write_target(
        root,
        "etc/portage/package.use/rustaller-installkernel",
        b"sys-kernel/installkernel dracut -systemd -systemd-boot -uki -ukify -efistub -grub -refind\n",
    )?;
    // Both PID 1 variants use the legacy installkernel backend: it installs
    // versioned regular files. Disable kernel-install layouts and UKIs explicitly;
    // static symlinks cannot be created on the FAT32 /boot filesystem.
    write_target(
        root,
        "etc/kernel/install.conf",
        b"layout=compat\ninitrd_generator=dracut\nuki_generator=none\n",
    )?;
    write_target(
        root,
        "etc/kernel/install.d/90-zz-update-static.install",
        b"#!/bin/sh\n# FAT32 /boot has no static symlinks; versioned regular files only.\nexit 0\n",
    )?;
    fs::set_permissions(
        root.join("etc/kernel/install.d/90-zz-update-static.install"),
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )?;
    let btrfs = plan.plan().system.root_filesystem == crate::config::RootFilesystem::Btrfs;
    let flags = if btrfs { " rootflags=subvol=@" } else { "" };
    let drivers = if btrfs {
        "add_drivers+=\" btrfs \"\n"
    } else {
        ""
    };
    write_target(
        root,
        "etc/dracut.conf.d/00-rustaller-root.conf",
        format!("hostonly=\"no\"\nkernel_cmdline=\" root=UUID={root_uuid}{flags} \"\n{drivers}")
            .as_bytes(),
    )?;
    write_target(root, "etc/locale.gen", b"en_US.UTF-8 UTF-8\n")?;
    write_target(root, "etc/locale.conf", b"LANG=C.UTF-8\n")?;
    // getuto uses the trust configuration shipped in the authenticated stage3.
    // TERM=dumb avoids inheriting the Live OpenRC helper context.
    chroot_checked(runner, root, ["getuto"], "gentoo-native-binary-trust")?;
    chroot_checked(
        runner,
        root,
        ["emerge-webrsync"],
        "gentoo-signed-repository",
    )?;
    let profiles = chroot_capture(
        runner,
        root,
        ["eselect", "profile", "list"],
        "gentoo-list-nomultilib-profiles",
    )?;
    let profile =
        select_nomultilib_profile(&profiles, init).ok_or(RollingError::MissingNomultilibProfile)?;
    chroot_checked(
        runner,
        root,
        ["eselect", "profile", "set", &profile],
        "gentoo-select-nomultilib-profile",
    )?;
    let selected = chroot_capture(
        runner,
        root,
        ["eselect", "profile", "show"],
        "gentoo-verify-nomultilib-profile",
    )?;
    if !selected.split_whitespace().any(|name| name == profile) {
        return Err(RollingError::MissingNomultilibProfile);
    }
    let abi = chroot_capture(
        runner,
        root,
        ["portageq", "envvar", "ABI_X86"],
        "gentoo-verify-64-bit-abi",
    )?;
    if abi.trim() != "64" {
        return Err(RollingError::InvalidInput(
            "Gentoo no-multilib ABI_X86 must be 64",
        ));
    }
    chroot_checked(runner, root, ["locale-gen"], "gentoo-target-locale")?;
    chroot_checked(runner, root, ["env-update"], "gentoo-target-environment")?;
    let mut packages = packages::emerge_package_set_for_plan(
        plan.plan(),
        boot,
        plan.plan().system.root_filesystem.as_str(),
        firmware,
    )?;
    for atom in [
        "sys-kernel/dracut",
        "sys-kernel/installkernel",
        "net-misc/openssh",
        "app-misc/ca-certificates",
    ] {
        if !packages.iter().any(|old| old == atom) {
            packages.push(atom.into());
        }
    }
    packages::verify_kernel_available(runner, root, plan.plan()).map_err(|_| {
        RollingError::InvalidInput("selected kernel package or dependencies unavailable")
    })?;
    run_checked(runner, &{
        let mut args = vec![
            root.as_os_str().to_owned(),
            "emerge".into(),
            "--getbinpkg".into(),
            "--usepkg".into(),
            "--noreplace".into(),
        ];
        args.extend(packages.into_iter().map(Into::into));
        command("chroot", args, "gentoo-signed-packages-and-kernel")
    })?;
    chroot_checked(
        runner,
        root,
        ["dracut", "--regenerate-all", "--force", "--no-hostonly"],
        "gentoo-target-initramfs",
    )
}

fn arch_configuration(strip_docs: bool) -> Result<String, ConfigurationError> {
    arch_policy(
        &format!(
            "[options]\nArchitecture = x86_64\nGPGDir = /etc/pacman.d/gnupg\nLocalFileSigLevel = Required\nParallelDownloads = 5\n[core]\nServer = {ARCH_MIRROR}/$repo/os/$arch\n[extra]\nServer = {ARCH_MIRROR}/$repo/os/$arch\n"
        ),
        strip_docs,
    )
}

// libalpm's internal curl downloader does not honor curl's CA environment.
// The explicit downloader only applies to the installer transport, while
// libalpm continues verifying packages against the native signing keyring.
pub(super) fn arch_transport_config(config: &str, ca: &str) -> Result<String, ConfigurationError> {
    if !matches!(
        ca,
        super::install::LIVE_TRANSPORT_CA | super::install::TARGET_TRANSPORT_CA
    ) || !config.contains("[options]\n")
        || config.lines().any(|line| {
            line.split_once('=')
                .is_some_and(|(name, _)| name.trim() == "XferCommand")
        })
    {
        return Err(ConfigurationError::InvalidConfigFile);
    }
    Ok(config.replacen(
        "[options]\n",
        &format!("[options]\nXferCommand = /usr/bin/curl --disable --fail --silent --show-error --location --proto =https --proto-redir =https --connect-timeout 15 --speed-limit 1 --speed-time 30 --max-time 3600 --cacert {ca} --output %o -- %u\n"),
        1,
    ))
}
fn validate_route(
    root: &Path,
    plan: &ValidatedLivePlan,
    target: Target,
) -> Result<(), RollingError> {
    validate_root(root)?;
    plan.plan().validate()?;
    if plan.plan().target != target || plan.plan().hold == Some(1) {
        return Err(RollingError::InvalidRoute);
    }
    Ok(())
}
fn validate_work_file(path: &Path) -> Result<(), RollingError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|p| matches!(p, Component::CurDir | Component::ParentDir))
    {
        return Err(RollingError::InvalidInput(
            "private work configuration path",
        ));
    }
    validate_planned_file(path)?;
    Ok(())
}
fn validate_packages(packages: &[String]) -> Result<(), RollingError> {
    if packages.len() > 256
        || packages.iter().any(|s| {
            s.len() > 256
                || !s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+_.:/=@-".contains(&b))
        })
    {
        return Err(RollingError::InvalidInput("firmware package selection"));
    }
    Ok(())
}
fn select_nomultilib_profile(output: &str, init: GentooInit) -> Option<String> {
    let candidates = output
        .lines()
        .filter(|line| line.contains("(stable)"))
        .filter_map(|line| line.split_once(']').map(|(_, rest)| rest))
        .filter_map(|rest| {
            rest.split_whitespace()
                .next()
                .map(|name| (name, rest.split_whitespace().any(|word| word == "*")))
        })
        .filter(|(name, _)| {
            name.starts_with("default/linux/amd64/")
                && name.split('/').any(|part| part == "no-multilib")
                && name.split('/').any(|part| part == "systemd") == (init == GentooInit::Systemd)
                && name.split('/').all(|part| {
                    !part.is_empty()
                        && part != "."
                        && part != ".."
                        && part
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                })
        })
        .collect::<Vec<_>>();
    // Preserve the selected matching stable profile in the authenticated stage3
    // rather than replacing its ABI/merged-usr selection with a shorter name.
    candidates
        .iter()
        .find(|(_, selected)| *selected)
        .or_else(|| {
            candidates
                .iter()
                .min_by(|(a, _), (b, _)| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
        })
        .map(|(name, _)| (*name).to_owned())
}
fn write_target(root: &Path, name: &str, bytes: &[u8]) -> Result<(), RollingError> {
    write_file(&root.join(name), bytes)
}
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), RollingError> {
    crate::shared::configure::write_config(path, bytes, 0o644)
}
fn command<I, S>(program: &str, args: I, phase: &str) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    crate::command::target_command(program, args, phase)
        .env("LANG", "C.UTF-8")
        .env("TERM", "dumb")
}
fn checked<R, I, S>(runner: &mut R, program: &str, args: I, phase: &str) -> Result<(), RollingError>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_checked(runner, &command(program, args, phase))?;
    Ok(())
}
fn chroot_command<I, S>(root: &Path, args: I, phase: &str) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut out = vec![root.as_os_str().to_owned()];
    out.extend(args.into_iter().map(|s| s.as_ref().to_owned()));
    command("chroot", out, phase)
}
fn chroot_checked<R, I, S>(
    runner: &mut R,
    root: &Path,
    args: I,
    phase: &str,
) -> Result<(), RollingError>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_checked(runner, &chroot_command(root, args, phase))?;
    Ok(())
}
fn chroot_capture<R, I, S>(
    runner: &mut R,
    root: &Path,
    args: I,
    phase: &str,
) -> Result<String, RollingError>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let command = chroot_command(root, args, phase)
        .env("LC_ALL", "C")
        .stdout_mode(OutputMode::Capture);
    let bytes = crate::command::capture_checked(runner, &command)?;
    String::from_utf8(bytes)
        .map_err(|_| RollingError::InvalidInput("native profile output encoding"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandOutput, MockRunner};
    use crate::model::InstallPlan;
    use crate::worker::live::validate_live_plan_with_uid;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "rustaller-rolling-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::create_dir(path.join("root")).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn arch_plan() -> ValidatedLivePlan {
        let plan = InstallPlan::new(Target::Arch);
        validate_live_plan_with_uid(plan, "x86_64", 1024, crate::RUNTIME_VERSION, 0).unwrap()
    }
    fn successful_calls(count: usize) -> MockRunner {
        MockRunner::new((0..count).map(|_| CommandOutput::new(Some(0), vec![], vec![])))
    }
    #[test]
    fn arch_keyring_shutdown_is_scoped_to_both_target_views() {
        let scratch = Scratch::new();
        let root = scratch.0.join("root");
        let mut runner = successful_calls(2);
        stop_arch_keyring_daemons(&mut runner, &root, &arch_plan()).unwrap();
        assert_eq!(runner.calls()[0].program, "chroot");
        assert_eq!(
            runner.calls()[0].args,
            [
                root.as_os_str(),
                OsStr::new("gpgconf"),
                OsStr::new("--homedir"),
                OsStr::new("/etc/pacman.d/gnupg"),
                OsStr::new("--kill"),
                OsStr::new("all")
            ]
        );
        assert_eq!(runner.calls()[1].program, "gpgconf");
        assert_eq!(
            runner.calls()[1].args[1],
            root.join("etc/pacman.d/gnupg").as_os_str()
        );
        assert_eq!(runner.calls()[1].args[2..], ["--kill", "all"]);
    }

    #[test]
    fn arch_configuration_never_disables_package_signatures() {
        for minimal in [false, true] {
            let config = arch_configuration(minimal).unwrap();
            assert!(config.contains("SigLevel = Required DatabaseOptional"));
            assert!(!config.contains("Never"));
            assert!(config.contains("LocalFileSigLevel = Required"));
            assert_eq!(config.contains("NoExtract"), minimal);
            assert!(config.contains("https://geo.mirror.pkgbuild.com/$repo/os/$arch"));
        }
    }
    #[test]
    fn arch_private_transport_uses_https_and_retains_package_verification() {
        let native = arch_configuration(false).unwrap();
        assert!(!native.contains("XferCommand"));
        for ca in [
            super::super::install::LIVE_TRANSPORT_CA,
            super::super::install::TARGET_TRANSPORT_CA,
        ] {
            let temporary = arch_transport_config(&native, ca).unwrap();
            assert!(temporary.contains(&format!("--cacert {ca} --output %o -- %u")));
            assert!(temporary.contains("--proto =https --proto-redir =https"));
            assert!(temporary.contains("SigLevel = Required DatabaseOptional"));
            assert!(arch_transport_config(&temporary, ca).is_err());
        }
    }
    #[test]
    fn stable_profile_selection_requires_nomultilib_and_exact_init() {
        let text = "Available profile symlink targets:\n [1] default/linux/amd64/23.0 (stable)\n [2] default/linux/amd64/23.0/systemd (stable) *\n [3] default/linux/amd64/23.0/no-multilib (stable)\n [4] default/linux/amd64/23.0/no-multilib/systemd (stable)\n [5] default/linux/arm64/23.0/no-multilib (stable)\n [6] default/linux/amd64/experimental/no-multilib/systemd (exp)\n";
        assert_eq!(
            select_nomultilib_profile(text, GentooInit::Openrc).as_deref(),
            Some("default/linux/amd64/23.0/no-multilib")
        );
        assert_eq!(
            select_nomultilib_profile(text, GentooInit::Systemd).as_deref(),
            Some("default/linux/amd64/23.0/no-multilib/systemd")
        );
        assert!(select_nomultilib_profile("[1] default/linux/amd64/23.0 (stable)\n[2] default/linux/amd64/23.0/systemd (stable)\n", GentooInit::Openrc).is_none());
        assert!(
            select_nomultilib_profile(
                "[1] default/linux/amd64/23.0/no-multilib (stable)\n",
                GentooInit::Systemd
            )
            .is_none()
        );
        assert_eq!(select_nomultilib_profile("[1] default/linux/amd64/23.0/no-multilib (stable)\n[2] default/linux/amd64/23.0/split-usr/no-multilib (stable) *\n", GentooInit::Openrc).as_deref(), Some("default/linux/amd64/23.0/split-usr/no-multilib"));
    }
    #[test]
    fn arch_rootfs_uses_signed_native_keyring_and_does_not_copy_a_missing_live_mirrorlist() {
        let scratch = Scratch::new();
        let root = scratch.0.join("root");
        let config = scratch.0.join("pacman.conf");
        let plan = arch_plan();
        let mut runner = successful_calls(4);
        bootstrap_arch_rootfs(
            &mut runner,
            &root,
            &plan,
            BootMode::Bios,
            &[],
            "v3.24",
            &config,
        )
        .unwrap();
        assert_eq!(
            runner
                .calls()
                .iter()
                .map(|spec| spec.program.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["apk", "pacman-key", "pacman-key", "pacstrap"]
        );
        let args = &runner.calls()[3].args;
        assert!(args.iter().any(|arg| arg == "-K"));
        assert!(args.iter().any(|arg| arg == "-M"));
        assert!(args.iter().any(|arg| arg == "archlinux-keyring"));
        assert!(
            fs::read_to_string(config)
                .unwrap()
                .contains("SigLevel = Required DatabaseOptional")
        );
        assert!(!runner.calls().iter().any(|spec| {
            spec.args
                .iter()
                .any(|arg| arg == "--allow-untrusted" || arg == "--nosignature")
        }));
    }
    #[test]
    fn arch_kernel_hooks_are_separate_from_rootfs_bootstrap() {
        let scratch = Scratch::new();
        let root = scratch.0.join("root");
        let mut runner = successful_calls(5);
        configure_arch(&mut runner, &root, &arch_plan()).unwrap();
        assert_eq!(
            runner
                .calls()
                .iter()
                .map(|spec| spec.args[1].to_str().unwrap())
                .collect::<Vec<_>>(),
            ["pacman-key", "locale-gen", "pacman", "pacman", "mkinitcpio"]
        );
        assert!(runner.calls()[2].args.iter().any(|arg| arg == "-Syp"));
        assert!(runner.calls()[2].args.iter().any(|arg| arg == "linux"));
        assert!(runner.calls()[3].args.iter().any(|arg| arg == "linux"));
        assert!(runner.calls()[4].args.iter().any(|arg| arg == "-P"));
    }
    #[test]
    fn both_gentoo_variants_enforce_nomultilib_profile_abi_and_fat_kernel_policy() {
        for init in [GentooInit::Openrc, GentooInit::Systemd] {
            let scratch = Scratch::new();
            let root = scratch.0.join("root");
            let key = root.join("usr/share/openpgp-keys/gentoo-release.asc");
            fs::create_dir_all(key.parent().unwrap()).unwrap();
            fs::write(key, b"authenticated stage3 fixture").unwrap();
            let mut p = InstallPlan::new(Target::Gentoo);
            p.system.gentoo_init = Some(init);
            let plan =
                validate_live_plan_with_uid(p, "x86_64", 4096, crate::RUNTIME_VERSION, 0).unwrap();
            let profile = format!(
                "default/linux/amd64/23.0/no-multilib{}",
                if init == GentooInit::Systemd {
                    "/systemd"
                } else {
                    ""
                }
            );
            let mut runner = MockRunner::new((0..11).map(|index| {
                let text = match index {
                    2 => format!("[1] {profile} (stable) *\n"),
                    4 => format!("Current /etc/portage/make.profile symlink:\n  {profile}\n"),
                    5 => "64\n".into(),
                    _ => String::new(),
                };
                CommandOutput::new(Some(0), text.into_bytes(), vec![])
            }));
            configure_gentoo(
                &mut runner,
                &root,
                &plan,
                BootMode::Bios,
                &[],
                "00112233-4455-6677-8899-aabbccddeeff",
            )
            .unwrap();
            assert!(
                runner.calls()[3]
                    .args
                    .iter()
                    .any(|arg| arg == profile.as_str())
            );
            assert!(runner.calls()[8].args.iter().any(|arg| arg == "--pretend"));
            assert!(
                runner.calls()[10]
                    .args
                    .iter()
                    .any(|arg| arg == "--regenerate-all")
            );
            assert!(
                fs::read_to_string(root.join("etc/portage/make.conf"))
                    .unwrap()
                    .contains("ABI_X86=\"64\"")
            );
            let policy =
                fs::read_to_string(root.join("etc/portage/package.use/rustaller-installkernel"))
                    .unwrap();
            assert!(policy.contains("-systemd -systemd-boot -uki -ukify -efistub -grub"));
            assert_eq!(
                fs::read_to_string(root.join("etc/kernel/install.conf")).unwrap(),
                "layout=compat\ninitrd_generator=dracut\nuki_generator=none\n"
            );
            let static_hook = root.join("etc/kernel/install.d/90-zz-update-static.install");
            assert!(fs::symlink_metadata(&static_hook).unwrap().is_file());
            assert!(
                std::os::unix::fs::PermissionsExt::mode(
                    &fs::metadata(static_hook).unwrap().permissions()
                ) & 0o111
                    != 0
            );
        }
    }
}
