//! Native rolling-release adapters used only by the validated Alpine Live driver.
//!
//! APK verifies the Arch bootstrap tooling/keyring against Alpine's installed
//! signing keys. pacman then keeps its own signature validation enabled. Gentoo
//! extraction requires the parent's OpenPGP-and-digest-verified stage3 token;
//! Portage verifies repository and binary-package signatures independently.
use super::install::VerifiedStage3;
use crate::command::{CommandError, CommandRunner, CommandSpec, OutputMode, run_checked};
use crate::host::sources::{ALPINE_ARCHIVE, ARCH_MIRROR};
use crate::model::{BootMode, PlanError, Target};
use crate::shared::configure::{ConfigurationError, validate_root};
use crate::shared::dns::{read_config, replace_managed_block, validate_planned_file};
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
    MissingSystemdProfile,
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
            Self::MissingSystemdProfile => {
                f.write_str("Gentoo repository contains no stable amd64 systemd profile")
            }
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
    if !valid_alpine_branch(alpine_branch) {
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
    write_file(pacman_config, config.as_bytes())?;

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
    run_checked(
        runner,
        &packages::kernel_install_command_for_plan(plan.plan(), root)?.env("LANG", "C.UTF-8"),
    )?;
    // The native kernel package's hooks normally do this; the explicit rebuild
    // makes success depend on an actual initramfs, including the selected LTS.
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

/// Configure the signed systemd stage3 after pseudo-filesystems and resolver are
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
    if !valid_uuid(root_uuid) {
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
    let extra = format!(
        "GRUB_PLATFORMS=\"{}\"\nMAKEOPTS=\"-j1\"\nGENTOO_MIRRORS=\"https://distfiles.gentoo.org\"\nFEATURES=\"${{FEATURES}} getbinpkg binpkg-request-signature\"\n",
        packages::grub_platforms(boot)
    );
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
        b"sys-kernel/installkernel dracut grub\n",
    )?;
    write_target(
        root,
        "etc/dracut.conf.d/00-rustaller-root.conf",
        format!("hostonly=\"no\"\nkernel_cmdline=\" root=UUID={root_uuid} \"\n").as_bytes(),
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
        "gentoo-list-systemd-profiles",
    )?;
    let profile = select_systemd_profile(&profiles).ok_or(RollingError::MissingSystemdProfile)?;
    chroot_checked(
        runner,
        root,
        ["eselect", "profile", "set", &profile],
        "gentoo-select-systemd-profile",
    )?;
    chroot_checked(runner, root, ["locale-gen"], "gentoo-target-locale")?;
    chroot_checked(runner, root, ["env-update"], "gentoo-target-environment")?;
    let mut packages = packages::emerge_package_set_for_plan(
        plan.plan(),
        boot,
        plan.plan().system.root_filesystem.as_str(),
        firmware,
    )?;
    for atom in [
        "sys-boot/grub",
        "sys-kernel/dracut",
        "sys-kernel/installkernel",
        "net-misc/openssh",
        "app-misc/ca-certificates",
    ] {
        if !packages.iter().any(|old| old == atom) {
            packages.push(atom.into());
        }
    }
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

/// Both targets use GRUB. EFI installs use the standard fallback path and never
/// create firmware entries; BIOS explicitly selects i386-pc and the bound disk.
pub(super) fn install_grub<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    boot: BootMode,
    disk: &str,
    tty_cmdline: &str,
) -> Result<(), RollingError> {
    validate_root(root)?;
    if !disk.strip_prefix("/dev/").is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    }) {
        return Err(RollingError::InvalidInput("bound boot disk"));
    }
    if tty_cmdline.len() > 1024
        || !tty_cmdline
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b" =,._-".contains(&b))
    {
        return Err(RollingError::InvalidInput("console kernel arguments"));
    }
    let defaults = root.join("etc/default/grub");
    validate_planned_file(&defaults)?;
    let body = format!("GRUB_CMDLINE_LINUX=\"${{GRUB_CMDLINE_LINUX}} {tty_cmdline}\"\n");
    let contents = replace_managed_block(
        &read_config(&defaults)?,
        &body,
        "# BEGIN rustaller console",
        "# END rustaller console",
        false,
    )?;
    write_file(&defaults, contents.as_bytes())?;
    match boot {
        BootMode::Efi => chroot_checked(
            runner,
            root,
            [
                "grub-install",
                "--target=x86_64-efi",
                "--efi-directory=/efi",
                "--bootloader-id=rustaller",
                "--no-nvram",
                "--removable",
            ],
            "rolling-install-grub-efi",
        )?,
        BootMode::Bios => chroot_checked(
            runner,
            root,
            ["grub-install", "--target=i386-pc", "--recheck", disk],
            "rolling-install-grub-bios",
        )?,
    }
    chroot_checked(
        runner,
        root,
        ["grub-mkconfig", "-o", "/boot/grub/grub.cfg"],
        "rolling-generate-grub-config",
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
fn valid_alpine_branch(branch: &str) -> bool {
    branch
        .strip_prefix('v')
        .and_then(|s| s.split_once('.'))
        .is_some_and(|(a, b)| {
            !a.is_empty() && !b.is_empty() && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit())
        })
}
fn valid_uuid(uuid: &str) -> bool {
    uuid.len() == 36
        && uuid.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn select_systemd_profile(output: &str) -> Option<String> {
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
            name.split('/').any(|part| part == "systemd")
                && name.split('/').any(|part| part == "amd64")
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
    // Preserve the selected stable systemd profile in the authenticated stage3
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
    crate::shared::dns::write_config(path, bytes, 0o644)
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
        let mut plan = InstallPlan::new(Target::Arch);
        plan.confhome = Some("https://assets.example/reinstall".into());
        validate_live_plan_with_uid(plan, "x86_64", 1024, crate::RUNTIME_VERSION, 0).unwrap()
    }
    fn successful_calls(count: usize) -> MockRunner {
        MockRunner::new((0..count).map(|_| CommandOutput::new(Some(0), vec![], vec![])))
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
    fn stable_systemd_selection_excludes_openrc_experimental_and_other_architectures() {
        let text = "Available profile symlink targets:\n [1] default/linux/amd64/23.0 (stable)\n [2] default/linux/amd64/23.0/systemd (stable) *\n [3] default/linux/amd64/23.0/desktop/systemd (stable)\n [4] default/linux/arm64/23.0/systemd (stable)\n [5] default/linux/amd64/experimental/systemd (exp)\n";
        assert_eq!(
            select_systemd_profile(text).as_deref(),
            Some("default/linux/amd64/23.0/systemd")
        );
        assert!(select_systemd_profile("[1] default/linux/amd64/23.0 (stable)\n").is_none());
        assert_eq!(select_systemd_profile("[1] default/linux/amd64/23.0/systemd (stable)\n[2] default/linux/amd64/23.0/split-usr/systemd (stable) *\n").as_deref(),Some("default/linux/amd64/23.0/split-usr/systemd"));
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
        let mut runner = successful_calls(4);
        configure_arch(&mut runner, &root, &arch_plan()).unwrap();
        assert_eq!(
            runner
                .calls()
                .iter()
                .map(|spec| spec.args[1].to_str().unwrap())
                .collect::<Vec<_>>(),
            ["pacman-key", "locale-gen", "pacman", "mkinitcpio"]
        );
        assert!(runner.calls()[2].args.iter().any(|arg| arg == "linux"));
        assert!(runner.calls()[3].args.iter().any(|arg| arg == "-P"));
    }
}
