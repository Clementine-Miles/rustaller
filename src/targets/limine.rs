//! Limine 9.6.5: authenticated native installer, FAT boot artifacts and Rust refresh hooks.
use crate::command::{CommandInput, CommandRunner, CommandSpec, capture_checked, run_checked};
use crate::fsutil::atomic_replace;
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
    for (source, destination, mode) in [
        ("limine-bios.sys", "efi/limine-bios.sys", 0o600),
        ("BOOTX64.EFI", "efi/EFI/BOOT/BOOTX64.EFI", 0o600),
        ("limine", "usr/local/libexec/limine", 0o755),
        ("LICENSE", "usr/share/licenses/limine/LICENSE", 0o644),
    ] {
        let bytes = fs::read(work.join(source)).map_err(|e| e.to_string())?;
        write_config::<crate::shared::configure::ConfigurationError>(
            &root.join(destination),
            &bytes,
            mode,
        )
        .map_err(|e| e.to_string())?;
    }
    if layout.boot_mode() == BootMode::Bios {
        run_checked(runner, &bios_command(layout, &work.join("limine"))?)
            .map_err(|e| e.to_string())?;
    }
    let state = format!(
        "RUSTALLER-LIMINE-{VERSION}\n{uuid}\n{}\n{boot_uuid}\n",
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
            "etc/kernel/postinst.d/zz-rustaller-limine",
            "etc/kernel/postrm.d/zz-rustaller-limine",
            "etc/initramfs/post-update.d/zz-rustaller-limine",
        ],
        Target::Alpine => &["etc/apk/commit_hooks.d/zz-rustaller-limine"],
        Target::Gentoo => &["etc/kernel/postinst.d/zz-rustaller-limine"],
        Target::Arch => &[],
        Target::Reset => return Err("installation target required".into()),
    };
    write_config::<crate::shared::configure::ConfigurationError>(&root.join(WORKER), worker, 0o755)
        .map_err(|e| e.to_string())?;
    for name in links {
        let path = root.join(name);
        validate_planned_file(&path).map_err(|e| e.to_string())?;
        crate::shared::configure::ensure_parent(&path).map_err(|e| e.to_string())?;
        symlink(format!("/{WORKER}"), &path).map_err(|e| e.to_string())?;
    }
    if target == Target::Arch {
        write_config::<crate::shared::configure::ConfigurationError>(&root.join("etc/pacman.d/hooks/zz-rustaller-limine.hook"),b"[Trigger]\nOperation=Install\nOperation=Upgrade\nOperation=Remove\nType=Path\nTarget=boot/*\nTarget=usr/lib/modules/*/pkgbase\nTarget=usr/lib/initcpio/*\n[Action]\nDescription=Refresh Limine Linux boot artifacts\nWhen=PostTransaction\nExec=/usr/local/libexec/rustaller-limine-worker limine-refresh\n",0o644).map_err(|e|e.to_string())?;
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
pub fn refresh(runner: &mut impl CommandRunner, root: &Path) -> Result<()> {
    let state = regular(root, &root.join("etc/rustaller/boot-state"))?;
    let text = std::str::from_utf8(&state).map_err(|_| "invalid boot state")?;
    let mut lines = text.lines();
    if lines.next() != Some(&format!("RUSTALLER-LIMINE-{VERSION}")) {
        return Err("unsupported boot state".into());
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
    // Never copy into an unmounted /efi directory on a running system.
    let boot = root.join("efi");
    validate_planned_file(&boot.join("limine.conf")).map_err(|e| e.to_string())?;
    verify_loaders(root)?;
    let mount = capture_checked(
        runner,
        &cmd(
            "findmnt",
            [
                "-nro",
                "FSTYPE,TARGET,UUID",
                "--target",
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
        return Err("FAT32 /efi must be mounted with its recorded UUID before boot refresh".into());
    }
    let mut kernels: Vec<_> = fs::read_dir(root.join("boot"))
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
        // installkernel keeps same-version backup files with an .old suffix.
        if version.ends_with(".old") {
            continue;
        }
        let initrd = [
            format!("initrd.img-{version}"),
            format!("initramfs-{version}.img"),
            format!("initramfs-{version}"),
        ]
        .into_iter()
        .map(|p| root.join("boot").join(p))
        .find(|p| p.is_file());
        let Some(initrd) = initrd else {
            return Err(format!("kernel {name} has no matching initramfs"));
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
    // Validate the complete configuration before creating any new generation.
    render(
        uuid,
        filesystem,
        &pairs
            .iter()
            .map(|(name, _, _)| {
                (
                    name.clone(),
                    "generation/kernel".into(),
                    "generation/initrd".into(),
                )
            })
            .collect::<Vec<_>>(),
    )?;
    let mut microcode = Vec::new();
    for name in ["intel-ucode.img", "amd-ucode.img"] {
        let path = root.join("boot").join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => microcode.push(checked_source(root, &path)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    let needed: u64 = pairs.iter().map(|(_, k, i)| k.1 + i.1).sum::<u64>()
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
    if available < needed + 1024 * 1024 {
        return Err("FAT boot partition lacks space for a safe new generation; retain previous generation and free space or enlarge /efi".into());
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
    let config = render_with_microcode(uuid, filesystem, &entries, &modules)?;
    atomic_replace(&boot.join("limine.conf"), config.as_bytes(), 0o600)
        .map_err(|e| e.to_string())?;
    verify(root)
}
pub fn verify(root: &Path) -> Result<()> {
    let config = regular(root, &root.join("efi/limine.conf"))?;
    let text = std::str::from_utf8(&config).map_err(|_| "invalid Limine configuration")?;
    if !text.starts_with("timeout: 3\nserial: yes\n") || !text.contains("protocol: linux") {
        return Err("invalid Limine configuration".into());
    }
    for line in text.lines() {
        if let Some(path) = line
            .trim()
            .strip_prefix("path: boot():/")
            .or_else(|| line.trim().strip_prefix("module_path: boot():/"))
        {
            if !safe_boot_path(path) {
                return Err("unsafe published boot path".into());
            }
            checked_source(&root.join("efi"), &root.join("efi").join(path))?;
        }
    }
    verify_loaders(root)
}
fn verify_loaders(root: &Path) -> Result<()> {
    regular(root, &root.join("efi/limine-bios.sys"))?;
    let efi = regular(root, &root.join("efi/EFI/BOOT/BOOTX64.EFI"))?;
    super::install::verify_efi_loader(&root.join("efi/EFI/BOOT/BOOTX64.EFI"))
        .map_err(|e| e.to_string())?;
    if !efi.starts_with(b"MZ") {
        return Err("invalid Limine EFI image".into());
    }
    Ok(())
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
    use std::os::unix::fs::OpenOptionsExt;
    validate_planned_file(destination).map_err(|e| e.to_string())?;
    crate::shared::configure::ensure_parent(destination).map_err(|e| e.to_string())?;
    let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
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
    fs::File::open(destination.parent().unwrap())
        .and_then(|v| v.sync_all())
        .map_err(|e| e.to_string())
}
fn prune_old_generations(root: &Path) -> Result<()> {
    let boot = root.join("efi");
    let current = match fs::read_to_string(boot.join("limine.conf")) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    for entry in fs::read_dir(&boot).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name
            .strip_prefix("rustaller-")
            .and_then(|s| s.split_once('-'))
            .is_some_and(|(pid, time)| {
                !pid.is_empty()
                    && !time.is_empty()
                    && pid.bytes().chain(time.bytes()).all(|v| v.is_ascii_digit())
            })
            || current.contains(&format!("boot():/{name}/"))
        {
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
            if !["kernel-", "initrd-", "microcode-"].iter().any(|p| {
                n.strip_prefix(p)
                    .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            }) {
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
