//! Authenticated CachyOS repositories and the fixed Arch server kernel.
use crate::command::{
    CommandInput, CommandRunner, CommandSpec, OutputMode, capture_checked, run_checked,
};
use crate::host::inspect::cpu::{self, IsaLevel};
use crate::model::Target;
use crate::postinstall::required_kernel;
use crate::shared::configure::write_config;
use crate::shared::configure::{read_config, validate_root};
use std::{
    ffi::OsStr,
    fs, io,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

// The upstream mirror's rolling archive can change independently of this installer.
pub const ARCHIVE_URL: &str = "https://codeload.github.com/CachyOS/cachyos-repo-add-script/tar.gz/5b27dc1eff276e104077cfaf75798b4c290eaef4";
pub const ARCHIVE_SHA256: &str = "1076d254cf10036dfa910bd6ca8e45c8d623836d19b2f1722fff7f6cf7aee510";
const ARCHIVE_SCRIPT: &str =
    "cachyos-repo-add-script-5b27dc1eff276e104077cfaf75798b4c290eaef4/cachyos-repo.sh";
const MIRROR: &str = "https://mirror.cachyos.org/repo";
pub const BASELINE_REPOSITORY: &str = "https://mirror.cachyos.org/repo/x86_64/cachyos";
const KEYRING_PACKAGE: &str = "cachyos-keyring-20240331-1-any.pkg.tar.zst";
const PUBLIC_KEY: &str = "cachyos-public.asc";
const KEYRING_MAX_BYTES: u64 = 1024 * 1024;
pub const RELEASE_FINGERPRINT: &str = "882DCFE48E2051D48E2562ABF3B607488DB35A47";
const KEY_URL: &str = "https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x882DCFE48E2051D48E2562ABF3B607488DB35A47";
const WORKER: &str = "usr/local/libexec/rustaller-cachyos-worker";
const PENDING: &str = "var/lib/rustaller/cachyos-firstboot.pending";
const UNIT: &str = "[Unit]\nDescription=Complete CachyOS package upgrade after installation\nWants=network-online.target systemd-networkd-wait-online.service\nAfter=network-online.target systemd-networkd-wait-online.service\nConditionPathExists=/var/lib/rustaller/cachyos-firstboot.pending\nStartLimitIntervalSec=0\n[Service]\nType=oneshot\nExecStart=/usr/local/libexec/rustaller-cachyos-worker cachyos-firstboot\nRestart=on-failure\nRestartSec=60\nTimeoutStartSec=0\n[Install]\nWantedBy=multi-user.target\n";

#[derive(Debug)]
pub struct CachyosError(String);
impl std::fmt::Display for CachyosError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CachyOS setup stopped: {}", self.0)
    }
}
impl std::error::Error for CachyosError {}
impl From<crate::shared::configure::ConfigurationError> for CachyosError {
    fn from(e: crate::shared::configure::ConfigurationError) -> Self {
        Self(e.to_string())
    }
}
impl From<io::Error> for CachyosError {
    fn from(e: io::Error) -> Self {
        Self(e.to_string())
    }
}
fn error(e: impl std::fmt::Display) -> CachyosError {
    CachyosError(e.to_string())
}
fn command<I, S>(program: &str, args: I, phase: &str) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    crate::command::live_command(program, args, phase).env("LC_ALL", "C")
}
fn chroot<I, S>(root: &Path, args: I, phase: &str) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    {
        let mut argv = vec![root.as_os_str().to_owned()];
        argv.extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        crate::command::target_command("chroot", argv, phase).env("LC_ALL", "C")
    }
}
fn checked<R: CommandRunner>(r: &mut R, c: &CommandSpec) -> Result<(), CachyosError> {
    run_checked(r, c).map_err(error)
}
fn capture<R: CommandRunner>(r: &mut R, c: &CommandSpec) -> Result<String, CachyosError> {
    String::from_utf8(
        capture_checked(r, &c.clone().stdout_mode(OutputMode::Capture)).map_err(error)?,
    )
    .map_err(error)
}
fn regular(path: &Path, max: u64) -> Result<(), CachyosError> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.len() == 0 || m.len() > max {
        return Err(error("missing, oversized or nonregular input"));
    }
    Ok(())
}
fn download<R: CommandRunner>(
    r: &mut R,
    url: &str,
    path: &Path,
    max: u64,
) -> Result<(), CachyosError> {
    checked(
        r,
        &command(
            "curl",
            [
                OsStr::new("--disable"),
                "--fail".as_ref(),
                "--location".as_ref(),
                "--proto".as_ref(),
                "=https".as_ref(),
                "--proto-redir".as_ref(),
                "=https".as_ref(),
                "--connect-timeout".as_ref(),
                "20".as_ref(),
                "--max-time".as_ref(),
                "180".as_ref(),
                "--max-filesize".as_ref(),
                max.to_string().as_ref(),
                "--output".as_ref(),
                path.as_os_str(),
                url.as_ref(),
            ],
            "cachyos-download",
        ),
    )?;
    regular(path, max)
}
fn verify_archive<R: CommandRunner>(r: &mut R, path: &Path) -> Result<(), CachyosError> {
    regular(path, 1024 * 1024)?;
    checked(
        r,
        &command(
            "sha256sum",
            ["--check", "--status"],
            "cachyos-verify-official-scripts",
        )
        .stdin(CommandInput::Bytes(
            format!("{ARCHIVE_SHA256}  {}\n", path.display()).into_bytes(),
        )),
    )
}
/// Authenticate the upstream setup revision, trust root, keyring package and
/// repository metadata, then resolve the required package and dependencies.
pub fn prepare_assets<R: CommandRunner>(r: &mut R, work: &Path) -> Result<(), CachyosError> {
    validate_root(work).map_err(error)?;
    let archive = work.join("cachyos-repo.tar.gz");
    download(r, ARCHIVE_URL, &archive, 1024 * 1024)?;
    verify_archive(r, &archive)?;
    let script = capture(
        r,
        &command(
            "tar",
            [
                OsStr::new("-xzOf"),
                archive.as_os_str(),
                ARCHIVE_SCRIPT.as_ref(),
            ],
            "cachyos-read-reviewed-keyring-source",
        ),
    )?;
    if !script.lines().any(|line| {
        line.trim_start()
            .starts_with(&format!("pacman -U \"${{mirror_url}}/{KEYRING_PACKAGE}\""))
    }) {
        return Err(error(
            "reviewed source no longer identifies the native keyring package",
        ));
    }

    let key = work.join(PUBLIC_KEY);
    download(r, KEY_URL, &key, KEYRING_MAX_BYTES)?;
    let home = work.join("cachyos-gnupg");
    fs::create_dir(&home)?;
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?;
    let gpg = |args: &[&OsStr], phase: &str| {
        command(
            "gpg",
            [
                OsStr::new("--batch"),
                "--homedir".as_ref(),
                home.as_os_str(),
            ]
            .into_iter()
            .chain(args.iter().copied()),
            phase,
        )
    };
    let listing = capture(
        r,
        &gpg(
            &[
                "--with-colons".as_ref(),
                "--show-keys".as_ref(),
                key.as_os_str(),
            ],
            "cachyos-verify-pinned-primary-key",
        ),
    )?;
    if !valid_primary_key(&listing) {
        return Err(error(
            "release key does not match the pinned full fingerprint",
        ));
    }
    checked(
        r,
        &gpg(
            &["--import".as_ref(), key.as_os_str()],
            "cachyos-import-release-key",
        ),
    )?;
    let package = work.join(KEYRING_PACKAGE);
    let signature = work.join(format!("{KEYRING_PACKAGE}.sig"));
    let url = format!("{BASELINE_REPOSITORY}/{KEYRING_PACKAGE}");
    download(r, &url, &package, KEYRING_MAX_BYTES)?;
    download(r, &format!("{url}.sig"), &signature, 64 * 1024)?;
    checked(
        r,
        &gpg(
            &[
                "--verify".as_ref(),
                signature.as_os_str(),
                package.as_os_str(),
            ],
            "cachyos-verify-keyring-package",
        ),
    )?;
    checked(
        r,
        &command(
            "gpgconf",
            [
                OsStr::new("--homedir"),
                home.as_os_str(),
                "--kill".as_ref(),
                "all".as_ref(),
            ],
            "cachyos-stop-release-key-agent",
        ),
    )?;
    fs::remove_dir_all(home)?;

    let home = work.join("cachyos-pacman-gnupg");
    let db = work.join("cachyos-pacman-db");
    fs::create_dir(&db)?;
    let keyring = |args: &[&OsStr], phase: &str| {
        command(
            "pacman-key",
            [OsStr::new("--gpgdir"), home.as_os_str()]
                .into_iter()
                .chain(args.iter().copied()),
            phase,
        )
    };
    checked(
        r,
        &keyring(&["--init".as_ref()], "cachyos-preflight-keyring-init"),
    )?;
    checked(
        r,
        &keyring(
            &["--populate".as_ref(), "archlinux".as_ref()],
            "cachyos-preflight-arch-keys",
        ),
    )?;
    checked(
        r,
        &keyring(
            &["--add".as_ref(), key.as_os_str()],
            "cachyos-preflight-add-release-key",
        ),
    )?;
    checked(
        r,
        &keyring(
            &["--lsign-key".as_ref(), RELEASE_FINGERPRINT.as_ref()],
            "cachyos-preflight-trust-release-key",
        ),
    )?;

    let isa = guest_isa();
    let config = work.join("cachyos-pacman.conf");
    let text = repository_config(&super::rolling::arch_configuration(false)?, isa)?;
    let text = if Path::new(super::install::LIVE_TRANSPORT_CA).is_file() {
        super::rolling::arch_transport_config(&text, super::install::LIVE_TRANSPORT_CA)?
    } else {
        text
    };
    write_config::<CachyosError>(&config, text.as_bytes(), 0o600)?;
    checked(
        r,
        &command(
            "pacman",
            [
                OsStr::new("--config"),
                config.as_os_str(),
                "--gpgdir".as_ref(),
                home.as_os_str(),
                "--dbpath".as_ref(),
                db.as_os_str(),
                "-Syup".as_ref(),
                "--noconfirm".as_ref(),
                "--print-format".as_ref(),
                "%n".as_ref(),
                server_target(isa).as_ref(),
                "base".as_ref(),
                "mkinitcpio".as_ref(),
            ],
            "cachyos-resolve-authenticated-server-kernel",
        ),
    )?;
    checked(
        r,
        &command(
            "gpgconf",
            [
                OsStr::new("--homedir"),
                home.as_os_str(),
                "--kill".as_ref(),
                "all".as_ref(),
            ],
            "cachyos-stop-preflight-key-agent",
        ),
    )?;
    Ok(())
}
pub fn valid_primary_key(listing: &str) -> bool {
    let mut primary = false;
    let mut primary_count = 0;
    let mut found = Vec::new();
    for line in listing.lines() {
        let fields = line.split(':').collect::<Vec<_>>();
        match fields.first().copied() {
            Some("pub") => {
                primary_count += 1;
                if fields.get(1).is_some_and(|v| matches!(*v, "e" | "r" | "d")) {
                    return false;
                }
                primary = true;
            }
            Some("sub") => primary = false,
            Some("fpr") if primary => {
                found.push(fields.get(9).copied().unwrap_or(""));
                primary = false;
            }
            _ => {}
        }
    }
    primary_count == 1 && found == [RELEASE_FINGERPRINT]
}
fn guest_isa() -> IsaLevel {
    cpu::inspect(Path::new("/"))
        .isa_level
        .unwrap_or(IsaLevel::V1)
}

