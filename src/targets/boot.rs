//! Installation and maintenance of native Linux boot generations on UUID-bound FAT32.
//! Both firmware paths consume the same versioned TOML and verified artifacts.
use crate::command::{CommandInput, CommandRunner, CommandSpec, capture_checked};
use crate::model::{BootMode, Target};
use crate::shared::configure::{validate_planned_file, validate_root, write_config};
use crate::shared::storage::DiskLayout;
use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};
pub(super) type Result<T> = std::result::Result<T, String>;
pub const VERSION: &str = "1";
const BIOS_STATE: &str = "RUSTALLER-BOOT-1-BIOS";
const NATIVE_EFI_STATE: &str = "RUSTALLER-BOOT-1-EFI";
const BACKUP_CONFIG: &str = "rustaller/boot.toml.previous";
const NATIVE_CONFIG: &str = "rustaller/boot.toml";
const WORKER: &str = "usr/local/libexec/rustaller-boot-update";
fn cmd(program: &str, args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>) -> CommandSpec {
    CommandSpec::new(program, args)
        .phase("target-boot")
        .stdin(CommandInput::Null)
}
/// Stage only artifacts embedded and checked by the release build. The Live
/// system never needs a C toolchain or a network fetch to install a loader.
pub fn prepare(mode: BootMode, work: &Path) -> Result<()> {
    use crate::boot_artifacts as assets;
    validate_root(work).map_err(|e| e.to_string())?;
    if !assets::UPDATE.starts_with(b"\x7fELF")
        || !assets::UPDATE
            .windows(crate::RUNTIME_VERSION.len())
            .any(|v| v == crate::RUNTIME_VERSION.as_bytes())
    {
        return Err("matching release boot-update helper missing".into());
    }
    match mode {
        BootMode::Efi => {
            let path = work.join("BOOTX64.EFI");
            stage(&path, assets::UEFI, 0o600)?;
            super::install::verify_efi_loader(&path).map_err(|e| e.to_string())
        }
        BootMode::Bios => {
            super::bios::validate_stages(assets::BIOS_MBR, assets::BIOS_STAGE)?;
            stage(&work.join("bios-mbr.bin"), assets::BIOS_MBR, 0o600)?;
            stage(&work.join("bios-stage2.bin"), assets::BIOS_STAGE, 0o600)
        }
    }
}
fn stage(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    if bytes.is_empty() {
        return Err("release boot artifact missing".into());
    }
    write_config::<crate::shared::configure::ConfigurationError>(path, bytes, mode)
        .map_err(|e| e.to_string())
}
pub fn install(
    runner: &mut impl CommandRunner,
    root: &Path,
    layout: &DiskLayout,
    work: &Path,
    uuid: &str,
    boot_uuid: &str,
) -> Result<()> {
    validate_root(root).map_err(|e| e.to_string())?;
    if !crate::shared::storage::valid_uuid(uuid) || !crate::shared::storage::valid_uuid(boot_uuid) {
        return Err("invalid root UUID".into());
    }
    verify_mount(runner, root, boot_uuid)?;
    let state_version = match layout.boot_mode() {
        BootMode::Efi => {
            fat_replace(
                &root.join("boot/EFI/BOOT/BOOTX64.EFI"),
                &fs::read(work.join("BOOTX64.EFI")).map_err(|e| e.to_string())?,
            )?;
            NATIVE_EFI_STATE
        }
        BootMode::Bios => {
            super::bios::install(layout, work)?;
            BIOS_STATE
        }
    };
    let state = format!(
        "{state_version}\n{uuid}\n{}\n{boot_uuid}\n",
        layout.root_filesystem().as_str()
    );
    write_config::<crate::shared::configure::ConfigurationError>(
        &root.join("etc/rustaller/boot-state"),
        state.as_bytes(),
        0o600,
    )
    .map_err(|e| e.to_string())?;
    install_hooks(root, layout.target(), crate::boot_artifacts::UPDATE)?;
    refresh_inner(runner, root, true)
}
pub fn install_hooks(root: &Path, target: Target, worker: &[u8]) -> Result<()> {
    validate_root(root).map_err(|e| e.to_string())?;
    if !worker
        .windows(crate::RUNTIME_VERSION.len())
        .any(|v| v == crate::RUNTIME_VERSION.as_bytes())
    {
        return Err("incompatible boot refresh worker".into());
    }
    let links: &[&str] = match target {
        Target::Debian => &[
            "etc/kernel/preinst.d/zz-rustaller-boot-check",
            "etc/kernel/prerm.d/zz-rustaller-boot-check",
            "etc/initramfs-tools/hooks/zz-rustaller-boot-check",
            "etc/kernel/postinst.d/zz-rustaller-boot",
            "etc/kernel/postrm.d/zz-rustaller-boot",
            "etc/initramfs/post-update.d/zz-rustaller-boot",
        ],
        Target::Alpine => &["etc/apk/commit_hooks.d/zz-rustaller-boot"],
        Target::Gentoo => &[
            "etc/kernel/preinst.d/zz-rustaller-boot-check",
            "etc/kernel/postinst.d/zz-rustaller-boot",
        ],
        Target::Arch => &["etc/initcpio/post/zz-rustaller-boot"],
        Target::Reset => return Err("installation target required".into()),
    };
    write_config::<crate::shared::configure::ConfigurationError>(&root.join(WORKER), worker, 0o755)
        .map_err(|e| e.to_string())?;
    for name in links {
        let path = root.join(name);
        validate_planned_file(&path).map_err(|e| e.to_string())?;
        crate::shared::configure::ensure_parent(&path).map_err(|e| e.to_string())?;
        if *name == "etc/initramfs-tools/hooks/zz-rustaller-boot-check" {
            // initramfs-tools runs `sh -n` before accepting hooks. An ELF
            // symlink is rejected as shell syntax and silently skipped.
            write_config::<crate::shared::configure::ConfigurationError>(
                &path,
                b"#!/bin/sh\ncase \"${1:-}\" in prereqs) exit 0 ;; esac\nexec /usr/local/libexec/rustaller-boot-update check\n",
                0o755,
            ).map_err(|e| e.to_string())?;
        } else {
            symlink(format!("/{WORKER}"), &path).map_err(|e| e.to_string())?;
        }
    }
    install_generator_guards(root, target)?;
    if target == Target::Arch {
        write_config::<crate::shared::configure::ConfigurationError>(&root.join("etc/pacman.d/hooks/00-rustaller-boot-mount.hook"), b"[Trigger]\nOperation=Install\nOperation=Upgrade\nOperation=Remove\nType=Path\nTarget=boot/*\nTarget=usr/lib/modules/*\n[Action]\nDescription=Verify Rustaller FAT32 boot mount\nWhen=PreTransaction\nExec=/usr/local/libexec/rustaller-boot-update check\nAbortOnFail\n", 0o644).map_err(|e|e.to_string())?;
        write_config::<crate::shared::configure::ConfigurationError>(&root.join("etc/pacman.d/hooks/zz-rustaller-boot.hook"),b"[Trigger]\nOperation=Install\nOperation=Upgrade\nOperation=Remove\nType=Path\nTarget=boot/*\nTarget=usr/lib/modules/*/pkgbase\nTarget=usr/lib/initcpio/*\n[Action]\nDescription=Refresh Linux boot generations\nWhen=PostTransaction\nExec=/usr/local/libexec/rustaller-boot-update refresh\n",0o644).map_err(|e|e.to_string())?;
    }
    Ok(())
}
/// Native generators source these configuration files before touching output.
/// Keep the distribution configuration and add only a bounded managed block.
fn install_generator_guards(root: &Path, target: Target) -> Result<()> {
    const CHECK: &str = "/usr/local/libexec/rustaller-boot-update check || exit 1\n";
    let (relative, mut guard) = match target {
        Target::Arch => ("etc/mkinitcpio.conf", CHECK.to_owned()),
        // Build-only package configuration is sourced before output is touched.
        // Do not use initramfs.conf/conf.d: mkinitramfs copies those into the
        // image and /init sources them without the installed Rust worker.
        Target::Debian => (
            "usr/share/initramfs-tools/conf-hooks.d/00-rustaller-mount",
            CHECK.to_owned(),
        ),
        Target::Gentoo => (
            "etc/dracut.conf.d/00-rustaller-mount.conf",
            CHECK.to_owned(),
        ),
        Target::Alpine => (
            "etc/mkinitfs/mkinitfs.conf",
            format!(
                "{CHECK}if [ -z \"${{list_features:-}}${{list_sources:-}}${{DEBUG_KMOD:-}}\" ]; then\n trap 'rustaller_status=$?; if [ \"$rustaller_status\" -eq 0 ]; then /usr/local/libexec/rustaller-boot-update refresh || exit 1; fi; exit \"$rustaller_status\"' EXIT\nfi\n"
            ),
        ),
        _ => return Ok(()),
    };
    guard.insert_str(
        0,
        "# Require the recorded FAT32 /boot before native output writes.\n",
    );
    let path = root.join(relative);
    validate_planned_file(&path).map_err(|e| e.to_string())?;
    let original = match crate::shared::configure::read_config(&path) {
        Ok(text) => text,
        Err(crate::shared::configure::ConfigurationError::Filesystem(e))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            String::new()
        }
        Err(e) => return Err(e.to_string()),
    };
    let text = crate::shared::configure::replace_managed_block(
        &original,
        &guard,
        "# BEGIN rustaller boot guard",
        "# END rustaller boot guard",
        false,
    )
    .map_err(|e| e.to_string())?;
    write_config::<crate::shared::configure::ConfigurationError>(&path, text.as_bytes(), 0o644)
        .map_err(|e| e.to_string())?;
    if target == Target::Gentoo {
        // dracut has no stable post-output hook. Preserve the packaged binary.
        // Explicit/unknown output arguments may refer to installkernel staging;
        // defer those to installkernel postinst or an explicit worker refresh.
        write_config::<crate::shared::configure::ConfigurationError>(
            &root.join("usr/local/sbin/dracut"),
            br#"#!/bin/sh
/usr/local/libexec/rustaller-boot-update check || exit 1
rustaller_refresh=yes
for rustaller_arg in "$@"; do
 case "$rustaller_arg" in
  -f|--force|--regenerate-all|-H|--hostonly|-N|--no-hostonly|-v|--verbose|-q|--quiet) ;;
  *) rustaller_refresh=no ;;
 esac
