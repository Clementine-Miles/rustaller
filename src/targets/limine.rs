//! Limine 9.6.5: authenticated native installer, FAT boot artifacts and Rust refresh hooks.
use crate::command::{CommandInput, CommandRunner, CommandSpec, capture_checked, run_checked};
use crate::model::{BootMode, Target};
use crate::shared::configure::{validate_planned_file, validate_root, write_config};
use crate::shared::storage::{DiskLayout, PartitionRole};
use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, String>;
pub const VERSION: &str = "9.6.5";
const COMMIT: &str = "81a11885edab8e59fdad2617b4ef6f35abe44e44";
const FILES: &[(&str, &str)] = &[
    (
        "LICENSE",
        "45a0d60afc1cdef1d531d788cd53680261cff75fc14e7d96f44dba68a2f56b8d",
    ),
    (
        "limine.c",
        "668eef8ff649d8d61978930b5dce5bf6bf4092dae8ca1a85d710fccf31d3b503",
    ),
    (
        "limine-bios-hdd.h",
        "7d983c34e595642fa3d0b2bf3c4c75cf5685bacc07cf6e91c9894e444d66b38e",
    ),
    (
        "limine-bios.sys",
        "8385e03cc2d14348a17667cf0bd97b6e8ca0a6dec991808b599feb67600491a4",
    ),
    (
        "BOOTX64.EFI",
        "bb2f1c0b3bfad90823d8d9b883a026fa685506d9c797b743ce55c162f78efd68",
    ),
];
const WORKER: &str = "usr/local/libexec/rustaller-limine-worker";
fn cmd(program: &str, args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>) -> CommandSpec {
    CommandSpec::new(program, args)
        .phase("limine")
        .stdin(CommandInput::Null)
}
pub fn prepare(runner: &mut impl CommandRunner, work: &Path) -> Result<()> {
    validate_root(work).map_err(|e| e.to_string())?;
    for (name, digest) in FILES {
        let url =
            format!("https://raw.githubusercontent.com/limine-bootloader/limine/{COMMIT}/{name}");
        let bytes = capture_checked(
            runner,
            &cmd(
                "curl",
                [
                    "--disable",
                    "-fsSL",
                    "--proto",
                    "=https",
                    "--proto-redir",
                    "=https",
                    "--max-time",
                    "120",
                    "--max-filesize",
                    "4194304",
                    &url,
                ],
            ),
        )
        .map_err(|e| e.to_string())?;
        if bytes.is_empty() || bytes.len() > 4 * 1024 * 1024 {
            return Err("invalid Limine resource size".into());
        }
        let hash = capture_checked(
            runner,
            &cmd("sha256sum", [] as [&str; 0]).stdin(CommandInput::Bytes(bytes.clone())),
        )
        .map_err(|e| e.to_string())?;
        if String::from_utf8_lossy(&hash).split_whitespace().next() != Some(digest) {
            return Err(format!("Limine digest mismatch: {name}"));
        }
        write_config::<crate::shared::configure::ConfigurationError>(
            &work.join(name),
            &bytes,
            0o600,
        )
        .map_err(|e| e.to_string())?;
    }
    run_checked(
        runner,
        &cmd(
            "cc",
            ["-static", "-O2", "-std=c99", "limine.c", "-o", "limine"],
        )
        .cwd(work),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
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
    for (source, destination, mode) in [
        ("limine-bios.sys", "boot/limine-bios.sys", 0o600),
        ("BOOTX64.EFI", "boot/EFI/BOOT/BOOTX64.EFI", 0o600),
        ("limine", "usr/local/libexec/limine", 0o755),
        ("LICENSE", "usr/share/licenses/limine/LICENSE", 0o644),
    ] {
        let bytes = fs::read(work.join(source)).map_err(|e| e.to_string())?;
        if destination.starts_with("boot/") {
            fat_replace(&root.join(destination), &bytes)?;
        } else {
            write_config::<crate::shared::configure::ConfigurationError>(
                &root.join(destination),
                &bytes,
                mode,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if layout.boot_mode() == BootMode::Bios {
        run_checked(runner, &bios_command(layout, &work.join("limine"))?)
            .map_err(|e| e.to_string())?;
    }
    let state = format!(
        "RUSTALLER-LIMINE-{VERSION}-BOOT-1\n{uuid}\n{}\n{boot_uuid}\n",
        layout.root_filesystem().as_str()
    );
    write_config::<crate::shared::configure::ConfigurationError>(
        &root.join("etc/rustaller/boot-state"),
        state.as_bytes(),
        0o600,
    )
    .map_err(|e| e.to_string())?;
    let worker = fs::read("/rustaller-worker").map_err(|e| e.to_string())?;
    install_hooks(root, layout.target(), &worker)?;
    refresh(runner, root)
}
pub fn bios_command(layout: &DiskLayout, executable: &Path) -> Result<CommandSpec> {
    if layout.boot_mode() != BootMode::Bios {
        return Err("BIOS installer requires BIOS firmware".into());
    }
    let mut args = vec!["bios-install".to_owned(), layout.device()];
    if let Some(number) = layout.role_number(PartitionRole::BiosBoot) {
        args.push(number.to_string());
    }
    Ok(CommandSpec::new(executable, args)
        .phase("limine-bios-install")
        .stdin(CommandInput::Null))
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
            "etc/kernel/preinst.d/zz-rustaller-limine-check",
            "etc/kernel/prerm.d/zz-rustaller-limine-check",
            "etc/initramfs-tools/hooks/zz-rustaller-limine-check",
            "etc/kernel/postinst.d/zz-rustaller-limine",
            "etc/kernel/postrm.d/zz-rustaller-limine",
            "etc/initramfs/post-update.d/zz-rustaller-limine",
        ],
        Target::Alpine => &["etc/apk/commit_hooks.d/zz-rustaller-limine"],
        Target::Gentoo => &[
            "etc/kernel/preinst.d/zz-rustaller-limine-check",
            "etc/kernel/postinst.d/zz-rustaller-limine",
        ],
        Target::Arch => &["etc/initcpio/post/zz-rustaller-limine"],
        Target::Reset => return Err("installation target required".into()),
    };
    write_config::<crate::shared::configure::ConfigurationError>(&root.join(WORKER), worker, 0o755)
        .map_err(|e| e.to_string())?;
    for name in links {
        let path = root.join(name);
        validate_planned_file(&path).map_err(|e| e.to_string())?;
        crate::shared::configure::ensure_parent(&path).map_err(|e| e.to_string())?;
        if *name == "etc/initramfs-tools/hooks/zz-rustaller-limine-check" {
            // initramfs-tools runs `sh -n` before accepting hooks. An ELF
            // symlink is rejected as shell syntax and silently skipped.
            write_config::<crate::shared::configure::ConfigurationError>(
                &path,
                b"#!/bin/sh\ncase \"${1:-}\" in prereqs) exit 0 ;; esac\nexec /usr/local/libexec/rustaller-limine-worker limine-check\n",
                0o755,
            ).map_err(|e| e.to_string())?;
        } else {
            symlink(format!("/{WORKER}"), &path).map_err(|e| e.to_string())?;
        }
    }
    install_generator_guards(root, target)?;
    if target == Target::Arch {
        write_config::<crate::shared::configure::ConfigurationError>(&root.join("etc/pacman.d/hooks/00-rustaller-boot-mount.hook"), b"[Trigger]\nOperation=Install\nOperation=Upgrade\nOperation=Remove\nType=Path\nTarget=boot/*\nTarget=usr/lib/modules/*\n[Action]\nDescription=Verify Rustaller FAT32 boot mount\nWhen=PreTransaction\nExec=/usr/local/libexec/rustaller-limine-worker limine-check\nAbortOnFail\n", 0o644).map_err(|e|e.to_string())?;
        write_config::<crate::shared::configure::ConfigurationError>(&root.join("etc/pacman.d/hooks/zz-rustaller-limine.hook"),b"[Trigger]\nOperation=Install\nOperation=Upgrade\nOperation=Remove\nType=Path\nTarget=boot/*\nTarget=usr/lib/modules/*/pkgbase\nTarget=usr/lib/initcpio/*\n[Action]\nDescription=Refresh Limine Linux boot artifacts\nWhen=PostTransaction\nExec=/usr/local/libexec/rustaller-limine-worker limine-refresh\n",0o644).map_err(|e|e.to_string())?;
    }
    Ok(())
}
/// Native generators source these configuration files before touching output.
/// Keep the distribution configuration and add only a bounded managed block.
fn install_generator_guards(root: &Path, target: Target) -> Result<()> {
    const CHECK: &str = "/usr/local/libexec/rustaller-limine-worker limine-check || exit 1\n";
    let (relative, mut guard) = match target {
        Target::Arch => ("etc/mkinitcpio.conf", CHECK.to_owned()),
        // mkinitramfs sources this before touching its output. Its ordinary
        // hooks run later, after an output file may already have been created.
        Target::Debian => ("etc/initramfs-tools/initramfs.conf", CHECK.to_owned()),
        Target::Gentoo => (
            "etc/dracut.conf.d/00-rustaller-mount.conf",
            CHECK.to_owned(),
        ),
        Target::Alpine => (
            "etc/mkinitfs/mkinitfs.conf",
            format!(
                "{CHECK}if [ -z \"${{list_features:-}}${{list_sources:-}}${{DEBUG_KMOD:-}}\" ]; then\n trap 'rustaller_status=$?; if [ \"$rustaller_status\" -eq 0 ]; then /usr/local/libexec/rustaller-limine-worker limine-refresh || exit 1; fi; exit \"$rustaller_status\"' EXIT\nfi\n"
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
/usr/local/libexec/rustaller-limine-worker limine-check || exit 1
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
 exec /usr/local/libexec/rustaller-limine-worker limine-refresh
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
        return Err("oversized Limine metadata or firmware payload".into());
    }
    fs::read(resolved).map_err(|e| e.to_string())
}
pub fn render(uuid: &str, fs: &str, entries: &[(String, String, String)]) -> Result<String> {
    render_with_microcode(uuid, fs, entries, &[])
}
fn render_with_microcode(
    uuid: &str,
    fs: &str,
    entries: &[(String, String, String)],
    microcode: &[String],
) -> Result<String> {
    if !crate::shared::storage::valid_uuid(uuid)
        || !matches!(fs, "xfs" | "ext4" | "btrfs")
        || entries.is_empty()
    {
        return Err("invalid Limine boot plan".into());
    }
    let flags = if fs == "btrfs" {
        " rootflags=subvol=@"
    } else {
        ""
    };
    let mut text = "timeout: 3\nserial: yes\nserial_baudrate: 115200\n".to_owned();
    for (name, kernel, initrd) in entries {
        if name.contains('/') || [name, kernel, initrd].iter().any(|s| !safe_boot_path(s)) {
            return Err("unsafe Limine entry path".into());
        }
        text.push_str(&format!(
            "/{name}\n protocol: linux\n path: boot():/{kernel}\n"
        ));
        for module in microcode {
            if !safe_boot_path(module) {
                return Err("unsafe microcode path".into());
            }
            text.push_str(&format!(" module_path: boot():/{module}\n"));
        }
        text.push_str(&format!(" module_path: boot():/{initrd}\n cmdline: root=UUID={uuid} rootfstype={fs}{flags} rw {}\n",super::install::TTY_COMMAND_LINE));
    }
    Ok(text)
}
/// Numeric runs avoid booting 6.9 ahead of an installed 6.10 after an update.
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
    validate_planned_file(&boot.join("limine.conf")).map_err(|e| e.to_string())?;
    let mount = capture_checked(
        runner,
        &cmd(
            "findmnt",
            [
                "-nro",
                "FSTYPE,TARGET,UUID",
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
        || words.next().is_some()
    {
        return Err("FAT32 /boot must be mounted with its recorded UUID before boot writes".into());
    }
    Ok(())
}

/// Package pre-transaction and initramfs hooks use the same recorded identity.
pub fn check(runner: &mut impl CommandRunner, root: &Path) -> Result<()> {
    let state = regular(root, &root.join("etc/rustaller/boot-state"))?;
    let text = std::str::from_utf8(&state).map_err(|_| "invalid boot state")?;
    let lines: Vec<_> = text.lines().collect();
    if lines.len() != 4
        || lines[0] != format!("RUSTALLER-LIMINE-{VERSION}-BOOT-1")
        || !crate::shared::storage::valid_uuid(lines[1])
        || !matches!(lines[2], "xfs" | "ext4" | "btrfs")
    {
        return Err("unsupported boot state (requires FAT32 /boot schema)".into());
    }
    verify_mount(runner, root, lines[3])
}
pub fn refresh(runner: &mut impl CommandRunner, root: &Path) -> Result<()> {
    let state = regular(root, &root.join("etc/rustaller/boot-state"))?;
    let text = std::str::from_utf8(&state).map_err(|_| "invalid boot state")?;
    let mut lines = text.lines();
    if lines.next() != Some(&format!("RUSTALLER-LIMINE-{VERSION}-BOOT-1")) {
        return Err("unsupported boot state (requires FAT32 /boot schema)".into());
    }
    let uuid = lines.next().ok_or("missing root UUID")?;
    let filesystem = lines.next().ok_or("missing root filesystem")?;
    let boot_uuid = lines
        .next()
        .filter(|v| crate::shared::storage::valid_uuid(v))
        .ok_or("missing or invalid FAT UUID")?;
    if lines.next().is_some() {
        return Err("invalid boot state".into());
    }
    verify_mount(runner, root, boot_uuid)?;
    verify_loaders(root)?;
    // flock is held on the Linux root, never on FAT (which lacks Unix locks).
    let _lock = refresh_lock(root)?;
    let boot = root.join("boot");
    recover_configuration(&boot)?;
    cleanup_temporaries(&boot)?;
    let previous = primary_configuration(&boot)?;
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
    render(
        uuid,
        filesystem,
        &pairs
            .iter()
            .map(|(n, _, _)| {
                (
                    n.clone(),
                    "generation/kernel".into(),
                    "generation/initrd".into(),
                )
            })
            .collect::<Vec<_>>(),
    )?;
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
    prune_old_generations(root)?;
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
    let current = render_with_microcode(uuid, filesystem, &entries, &modules)?;
    // Self-contained generation metadata permits recovery without guessing
    // which stale native file belongs to which installed kernel.
    fat_replace(
        &boot.join(&generation).join("limine.conf"),
        current.as_bytes(),
    )?;
    let mut combined = current;
    if let Some(previous) = previous {
        validate_configuration(&boot, &previous)?;
        fat_replace(&boot.join("limine.conf.previous"), previous.as_bytes())?;
        for line in previous.lines() {
            if let Some(name) = line.strip_prefix('/') {
                combined.push_str(&format!("/Previous-{name}\n"));
            } else if line.starts_with(' ') {
                combined.push_str(line);
                combined.push('\n');
            }
        }
    }
    validate_configuration(&boot, &combined)?;
    // FAT rename/fsync has weaker power-loss guarantees than journaled Linux
    // filesystems. Keep the independent previous config and payloads on disk.
    fat_replace(&boot.join("limine.conf"), combined.as_bytes())?;
    verify(root)?;
    prune_old_generations(root)
}

pub fn verify(root: &Path) -> Result<()> {
    let config = regular(root, &root.join("boot/limine.conf"))?;
    validate_configuration(
        &root.join("boot"),
        std::str::from_utf8(&config).map_err(|_| "invalid Limine configuration")?,
    )?;
    verify_loaders(root)
}
fn validate_configuration(boot: &Path, text: &str) -> Result<()> {
    if !text.starts_with("timeout: 3\nserial: yes\n") || !text.contains("protocol: linux") {
        return Err("invalid Limine configuration".into());
    }
    let mut kernels = 0usize;
    let mut initrds = 0usize;
    for line in text.lines() {
        if let Some(path) = line
            .trim()
            .strip_prefix("path: boot():/")
            .or_else(|| line.trim().strip_prefix("module_path: boot():/"))
        {
            if !safe_boot_path(path) {
                return Err("unsafe published boot path".into());
            }
            checked_source(boot, &boot.join(path))?;
            if line.trim().starts_with("path:") {
                kernels += 1;
            }
            if path
                .rsplit('/')
                .next()
                .is_some_and(|v| v.starts_with("initrd-"))
            {
                initrds += 1;
            }
        }
    }
    if kernels == 0 || kernels != initrds {
        return Err("incomplete published kernel/initramfs pairs".into());
    }
    Ok(())
}
fn primary_configuration(boot: &Path) -> Result<Option<String>> {
    let path = boot.join("limine.conf");
    let text = match fs::read_to_string(&path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    validate_configuration(boot, &text)?;
    let generation = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("path: boot():/"))
        .and_then(|p| p.split('/').next())
        .ok_or("missing generation")?;
    if !managed_generation(generation) {
        return Err("unmanaged boot configuration".into());
    }
    let bytes = regular(boot, &boot.join(generation).join("limine.conf"))?;
    let previous = String::from_utf8(bytes).map_err(|_| "invalid generation metadata")?;
    validate_configuration(boot, &previous)?;
    Ok(Some(previous))
}
fn recover_configuration(boot: &Path) -> Result<()> {
    let active = boot.join("limine.conf");
    validate_planned_file(&active).map_err(|e| e.to_string())?;
    if let Ok(text) = fs::read_to_string(&active) {
        if validate_configuration(boot, &text).is_ok() {
            return Ok(());
        }
    } else if !active.exists() && !boot.join("limine.conf.previous").exists() {
        return Ok(());
    }
    let previous = regular(boot, &boot.join("limine.conf.previous"))?;
    let text = std::str::from_utf8(&previous).map_err(|_| "invalid previous configuration")?;
    validate_configuration(boot, text)?;
    fat_replace(&active, &previous)
}
fn verify_loaders(root: &Path) -> Result<()> {
    regular(root, &root.join("boot/limine-bios.sys"))?;
    let efi = regular(root, &root.join("boot/EFI/BOOT/BOOTX64.EFI"))?;
    super::install::verify_efi_loader(&root.join("boot/EFI/BOOT/BOOTX64.EFI"))
        .map_err(|e| e.to_string())?;
    if !efi.starts_with(b"MZ") {
        return Err("invalid Limine EFI image".into());
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
fn refresh_lock(root: &Path) -> Result<fs::File> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
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
    // SAFETY: valid open descriptor, LOCK_EX|LOCK_NB, no borrowed pointer.
    if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
        return Err("another boot refresh is running".into());
    }
    Ok(file)
}
unsafe extern "C" {
    fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
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
fn prune_old_generations(root: &Path) -> Result<()> {
    let boot = root.join("boot");
    let mut current = match fs::read_to_string(boot.join("limine.conf")) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    if let Ok(previous) = fs::read_to_string(boot.join("limine.conf.previous")) {
        validate_configuration(&boot, &previous)?;
        current.push_str(&previous);
    }
    for entry in fs::read_dir(&boot).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !managed_generation(&name) || current.contains(&format!("boot():/{name}/")) {
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
            if n != "limine.conf"
                && !managed_temporary(&n)
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