fn repo_tier(isa: IsaLevel) -> Option<&'static str> {
    match isa {
        IsaLevel::V4 => Some("v4"),
        IsaLevel::V3 => Some("v3"),
        IsaLevel::V1 | IsaLevel::V2 => None,
    }
}

fn kernel_package() -> &'static str {
    required_kernel(Target::Arch).unwrap()
}

fn server_target(isa: IsaLevel) -> String {
    let repository = repo_tier(isa)
        .map(|tier| format!("cachyos-{tier}"))
        .unwrap_or_else(|| "cachyos".into());
    format!("{repository}/{}", kernel_package())
}

fn repository_blocks(isa: IsaLevel) -> String {
    let mut repositories = String::new();
    if let Some(tier) = repo_tier(isa) {
        for repository in [
            format!("cachyos-{tier}"),
            format!("cachyos-core-{tier}"),
            format!("cachyos-extra-{tier}"),
        ] {
            repositories.push_str(&format!(
                "[{repository}]\nSigLevel = Required\nServer = {MIRROR}/x86_64_{tier}/{repository}\n\n"
            ));
        }
    }
    repositories.push_str(&format!(
        "[cachyos]\nSigLevel = Required\nServer = {BASELINE_REPOSITORY}\n\n"
    ));
    repositories
}