done
/usr/bin/dracut "$@"
rustaller_status=$?
[ "$rustaller_status" -eq 0 ] || exit "$rustaller_status"
if [ "$rustaller_refresh" = yes ]; then
 exec /usr/local/libexec/rustaller-boot-update refresh
fi
exit 0
"#,
            0o755,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub fn hook_action(args: &[String]) -> bool {
    !args.iter().any(|v| v == "pre-commit")
}
fn regular(root: &Path, path: &Path) -> Result<Vec<u8>> {
    let (resolved, size) = checked_source(root, path)?;
    if size > 4 * 1024 * 1024 {
        return Err("oversized boot metadata".into());
    }
    fs::read(resolved).map_err(|e| e.to_string())
}
fn kernel_order(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    while !a.is_empty() && !b.is_empty() {
        let ordering = if a[0].is_ascii_digit() && b[0].is_ascii_digit() {
            let na = a.iter().take_while(|v| v.is_ascii_digit()).count();
            let nb = b.iter().take_while(|v| v.is_ascii_digit()).count();
            let (x, y) = (&a[..na], &b[..nb]);
            let mut x = x;
            let mut y = y;
            while x.len() > 1 && x[0] == b'0' {
                x = &x[1..];
            }
            while y.len() > 1 && y[0] == b'0' {
                y = &y[1..];
            }
            let result = x.len().cmp(&y.len()).then(x.cmp(y));
            a = &a[na..];
            b = &b[nb..];
            result
        } else {
            let result = a[0].cmp(&b[0]);
            a = &a[1..];
            b = &b[1..];
            result
        };
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    a.len().cmp(&b.len())
}
fn safe_boot_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._+-".contains(&b))
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
}
/// Verify the exact mounted filesystem before any native package or Rustaller
/// boot writes. A directory named /boot on the root filesystem is never enough.
pub fn verify_mount(runner: &mut impl CommandRunner, root: &Path, boot_uuid: &str) -> Result<()> {
    if !crate::shared::storage::valid_uuid(boot_uuid) {
        return Err("invalid FAT UUID".into());
    }
    let boot = root.join("boot");
    validate_planned_file(&boot.join(".rustaller-mount-check")).map_err(|e| e.to_string())?;
    let mount = capture_checked(
        runner,
        &cmd(
            "findmnt",
            [
                "-nro",
                "FSTYPE,TARGET,UUID,FSROOT,SOURCE",
                "--mountpoint",
                boot.to_str().ok_or("invalid boot root")?,
            ],
        ),
    )
    .map_err(|e| e.to_string())?;
    let mount = String::from_utf8(mount).map_err(|_| "invalid mount facts")?;
    let mut words = mount.split_whitespace();
    if words.next() != Some("vfat")
        || words.next() != boot.to_str()
        || !words
            .next()
            .is_some_and(|v| v.eq_ignore_ascii_case(boot_uuid))
        || words.next() != Some("/")
    {
        return Err("FAT32 /boot must be mounted with its recorded UUID before boot writes".into());
    }
    let source = words.next().ok_or("missing FAT device")?;
    if words.next().is_some()
        || !source.strip_prefix("/dev/").is_some_and(|n| {
            !n.is_empty()
                && n.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        })
    {
        return Err("invalid FAT partition source".into());
    }
    // Linux exposes FAT12/16/32 under the same vfat mount type. Read the
    // partition boot sector without opening it for writes or following a label.
    let sector = capture_checked(
        runner,
        &cmd(
            "dd",
            [format!("if={source}"), "bs=512".into(), "count=1".into()],
        ),
    )
    .map_err(|e| e.to_string())?;
    if sector.len() != 512
        || sector[510..] != [0x55, 0xaa]
        || sector[17..19] != [0, 0]
        || sector[22..24] != [0, 0]
        || sector[36..40] == [0; 4]
        || u32::from_le_bytes(sector[44..48].try_into().unwrap()) < 2
    {
        return Err("mounted /boot partition has no FAT32 boot record".into());
    }
    let serial = u32::from_le_bytes(sector[67..71].try_into().unwrap());
    if !format!("{:04X}-{:04X}", serial >> 16, serial & 0xffff).eq_ignore_ascii_case(boot_uuid) {
        return Err("FAT32 boot record does not match recorded UUID".into());
    }
    Ok(())
}

