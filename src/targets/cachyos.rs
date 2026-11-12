//! Opt-in CachyOS setup using a reviewed, digest-pinned official script archive.
//! Only a checked, isolated Arch target is modified. Package signatures remain required.
use crate::command::{
    CommandInput, CommandRunner, CommandSpec, OutputMode, capture_checked, run_checked,
};
use crate::shared::configure::validate_root;
use crate::shared::dns::write_config;
use std::{
    ffi::OsStr,
    fs, io,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

pub const ARCHIVE_URL: &str = "https://mirror.cachyos.org/cachyos-repo.tar.xz";
pub const ARCHIVE_SHA256: &str = "a1e3a74bdd12c10ac7317079e3e8c8e7d5557d818e4d4021364005a336b6afdc";
pub const RELEASE_FINGERPRINT: &str = "882DCFE48E2051D48E2562ABF3B607488DB35A47";
const KEY_URL: &str = "https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x882DCFE48E2051D48E2562ABF3B607488DB35A47";
const WORKER: &str = "usr/local/libexec/rustaller-cachyos-worker";
const PENDING: &str = "var/lib/rustaller/cachyos-firstboot.pending";
const UNIT: &str = "[Unit]\nDescription=Complete CachyOS package upgrade after installation\nWants=network-online.target systemd-networkd-wait-online.service\nAfter=network-online.target systemd-networkd-wait-online.service fix-eth-name.service\nConditionPathExists=/var/lib/rustaller/cachyos-firstboot.pending\nStartLimitIntervalSec=0\n[Service]\nType=oneshot\nExecStart=/usr/local/libexec/rustaller-cachyos-worker cachyos-firstboot\nRestart=on-failure\nRestartSec=60\nTimeoutStartSec=0\n[Install]\nWantedBy=multi-user.target\n";

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
/// Download and authenticate inputs before the destructive boundary.
pub fn prepare_assets<R: CommandRunner>(r: &mut R, work: &Path) -> Result<(), CachyosError> {
    validate_root(work).map_err(error)?;
    let archive = work.join("cachyos-repo.tar.xz");
    download(r, ARCHIVE_URL, &archive, 1024 * 1024)?;
    verify_archive(r, &archive)?;
    let key = work.join("cachyos-public.asc");
    download(r, KEY_URL, &key, 1024 * 1024)?;
    let home = work.join("cachyos-key-check");
    fs::create_dir(&home)?;
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700))?;
    let listing = capture(
        r,
        &command(
            "gpg",
            [
                OsStr::new("--batch"),
                "--homedir".as_ref(),
                home.as_os_str(),
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
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum IsaLevel {
    Baseline,
    V2,
    V3,
    V4,
}
/// glibc's supported/searched result includes kernel OSXSAVE/XCR0 usability,
/// unlike /proc/cpuinfo flags or CPUID alone. This is the official script's probe.
pub fn isa_level(loader_help: &str) -> IsaLevel {
    let supports = |level: &str| {
        loader_help
            .lines()
            .any(|l| l.trim() == format!("x86-64-{level} (supported, searched)"))
    };
    if supports("v4") {
        IsaLevel::V4
    } else if supports("v3") {
        IsaLevel::V3
    } else if supports("v2") {
        IsaLevel::V2
    } else {
        IsaLevel::Baseline
    }
}
/// Only reviewed substitutions: full pinned key instead of short-ID lookup,
/// noninteractive pacman and mandatory awk error propagation.
pub fn adapt_official_script(script: &str) -> Result<String, CachyosError> {
    let changes = [
        (
            "pacman-key --recv-keys F3B607488DB35A47 --keyserver keyserver.ubuntu.com",
            "pacman-key --add ./rustaller-cachyos.asc".to_owned(),
        ),
        (
            "pacman-key --lsign-key F3B607488DB35A47",
            format!("pacman-key --lsign-key {RELEASE_FINGERPRINT}"),
        ),
        (
            "gawk -i inplace -f $gawk_script $pacman_conf_cachyos || true",
            "gawk -i inplace -f $gawk_script $pacman_conf_cachyos".to_owned(),
        ),
        (
            "pacman -U \"${mirror_url}",
            "pacman --noconfirm -U \"${mirror_url}".to_owned(),
        ),
        (
            "    pacman -Syu\n",
            "    pacman --noconfirm -Syu\n".to_owned(),
        ),
    ];
    let mut out = script.to_owned();
    for (old, new) in changes {
        if out.matches(old).count() != 1 {
            return Err(error(
                "reviewed official script changed; update its pinned digest and adapter",
            ));
        }
        out = out.replacen(old, &new, 1);
    }
    Ok(out)
}
fn write(root: &Path, path: &str, bytes: &[u8], mode: u32) -> Result<(), CachyosError> {
    write_config::<CachyosError>(&root.join(path), bytes, mode)
}
/// Execute the official setup only inside a newly bootstrapped target chroot.
pub fn configure_repositories<R: CommandRunner>(
    r: &mut R,
    root: &Path,
    work: &Path,
) -> Result<(), CachyosError> {
    validate_root(root).map_err(error)?;
    verify_archive(r, &work.join("cachyos-repo.tar.xz"))?;
    if !root.join("etc/arch-release").is_file() {
        return Err(error("Arch target root required"));
    }
    checked(
        r,
        &chroot(
            root,
            [
                "pacman",
                "-Syu",
                "--noconfirm",
                "--needed",
                "gawk",
                "curl",
                "gnupg",
            ],
            "cachyos-target-setup-tools",
        ),
    )?;
    let help = capture(
        r,
        &chroot(
            root,
            ["/lib/ld-linux-x86-64.so.2", "--help"],
            "cachyos-detect-usable-isa",
        ),
    )?;
    if !help.contains("glibc-hwcaps") {
        return Err(error("cannot establish usable x86-64 ISA level"));
    }
    let isa = isa_level(&help);
    let target_dir = root.join("var/lib/rustaller/cachyos-setup");
    fs::create_dir_all(&target_dir)?;
    checked(
        r,
        &command(
            "tar",
            [
                OsStr::new("-xJf"),
                work.join("cachyos-repo.tar.xz").as_os_str(),
                "--no-same-owner".as_ref(),
                "--no-same-permissions".as_ref(),
                "-C".as_ref(),
                target_dir.as_os_str(),
            ],
            "cachyos-extract-pinned-scripts",
        ),
    )?;
    let dir = target_dir.join("cachyos-repo");
    let script = adapt_official_script(&fs::read_to_string(dir.join("cachyos-repo.sh"))?)?;
    write(
        root,
        "var/lib/rustaller/cachyos-setup/cachyos-repo/cachyos-repo.sh",
        script.as_bytes(),
        0o700,
    )?;
    write(
        root,
        "var/lib/rustaller/cachyos-setup/cachyos-repo/rustaller-cachyos.asc",
        &fs::read(work.join("cachyos-public.asc"))?,
        0o600,
    )?;
    // bash -c here is a fixed, audited command, never interpolated user input.
    checked(
        r,
        &chroot(
            root,
            [
                "bash",
                "-c",
                "cd /var/lib/rustaller/cachyos-setup/cachyos-repo && exec bash ./cachyos-repo.sh --install",
            ],
            "cachyos-official-repository-setup",
        ),
    )?;
    let path = root.join("etc/pacman.conf");
    let mut config = fs::read_to_string(&path)?;
    if matches!(isa, IsaLevel::Baseline | IsaLevel::V2)
        && !config.lines().any(|l| l.trim() == "[cachyos]")
    {
        // Official script only adds optimized tiers. The official manual documents
        // this unoptimized repository for CPUs below v3.
        let index = config
            .find("[core]")
            .ok_or_else(|| error("Arch core repository missing"))?;
        config.insert_str(
            index,
            "[cachyos]\nInclude = /etc/pacman.d/cachyos-mirrorlist\n\n",
        );
        write(root, "etc/pacman.conf", config.as_bytes(), 0o644)?;
    }
    validate_repository_config(&config, isa)?;
    fs::remove_dir_all(target_dir)?;
    Ok(())
}
pub fn validate_repository_config(config: &str, isa: IsaLevel) -> Result<(), CachyosError> {
    let lines = config
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>();
    if !lines.contains(&"SigLevel = Required DatabaseOptional")
        || lines.iter().any(|l| {
            (l.starts_with("SigLevel") && *l != "SigLevel = Required DatabaseOptional")
                || (l.starts_with("RemoteFileSigLevel") && *l != "RemoteFileSigLevel = Required")
        })
    {
        return Err(error("mandatory package signature policy changed"));
    }
    let tier_complete = |tier: &str| {
        [
            format!("[cachyos-{tier}]"),
            format!("[cachyos-core-{tier}]"),
            format!("[cachyos-extra-{tier}]"),
        ]
        .iter()
        .all(|repo| lines.contains(&repo.as_str()))
    };
    let complete = match isa {
        IsaLevel::V4 => tier_complete("v4") || tier_complete("znver4"),
        IsaLevel::V3 => tier_complete("v3"),
        _ => true,
    };
    if !lines.contains(&"[cachyos]") || !complete {
        return Err(error(
            "official script did not enable the complete detected ISA repository set",
        ));
    }
    if matches!(isa, IsaLevel::Baseline | IsaLevel::V2)
        && lines
            .iter()
            .any(|l| l.starts_with("[cachyos-") && !l.contains("mirror"))
    {
        return Err(error("unsupported optimized ISA repository"));
    }
    if isa == IsaLevel::V3
        && lines
            .iter()
            .any(|l| l.contains("-v4]") || l.contains("-znver4]"))
    {
        return Err(error("unsupported v4 repository"));
    }
    Ok(())
}
/// Independent worker survives the NIC firstboot worker's normal cleanup.
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
    crate::shared::dns::ensure_parent(&link).map_err(error)?;
    symlink("../rustaller-cachyos-update.service", link)?;
    Ok(())
}
pub fn run_firstboot_update<R: CommandRunner>(r: &mut R, root: &Path) -> Result<(), CachyosError> {
    regular(&root.join(PENDING), 1024)?;
    if !root.join("etc/arch-release").is_file() {
        return Err(error("firstboot update requires Arch"));
    }
    for (args, phase) in [
        (
            vec!["pacman", "-Syu", "--noconfirm"],
            "cachyos-firstboot-full-upgrade",
        ),
        (vec!["mkinitcpio", "-P"], "cachyos-firstboot-initramfs"),
        (
            vec!["grub-mkconfig", "-o", "/boot/grub/grub.cfg"],
            "cachyos-firstboot-grub",
        ),
    ] {
        let spec = if root == Path::new("/") {
            command(args[0], &args[1..], phase)
        } else {
            // Isolated roots are used by regression mocks, never a CLI path.
            chroot(root, args, phase)
        };
        checked(r, &spec)?;
    }
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