pub fn repository_config(base: &str, isa: IsaLevel) -> Result<String, CachyosError> {
    let (options, arch_repos) = base
        .split_once("\n[core]\n")
        .ok_or_else(|| error("Arch core repository missing"))?;
    if options
        .lines()
        .filter(|line| *line == "Architecture = x86_64")
        .count()
        != 1
        || base
            .lines()
            .any(|line| line.trim_start().starts_with("[cachyos"))
    {
        return Err(error("unexpected initial Arch repository configuration"));
    }
    let options = if let Some(tier) = repo_tier(isa) {
        options.replacen(
            "Architecture = x86_64",
            &format!("Architecture = x86_64 x86_64_{tier}"),
            1,
        )
    } else {
        options.into()
    };
    Ok(format!(
        "{options}\n{}[core]\n{arch_repos}",
        repository_blocks(isa)
    ))
}

fn write(root: &Path, path: &str, bytes: &[u8], mode: u32) -> Result<(), CachyosError> {
    write_config::<CachyosError>(&root.join(path), bytes, mode)
}

/// Install the signed native keyring before repository activation. The first
/// transaction that can install optimized packages is the full kernel upgrade.
pub fn configure_repositories<R: CommandRunner>(
    r: &mut R,
    root: &Path,
    work: &Path,
) -> Result<(), CachyosError> {
    validate_root(root).map_err(error)?;
    if !root.join("etc/arch-release").is_file() {
        return Err(error("Arch target root required"));
    }
    let key = work.join(PUBLIC_KEY);
    let package = work.join(KEYRING_PACKAGE);
    let sig_name = format!("{KEYRING_PACKAGE}.sig");
    let signature = work.join(&sig_name);
    let staging = "var/lib/rustaller/cachyos-setup";
    let key_path = format!("/{staging}/{PUBLIC_KEY}");
    let package_path = format!("/{staging}/{KEYRING_PACKAGE}");
    for (from, name, limit) in [
        (&key, PUBLIC_KEY, KEYRING_MAX_BYTES),
        (&package, KEYRING_PACKAGE, KEYRING_MAX_BYTES),
        (&signature, &sig_name, 64 * 1024),
    ] {
        regular(from, limit)?;
        write(root, &format!("{staging}/{name}"), &fs::read(from)?, 0o600)?;
    }
    checked(
        r,
        &chroot(
            root,
            ["pacman-key", "--populate", "archlinux"],
            "arch-populate-native-keyring",
        ),
    )?;
    checked(
        r,
        &chroot(
            root,
            ["pacman-key", "--add", &key_path],
            "cachyos-add-pinned-release-key",
        ),
    )?;
    checked(
        r,
        &chroot(
            root,
            ["pacman-key", "--lsign-key", RELEASE_FINGERPRINT],
            "cachyos-trust-pinned-release-key",
        ),
    )?;
    checked(
        r,
        &chroot(
            root,
            ["pacman", "-U", "--noconfirm", "--needed", &package_path],
            "cachyos-install-native-signed-keyring",
        ),
    )?;
    checked(
        r,
        &chroot(
            root,
            ["pacman-key", "--populate", "cachyos"],
            "cachyos-populate-native-keyring",
        ),
    )?;
    let path = "etc/pacman.conf";
    let config = repository_config(&read_config(&root.join(path))?, guest_isa())?;
    write(root, path, config.as_bytes(), 0o644)?;
    fs::remove_dir_all(root.join(staging))?;
    Ok(())
}