struct BootState {
    firmware: BootMode,
    uuid: String,
    filesystem: String,
    boot_uuid: String,
}

fn read_state(root: &Path) -> Result<BootState> {
    let bytes = regular(root, &root.join("etc/rustaller/boot-state"))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "invalid boot state")?;
    let lines: Vec<_> = text.lines().collect();
    if lines.len() != 4
        || !crate::shared::storage::valid_uuid(lines[1])
        || !matches!(lines[2], "xfs" | "ext4" | "btrfs")
        || !crate::shared::storage::valid_uuid(lines[3])
    {
        return Err("invalid boot state".into());
    }
    let firmware = match lines[0] {
        NATIVE_EFI_STATE => BootMode::Efi,
        BIOS_STATE => BootMode::Bios,
        _ => {
            return Err(
                "unsupported boot state; existing installations require their matching helper"
                    .into(),
            );
        }
    };
    Ok(BootState {
        firmware,
        uuid: lines[1].into(),
        filesystem: lines[2].into(),
        boot_uuid: lines[3].into(),
    })
}

/// Package pre-transaction and initramfs hooks use the same recorded identity.
pub fn check(runner: &mut impl CommandRunner, root: &Path) -> Result<()> {
    verify_mount(runner, root, &read_state(root)?.boot_uuid)
}
pub fn refresh(runner: &mut impl CommandRunner, root: &Path) -> Result<()> {
    refresh_inner(runner, root, false)
}
fn refresh_inner(runner: &mut impl CommandRunner, root: &Path, installing: bool) -> Result<()> {
    let state = read_state(root)?;
    verify_mount(runner, root, &state.boot_uuid)?;
    verify_loaders(root, state.firmware)?;
    let _lock = refresh_lock(root)?;
    let boot = root.join("boot");
    let active_toml = native_active(&boot, installing, state.firmware)?;
    cleanup_temporaries(&boot)?;
    if boot.join("rustaller").is_dir() {
        cleanup_temporaries(&boot.join("rustaller"))?;
    }
    let mut kernels: Vec<_> = fs::read_dir(&boot)
        .map_err(|e| e.to_string())?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    kernels.sort_by(|a, b| {
        kernel_order(
            &b.file_name().to_string_lossy(),
            &a.file_name().to_string_lossy(),
        )
    });
    let mut pairs = Vec::new();
    for entry in kernels {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(version) = name.strip_prefix("vmlinuz-") else {
            continue;
        };
        if version.ends_with(".old") {
            continue;
        }
        let initrd = [
            format!("initrd.img-{version}"),
            format!("initramfs-{version}.img"),
            format!("initramfs-{version}"),
        ]
        .into_iter()
        .map(|p| boot.join(p))
        .find(|p| p.is_file());
        let Some(initrd) = initrd else {
            return Err(format!(
                "kernel {name} has no matching initramfs; previous configuration retained"
            ));
        };
        pairs.push((
            version.to_owned(),
            checked_source(root, &entry.path())?,
            checked_source(root, &initrd)?,
        ));
    }
    if pairs.is_empty() || pairs.len() > 8 {
        return Err("require 1-8 complete kernel/initramfs pairs".into());
    }
    for (name, kernel, _) in &pairs {
        if !safe_boot_path(name) || name.contains('/') {
            return Err("unsafe kernel release name".into());
        }
        verify_kernel(&kernel.0, state.firmware)?;
    }
    let mut microcode = Vec::new();
    for name in ["intel-ucode.img", "amd-ucode.img"] {
        let path = boot.join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => microcode.push(checked_source(root, &path)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    // Flat native files are the only sources: never traverse our generation
    // directories, even though both sources and destinations now live on FAT.
    let needed = pairs.iter().map(|(_, k, i)| k.1 + i.1).sum::<u64>()
        + microcode.iter().map(|(_, n)| n).sum::<u64>();
    prune_old_generations(root, state.firmware)?;
    if boot.join(NATIVE_CONFIG).is_file()
        && let Some(active) = &active_toml
        && unchanged(&boot, active, &pairs, &microcode)?
    {
        return Ok(());
    }
    let capacity = capture_checked(runner, &cmd("df", ["-Pk", boot.to_str().unwrap()]))
        .map_err(|e| e.to_string())?;
    let available = String::from_utf8_lossy(&capacity)
        .lines()
        .last()
        .and_then(|v| v.split_whitespace().nth(3))
        .and_then(|v| v.parse::<u64>().ok())
        .and_then(|v| v.checked_mul(1024))
        .ok_or("cannot establish FAT free space")?;
    // Existing native files and BOTH referenced generations remain allocated.
    // Require the complete new generation plus metadata/recovery reserve.
    if available < needed.saturating_add(1024 * 1024) {
        return Err("FAT32 /boot lacks space for a complete new generation and recovery reserve; previous boot remains intact; free space or enlarge /boot".into());
    }
    let generation = format!(
        "rustaller-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    );
    let mut modules = Vec::new();
    for (i, (source, size)) in microcode.iter().enumerate() {
        let path = format!("{generation}/microcode-{i}");
        copy_generation(source, &boot.join(&path), *size)?;
        modules.push(path);
    }
    let mut entries = Vec::new();
    for (i, (version, kernel, initrd)) in pairs.into_iter().enumerate() {
        let k = format!("{generation}/kernel-{i}");
        let init = format!("{generation}/initrd-{i}");
        copy_generation(&kernel.0, &boot.join(&k), kernel.1)?;
        copy_generation(&initrd.0, &boot.join(&init), initrd.1)?;
        entries.push((version, k, init));
    }
    publish_toml(&boot, &state, active_toml.as_deref(), &entries, &modules)?;
    verify(root)?;
    prune_old_generations(root, state.firmware)
}

pub fn verify(root: &Path) -> Result<()> {
    let state = read_state(root)?;
    let boot = root.join("boot");
    let text = config_text(&boot, NATIVE_CONFIG)?.ok_or("active boot.toml missing")?;
    validate_toml(&boot, &text, state.firmware)?;
    verify_loaders(root, state.firmware)
}
fn config_text(boot: &Path, name: &str) -> Result<Option<String>> {
    let path = boot.join(name);
    validate_planned_file(&path).map_err(|e| e.to_string())?;
    match fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(m) if m.len() <= rustaller_boot_core::MAX_CONFIG_BYTES as u64 => {
            fs::read_to_string(path)
                .map(Some)
                .map_err(|e| e.to_string())
        }
        Ok(_) => Err("boot configuration exceeds 32 KiB".into()),
    }
}

fn validate_toml(boot: &Path, text: &str, mode: BootMode) -> Result<rustaller_boot_core::Config> {
    let cfg = rustaller_boot_core::Config::parse(text).map_err(|e| e.to_string())?;
    for entry in &cfg.entries {
        for path in std::iter::once(&entry.kernel).chain(&entry.initrds) {
            checked_source(boot, &boot.join(&path[1..]))?;
        }
        verify_kernel(&boot.join(&entry.kernel[1..]), mode)?;
    }
    Ok(cfg)
}

fn native_active(boot: &Path, installing: bool, mode: BootMode) -> Result<Option<String>> {
    if let Some(active) = config_text(boot, NATIVE_CONFIG)? {
        validate_toml(boot, &active, mode)?;
        return Ok(Some(active));
    }
    if let Some(previous) = config_text(boot, BACKUP_CONFIG)? {
        validate_toml(boot, &previous, mode)?;
        return Ok(Some(previous));
    }
    if installing {
        return Ok(None);
    }
    Err("boot.toml and its recovery copy are missing".into())
}

type Source = (PathBuf, u64);
type KernelPair = (String, Source, Source);

fn equal_source(source: &Source, saved: &Path) -> Result<bool> {
    use std::io::Read;
    let mut a = fs::File::open(&source.0).map_err(|e| e.to_string())?;
    let mut b = fs::File::open(saved).map_err(|e| e.to_string())?;
    if b.metadata().map_err(|e| e.to_string())?.len() != source.1 {
        return Ok(false);
    }
    let mut x = [0u8; 65536];
    let mut y = [0u8; 65536];
    loop {
        let n = a.read(&mut x).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(b.read(&mut y[..1]).map_err(|e| e.to_string())? == 0);
        }
        b.read_exact(&mut y[..n]).map_err(|e| e.to_string())?;
        if x[..n] != y[..n] {
            return Ok(false);
        }
    }
}
fn entry_name(index: usize) -> String {
    if index == 0 {
        "current".into()
    } else {
        format!("current-{}", index + 1)
    }
}
fn unchanged(boot: &Path, text: &str, pairs: &[KernelPair], microcode: &[Source]) -> Result<bool> {
    let cfg = rustaller_boot_core::Config::parse(text).map_err(|e| e.to_string())?;
    if pairs.len()
        != cfg
            .entries
            .iter()
            .filter(|e| e.name.starts_with("current"))
            .count()
    {
        return Ok(false);
    }
    for (i, (version, kernel, initrd)) in pairs.iter().enumerate() {
        let Some(entry) = cfg.entry(&entry_name(i)) else {
            return Ok(false);
        };
        if entry.version.as_deref() != Some(version)
            || entry.initrds.len() != microcode.len() + 1
            || !equal_source(kernel, &boot.join(&entry.kernel[1..]))?
        {
            return Ok(false);
        }
        for (src, path) in microcode
            .iter()
            .chain(std::iter::once(initrd))
            .zip(&entry.initrds)
        {
            if !equal_source(src, &boot.join(&path[1..]))? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn publish_toml(
    boot: &Path,
    state: &BootState,
    active: Option<&str>,
    pairs: &[(String, String, String)],
    modules: &[String],
) -> Result<()> {
    use rustaller_boot_core::{Entry, create, replace_entries};
    let mut entries: Vec<_> = pairs
        .iter()
        .enumerate()
        .map(|(i, (version, k, rd))| Entry {
            name: entry_name(i),
            version: Some(version.clone()),
            kernel: format!("/{k}"),
            initrds: modules
                .iter()
                .chain(std::iter::once(rd))
                .map(|p| format!("/{p}"))
                .collect(),
        })
        .collect();
    let text = if let Some(old) = active {
        let cfg = validate_toml(boot, old, state.firmware)?;
        let current: Vec<_> = cfg
            .entries
            .into_iter()
            .filter(|e| e.name.starts_with("current"))
            .collect();
        let previous: Vec<_> = current
            .iter()
            .cloned()
            .map(|mut e| {
                e.name = e.name.replacen("current", "previous", 1);
                e
            })
            .collect();
        entries.extend(previous.iter().cloned());
        let text = replace_entries(old, &entries).map_err(|e| e.to_string())?;
        validate_toml(boot, &text, state.firmware)?;
        // Both names in this recovery document refer to the same intact older
        // generation. This also preserves a user's explicit default="previous".
        let recovery: Vec<_> = current.into_iter().chain(previous).collect();
        let backup = replace_entries(old, &recovery).map_err(|e| e.to_string())?;
        fat_replace(&boot.join(BACKUP_CONFIG), backup.as_bytes())?;
        text
    } else {
        create(&format!("UUID={}", state.uuid), &state.filesystem, &entries)
            .map_err(|e| e.to_string())?
    };
    validate_toml(boot, &text, state.firmware)?;
    sync_directory(boot)?;
    fat_replace(&boot.join(NATIVE_CONFIG), text.as_bytes())
}

fn verify_loaders(root: &Path, mode: BootMode) -> Result<()> {
    match mode {
        BootMode::Bios => Ok(()), // Raw stages are read back from disk during installation.
        BootMode::Efi => super::install::verify_efi_loader(&root.join("boot/EFI/BOOT/BOOTX64.EFI"))
            .map_err(|e| e.to_string()),
    }
}
/// Linux EFI stub images are EFI applications that also carry the x86 Linux
/// setup header. A random PE executable is not a bootable Linux generation.
fn verify_kernel(path: &Path, mode: BootMode) -> Result<()> {
    use std::io::Read;
    let mut header = [0u8; 4096];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .map_err(|_| "truncated Linux EFI stub")?;
    if !rustaller_boot_core::valid_bzimage(&header) {
        return Err("kernel is not a supported relocatable x86-64 bzImage".into());
    }
    if mode == BootMode::Efi && !rustaller_boot_core::valid_efi_stub(&header) {
        return Err("kernel lacks x86-64 Linux EFI stub with LoadFile2 support".into());
    }
    Ok(())
}
/// FAT has no chmod, hardlinks or symlinks: create an ordinary new file with
/// mount-controlled permissions, sync its bytes, then rename and sync the dir.
fn fat_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    validate_planned_file(path).map_err(|e| e.to_string())?;
    crate::shared::configure::ensure_parent(path).map_err(|e| e.to_string())?;
    let parent = path.parent().ok_or("missing FAT parent")?;
    let temporary = parent.join(format!(
        ".rustaller-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temporary, path).map_err(|e| e.to_string())?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|v| v.sync_all())
        .map_err(|e| e.to_string())
}
struct RefreshLock(fs::File);
impl Drop for RefreshLock {
    fn drop(&mut self) {
        // Release explicitly. A concurrent fork can briefly inherit the open
        // file description before exec closes its CLOEXEC descriptors.
        let _ = self.0.unlock();
    }
}
fn refresh_lock(root: &Path) -> Result<RefreshLock> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = root.join("etc/rustaller/boot-refresh.lock");
    validate_planned_file(&path).map_err(|e| e.to_string())?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => "another boot refresh is running".into(),
        std::fs::TryLockError::Error(e) => format!("cannot lock boot generation: {e}"),
    })?;
    Ok(RefreshLock(file))
}
fn checked_source(root: &Path, path: &Path) -> Result<(PathBuf, u64)> {
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !path.starts_with(root) {
        return Err("boot source escapes root".into());
    }
    validate_planned_file(&path).map_err(|e| e.to_string())?;
    let n = fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if n == 0 || n > 512 * 1024 * 1024 {
        return Err("invalid kernel/initramfs length".into());
    }
    Ok((path, n))
}
fn copy_generation(source: &Path, destination: &Path, size: u64) -> Result<()> {
    use std::{
        io::{Read, Seek},
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    validate_planned_file(destination).map_err(|e| e.to_string())?;
    crate::shared::configure::ensure_parent(destination).map_err(|e| e.to_string())?;
    let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
    let before = input.metadata().map_err(|e| e.to_string())?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(destination)
        .map_err(|e| e.to_string())?;
    let copied = std::io::copy(&mut std::io::Read::take(&mut input, size + 1), &mut output)
        .map_err(|e| e.to_string())?;
    if copied != size || input.metadata().map_err(|e| e.to_string())?.len() != size {
        return Err("kernel changed during refresh".into());
    }
    output.sync_all().map_err(|e| e.to_string())?;
    // FAT timestamps can have coarse resolution. Compare the complete source
    // again so a same-length rewrite during copy cannot silently publish torn
    // bytes merely because size and timestamps still happen to match.
    input.rewind().map_err(|e| e.to_string())?;
    let mut copied = fs::File::open(destination).map_err(|e| e.to_string())?;
    let mut left = [0u8; 65536];
    let mut right = [0u8; 65536];
    let mut remaining = size;
    while remaining > 0 {
        let n = remaining.min(left.len() as u64) as usize;
        input
            .read_exact(&mut left[..n])
            .map_err(|e| e.to_string())?;
        copied
            .read_exact(&mut right[..n])
            .map_err(|e| e.to_string())?;
        if left[..n] != right[..n] {
            return Err("boot source changed during refresh".into());
        }
        remaining -= n as u64;
    }
    let after = input.metadata().map_err(|e| e.to_string())?;
    if before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
    {
        return Err("boot source changed during refresh".into());
    }
    fs::File::open(destination.parent().unwrap())
        .and_then(|v| v.sync_all())
        .map_err(|e| e.to_string())
}
fn managed_temporary(name: &str) -> bool {
    name.strip_prefix(".rustaller-")
        .and_then(|v| v.strip_suffix(".tmp"))
        .and_then(|v| v.split_once('-'))
        .is_some_and(|(pid, sequence)| {
            !pid.is_empty()
                && !sequence.is_empty()
                && pid
                    .bytes()
                    .chain(sequence.bytes())
                    .all(|v| v.is_ascii_digit())
        })
}
fn cleanup_temporaries(boot: &Path) -> Result<()> {
    for entry in fs::read_dir(boot).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if managed_temporary(&entry.file_name().to_string_lossy()) {
            validate_planned_file(&entry.path()).map_err(|e| e.to_string())?;
            fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
fn managed_generation(name: &str) -> bool {
    name.strip_prefix("rustaller-")
        .and_then(|s| s.split_once('-'))
        .is_some_and(|(pid, time)| {
            !pid.is_empty()
                && !time.is_empty()
                && pid.bytes().chain(time.bytes()).all(|v| v.is_ascii_digit())
        })
}
fn prune_old_generations(root: &Path, mode: BootMode) -> Result<()> {
    let boot = root.join("boot");
    let mut retained = std::collections::BTreeSet::new();
    for path in [NATIVE_CONFIG, BACKUP_CONFIG] {
        if let Some(text) = config_text(&boot, path)? {
            let cfg = validate_toml(&boot, &text, mode)?;
            for entry in cfg.entries {
                for path in std::iter::once(entry.kernel).chain(entry.initrds) {
                    if let Some(name) = path[1..].split('/').next() {
                        retained.insert(name.to_owned());
                    }
                }
            }
        }
    }
    for entry in fs::read_dir(&boot).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !managed_generation(&name) || retained.contains(name.as_ref()) {
            continue;
        }
        if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            return Err("unsafe stale boot generation".into());
        }
        let files: Vec<_> = fs::read_dir(entry.path())
            .map_err(|e| e.to_string())?
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        for file in &files {
            let n = file.file_name();
            let n = n.to_string_lossy();
            if !managed_temporary(&n)
                && !["kernel-", "initrd-", "microcode-"].iter().any(|p| {
                    n.strip_prefix(p)
                        .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
                })
            {
                return Err("unexpected generation file; manual recovery required".into());
            }
            validate_planned_file(&file.path()).map_err(|e| e.to_string())?;
        }
        for file in files {
            fs::remove_file(file.path()).map_err(|e| e.to_string())?;
        }
        fs::remove_dir(entry.path()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_stream_copy_cannot_publish_over_an_existing_payload() {
        let d = crate::test_support::Scratch::new("boot-copy");
        d.write("source", "kernel bytes");
        let source = d.0.join("source");
        let destination = d.0.join("new/kernel");
        assert!(copy_generation(&source, &destination, 1).is_err());
        assert!(fs::metadata(&destination).unwrap().len() <= 2);
        assert!(
            copy_generation(&source, &destination, 12).is_err(),
            "create_new never overwrites a partial generation"
        );
        assert_eq!(d.read("source"), "kernel bytes");
    }
}