/// Match the active repository tier to the capabilities of every schedulable
/// vCPU. A post-install CPU change must not trigger an incompatible upgrade.
pub(super) fn configured_server_target(root: &Path) -> Result<String, CachyosError> {
    let config = read_config(&root.join("etc/pacman.conf"))?;
    let active: Vec<_> = config
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("[cachyos"))
        .collect();
    let supported = guest_isa();
    let expected = |tier| {
        [
            format!("[cachyos-{tier}]"),
            format!("[cachyos-core-{tier}]"),
            format!("[cachyos-extra-{tier}]"),
            "[cachyos]".into(),
        ]
    };
    let selected = if active == ["[cachyos]"] {
        IsaLevel::V1
    } else if matches!(supported, IsaLevel::V3 | IsaLevel::V4) && active == expected("v3") {
        IsaLevel::V3
    } else if supported == IsaLevel::V4 && active == expected("v4") {
        IsaLevel::V4
    } else {
        return Err(error(
            "configured CachyOS repositories do not match usable guest ISA",
        ));
    };
    if !config.contains(&repository_blocks(selected)) {
        return Err(error(
            "configured CachyOS server or signature policy changed",
        ));
    }
    Ok(server_target(selected))
}

pub(super) fn verify_server_kernel<R: CommandRunner>(
    r: &mut R,
    root: &Path,
) -> Result<(), CachyosError> {
    let native = |args: &[&str], phase| {
        if root == Path::new("/") {
            command(args[0], &args[1..], phase)
        } else {
            chroot(root, args, phase)
        }
    };
    let name = kernel_package();
    let files = capture(
        r,
        &native(
            &["pacman", "-Qql", name],
            "cachyos-list-installed-server-files",
        ),
    )?;
    let mut modules = files.lines().filter_map(|line| {
        line.strip_prefix("/usr/lib/modules/")?
            .strip_suffix("/vmlinuz")
    });
    let version = modules
        .next()
        .filter(|&version| {
            version.ends_with("-cachyos-server")
                && version
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
        })
        .ok_or_else(|| error("server module release missing from installed package"))?;
    if modules.next().is_some() {
        return Err(error(
            "ambiguous server module release in installed package",
        ));
    }
    let directory = format!("usr/lib/modules/{version}");
    let pkgbase = root.join(&directory).join("pkgbase");
    regular(&pkgbase, 128)?;
    if fs::read_to_string(pkgbase)?.trim() != name {
        return Err(error("server module package identity mismatch"));
    }
    let module_kernel = format!("/{directory}/vmlinuz");
    let boot_kernel = format!("/boot/vmlinuz-{name}");
    let initramfs = format!("/boot/initramfs-{name}.img");
    for path in [&module_kernel, &boot_kernel, &initramfs] {
        regular(&root.join(path.trim_start_matches('/')), 1024 * 1024 * 1024)?;
    }
    let hashes = capture(
        r,
        &native(
            &["sha256sum", &module_kernel, &boot_kernel],
            "cachyos-compare-native-server-kernel",
        ),
    )?;
    let mut hashes = hashes
        .lines()
        .filter_map(|line| line.split_ascii_whitespace().next());
    if !matches!((hashes.next(), hashes.next()), (Some(a), Some(b)) if a.len() == 64 && a == b) {
        return Err(error(
            "FAT32 kernel differs from the server package payload",
        ));
    }
    let image = capture(
        r,
        &native(
            &["lsinitcpio", "-l", &initramfs],
            "cachyos-verify-matching-initramfs",
        ),
    )?;
    let modules = format!("{directory}/");
    if !image
        .lines()
        .any(|line| line.trim_start_matches("./").starts_with(&modules))
    {
        return Err(error("initramfs release differs from the server package"));
    }
    Ok(())
}
/// The dedicated worker remains until the first-boot upgrade succeeds.
pub fn install_firstboot_update(root: &Path) -> Result<(), CachyosError> {
    validate_root(root).map_err(error)?;
    let bytes = fs::read("/rustaller-worker")?;
    install_firstboot_update_with_worker(root, &bytes)
}
pub fn install_firstboot_update_with_worker(root: &Path, bytes: &[u8]) -> Result<(), CachyosError> {
    validate_root(root).map_err(error)?;
    if bytes.is_empty()
        || !bytes
            .windows(crate::RUNTIME_VERSION.len())
            .any(|w| w == crate::RUNTIME_VERSION.as_bytes())
    {
        return Err(error("matching firstboot worker missing"));
    }
    write(root, WORKER, bytes, 0o755)?;
    write(root, PENDING, b"CachyOS package upgrade pending\n", 0o600)?;
    write(
        root,
        "etc/systemd/system/rustaller-cachyos-update.service",
        UNIT.as_bytes(),
        0o644,
    )?;
    let link =
        root.join("etc/systemd/system/multi-user.target.wants/rustaller-cachyos-update.service");
    crate::shared::configure::ensure_parent(&link).map_err(error)?;
    symlink("../rustaller-cachyos-update.service", link)?;
    Ok(())
}
pub fn run_firstboot_update<R: CommandRunner>(r: &mut R, root: &Path) -> Result<(), CachyosError> {
    regular(&root.join(PENDING), 1024)?;
    if !root.join("etc/arch-release").is_file() {
        return Err(error("firstboot update requires Arch"));
    }
    let server = configured_server_target(root)?;
    let native = |args: &[&str], phase| {
        if root == Path::new("/") {
            command(args[0], &args[1..], phase)
        } else {
            chroot(root, args, phase)
        }
    };
    for (args, phase) in [
        (
            vec!["/usr/local/libexec/rustaller-boot-update", "check"],
            "cachyos-firstboot-check-boot-mount",
        ),
        (
            vec!["pacman", "-Syu", "--noconfirm", "--needed", &server],
            "cachyos-firstboot-full-upgrade",
        ),
        (vec!["mkinitcpio", "-P"], "cachyos-firstboot-initramfs"),
    ] {
        checked(r, &native(&args, phase))?;
    }
    verify_server_kernel(r, root)?;
    checked(
        r,
        &native(
            &["/usr/local/libexec/rustaller-boot-update", "refresh"],
            "cachyos-firstboot-boot-refresh",
        ),
    )?;
    // Only successful package hooks and explicit boot-artifact rebuilds complete
    // the job. A failed command leaves the pending marker and service for retry.
    fs::remove_file(root.join(PENDING))?;
    for path in [
        "etc/systemd/system/multi-user.target.wants/rustaller-cachyos-update.service",
        "etc/systemd/system/rustaller-cachyos-update.service",
        WORKER,
    ] {
        fs::remove_file(root.join(path))?;
    }
    Ok(())
}
