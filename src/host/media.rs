//! Confirmed, one-shot GRUB media boots. No disk installation or NVRAM writes.
use crate::command::{CommandInput, CommandRunner, CommandSpec, capture_checked, run_checked};
use crate::fsutil::atomic_replace;
use crate::model::{BootMode, InstallPlan, Target};
use crate::shared::configure::validate_planned_file;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

static NEXT_ISO: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn image_digest(runner: &mut impl CommandRunner, image: &Path) -> Result<String> {
    validate_planned_file(image).map_err(|e| e.to_string())?;
    let text = capture(
        runner,
        "sha256sum",
        &["--", image.to_str().ok_or("invalid ISO path")?],
    )?;
    let value = text.split_whitespace().next().ok_or("missing ISO digest")?;
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid ISO digest".into());
    }
    Ok(value.to_owned())
}
const ID: &str = "rustaller-media";
const ORIGIN: &str = "https://boot.netboot.xyz/ipxe";
type Result<T> = std::result::Result<T, String>;

pub struct MediaBoot {
    pub confirmation: String,
    pub firmware: BootMode,
    root: PathBuf,
    directory: PathBuf,
    prefix: &'static str,
    uuid: String,
    grub_path: String,
    config: Vec<u8>,
    custom: Option<Vec<u8>>,
    environment: Vec<u8>,
    custom_mode: u32,
    environment_mode: u32,
    reboot: CommandSpec,
    target: Target,
    iso: Option<super::iso::PreparedIso>,
    iso_digest: Option<String>,
}

fn command(
    program: impl AsRef<std::ffi::OsStr>,
    args: impl IntoIterator<Item = impl AsRef<std::ffi::OsStr>>,
) -> CommandSpec {
    CommandSpec::new(program, args)
        .phase("media-boot")
        .stdin(CommandInput::Null)
}
fn capture(runner: &mut impl CommandRunner, program: &str, args: &[&str]) -> Result<String> {
    let bytes = capture_checked(runner, &command(program, args)).map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("oversized boot-tool output".into());
    }
    String::from_utf8(bytes).map_err(|_| "invalid boot-tool text".into())
}
fn read(path: &Path) -> Result<Vec<u8>> {
    validate_planned_file(path).map_err(|e| e.to_string())?;
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > 1024 * 1024 {
        return Err("oversized GRUB configuration".into());
    }
    fs::read(path).map_err(|e| e.to_string())
}
fn optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(_) => read(path).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn discover(runner: &mut impl CommandRunner, plan: &InstallPlan) -> Result<MediaBoot> {
    super::preflight::collect_host_facts(runner, plan.target).map_err(|e| e.to_string())?;
    discover_at(runner, plan, Path::new("/"))
}

/// Explicit root for isolated fixtures; production dispatch always uses discover.
fn discover_at(
    runner: &mut impl CommandRunner,
    plan: &InstallPlan,
    root: &Path,
) -> Result<MediaBoot> {
    plan.validate().map_err(|e| e.to_string())?;
    if !matches!(plan.target, Target::NetbootXyz | Target::LocalIso) {
        return Err("unsupported media route".into());
    }
    let firmware = super::preflight::validate_source_boot(root).map_err(str::to_owned)?;
    if plan.force_boot_mode.is_some_and(|mode| mode != firmware) {
        return Err("media boot cannot change firmware mode".into());
    }
    let candidates: Vec<_> = ["boot/grub", "boot/grub2"]
        .into_iter()
        .map(|name| root.join(name))
        .filter(|dir| dir.join("grub.cfg").exists())
        .collect();
    if candidates.len() != 1 {
        return Err(
            "require one installed GRUB configuration under /boot/grub or /boot/grub2".into(),
        );
    }
    let directory = candidates[0].clone();
    if fs::symlink_metadata(directory.join(ID)).is_ok() {
        return Err("previous media staging remains: inspect its backups and clear next_entry before removing it".into());
    }
    let config = read(&directory.join("grub.cfg"))?;
    let text = std::str::from_utf8(&config).map_err(|_| "invalid GRUB configuration")?;
    for required in [
        "load_env",
        "save_env next_entry",
        "set next_entry=",
        "set default=\"${next_entry}\"",
        "custom.cfg",
    ] {
        if !text.contains(required) {
            return Err(format!(
                "GRUB configuration lacks required one-shot integration: {required}"
            ));
        }
    }
    if !(text.contains("source $prefix/custom.cfg")
        || text.contains("source ${config_directory}/custom.cfg"))
    {
        return Err("GRUB must source its adjacent custom.cfg".into());
    }
    let custom = optional(&directory.join("custom.cfg"))?;
    if custom
        .as_ref()
        .is_some_and(|v| v.windows(ID.len()).any(|w| w == ID.as_bytes()))
    {
        return Err("existing custom.cfg already owns the media entry".into());
    }
    let environment = read(&directory.join("grubenv"))?;
    if environment.len() != 1024 || !environment.starts_with(b"# GRUB Environment Block\n") {
        return Err("require a regular 1024-byte GRUB environment block".into());
    }
    let prefix = if directory.ends_with("grub2") {
        "grub2"
    } else {
        "grub"
    };
    let env = directory.join("grubenv");
    let env_arg = env.to_str().ok_or("invalid GRUB path")?;
    let pending = capture(runner, &format!("{prefix}-editenv"), &[env_arg, "list"])?;
    if pending
        .lines()
        .any(|line| line.starts_with("next_entry=") || line.starts_with("prev_saved_entry="))
    {
        return Err("another one-shot boot is already armed".into());
    }
    // GRUB cannot safely save its environment on arbitrary abstractions/filesystems.
    if !capture(
        runner,
        &format!("{prefix}-probe"),
        &["--target=abstraction", env_arg],
    )?
    .trim()
    .is_empty()
        || capture(
            runner,
            &format!("{prefix}-probe"),
            &["--target=fs", env_arg],
        )?
        .trim()
            != "ext2"
    {
        return Err("one-shot media boot requires a plain ext2/3/4 GRUB filesystem (no encryption, RAID or LVM)".into());
    }
    let uuid = capture(
        runner,
        &format!("{prefix}-probe"),
        &["--target=fs_uuid", env_arg],
    )?
    .trim()
    .to_owned();
    if !crate::shared::storage::valid_uuid(&uuid) {
        return Err("invalid GRUB filesystem UUID".into());
    }
    let cfg = directory.join("grub.cfg");
    let grub_path = capture(
        runner,
        &format!("{prefix}-mkrelpath"),
        &[cfg.to_str().ok_or("invalid GRUB path")?],
    )?
    .trim()
    .to_owned();
    if !super::boot::valid_grub_path(&grub_path) {
        return Err("unsafe GRUB relative path".into());
    }
    let platform = if firmware == BootMode::Bios {
        "i386-pc"
    } else {
        "x86_64-efi"
    };
    for module in [
        "normal",
        "loadenv",
        "search_fs_uuid",
        if firmware == BootMode::Bios {
            "linux16"
        } else {
            "chain"
        },
    ] {
        let file = directory.join(platform).join(format!("{module}.mod"));
        if read(&file)?.is_empty() {
            return Err(format!("missing GRUB module {module}"));
        }
    }
    if plan.target == Target::LocalIso {
        for module in [
            "loopback",
            "iso9660",
            "linux",
            "lvm",
            "mdraid1x",
            "all_video",
        ] {
            if read(&directory.join(platform).join(format!("{module}.mod")))?.is_empty() {
                return Err(format!("missing ISO GRUB module {module}"));
            }
        }
    }
    run_checked(
        runner,
        &command(format!("{prefix}-script-check"), [cfg.as_os_str()]),
    )
    .map_err(|e| e.to_string())?;
    let init = fs::read_link(root.join("proc/1/exe")).map_err(|e| e.to_string())?;
    let reboot = match super::handoff::detect_init(&init).map_err(|e| e.to_string())? {
        super::handoff::HostInit::Systemd => command("systemctl", ["reboot"]),
        super::handoff::HostInit::OpenRcInit => command("openrc-shutdown", ["--reboot", "now"]),
    };
    let mut confirmation = format!(
        "BOOT-ONCE {} {} UUID={} ACTIVE-GRUB-CONFIRMED",
        plan.target,
        firmware.as_str(),
        uuid
    );
    let mut iso = None;
    let mut iso_digest = None;
    if plan.target == Target::LocalIso {
        let options = plan.iso.as_ref().ok_or("missing ISO options")?;
        let image = super::iso::canonicalize_iso_source(Path::new(&options.path), &directory)
            .map_err(|e| e.to_string())?;
        validate_planned_file(&image).map_err(|e| e.to_string())?;
        let parent = root.join("var/tmp");
        crate::shared::configure::ensure_parent(&parent.join("placeholder"))
            .map_err(|e| e.to_string())?;
        let temporary = parent.join(format!(
            "rustaller-iso-{}-{}",
            std::process::id(),
            NEXT_ISO.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        DirBuilder::new()
            .mode(0o700)
            .create(&temporary)
            .map_err(|e| e.to_string())?;
        let inspected =
            super::iso::prepare_local_iso(runner, options, &temporary, &temporary.join("check"));
        // Never recursively remove an inspection directory after failed unmount.
        let _ = fs::remove_dir(temporary.join("check"));
        let _ = fs::remove_dir(&temporary);
        let prepared = inspected.map_err(|e| e.to_string())?;
        let digest = image_digest(runner, prepared.image())?;
        confirmation.push_str(&format!(" ISO-SHA256={digest}"));
        iso = Some(prepared);
        iso_digest = Some(digest);
    }
    Ok(MediaBoot {
        confirmation,
        firmware,
        root: root.into(),
        directory: directory.clone(),
        prefix,
        uuid,
        grub_path,
        config,
        custom,
        environment,
        custom_mode: fs::metadata(directory.join("custom.cfg"))
            .map(|m| m.permissions().mode() & 0o7777)
            .unwrap_or(0o644),
        environment_mode: fs::metadata(directory.join("grubenv"))
            .map_err(|e| e.to_string())?
            .permissions()
            .mode()
            & 0o7777,
        reboot,
        target: plan.target,
        iso,
        iso_digest,
    })
}

fn fetch(runner: &mut impl CommandRunner, name: &str, max: usize) -> Result<Vec<u8>> {
    let spec = command(
        "curl",
        [
            "--disable",
            "-fsSL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            "120",
            "--max-filesize",
            &max.to_string(),
            &format!("{ORIGIN}/{name}"),
        ],
    );
    let bytes = capture_checked(runner, &spec).map_err(|e| e.to_string())?;
    if bytes.is_empty() || bytes.len() > max {
        return Err("invalid netboot download size".into());
    }
    Ok(bytes)
}

fn validate_netboot(bytes: &[u8], firmware: BootMode) -> Result<()> {
    if bytes.len() < 1024 {
        return Err("truncated netboot artifact".into());
    }
    let valid = match firmware {
        BootMode::Bios => {
            bytes.get(0x1fe..0x200) == Some(&[0x55, 0xaa])
                && bytes.get(0x202..0x206) == Some(b"HdrS")
        }
        BootMode::Efi => {
            let offset = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
            bytes.starts_with(b"MZ")
                && offset.checked_add(94).is_some_and(|end| end <= bytes.len())
                && bytes.get(offset..offset + 4) == Some(b"PE\0\0")
                && bytes.get(offset + 4..offset + 6) == Some(&[0x64, 0x86])
                && bytes.get(offset + 24..offset + 26) == Some(&[0x0b, 0x02])
                && bytes.get(offset + 92..offset + 94) == Some(&[10, 0])
        }
    };
    if valid {
        Ok(())
    } else {
        Err(
            "netboot artifact does not match BIOS Linux/iPXE or x86_64 EFI application format"
                .into(),
        )
    }
}

fn checksum(manifest: &str, name: &str) -> Result<String> {
    let mut found = None;
    for line in manifest.lines() {
        let mut fields = line.split_whitespace();
        let Some(digest) = fields.next() else {
            continue;
        };
        let Some(file) = fields.next() else { continue };
        if file.trim_start_matches('*') != name {
            continue;
        }
        if found.is_some()
            || fields.next().is_some()
            || digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("invalid or ambiguous official SHA-256 entry".into());
        }
        found = Some(digest.to_ascii_lowercase());
    }
    found.ok_or_else(|| "missing official SHA-256 entry".into())
}

impl MediaBoot {
    fn unchanged(&self) -> Result<()> {
        if super::preflight::validate_source_boot(&self.root).map_err(str::to_owned)?
            != self.firmware
        {
            return Err("firmware changed since inspection".into());
        }
        if read(&self.directory.join("grub.cfg"))? != self.config
            || optional(&self.directory.join("custom.cfg"))? != self.custom
            || read(&self.directory.join("grubenv"))? != self.environment
        {
            return Err("GRUB state changed since inspection; inspect again".into());
        }
        Ok(())
    }
    pub fn execute(&self, runner: &mut impl CommandRunner, confirmation: &str) -> Result<()> {
        if confirmation != self.confirmation {
            return Err("boot cancelled: exact scoped confirmation required".into());
        }
        self.unchanged()?;
        if let Some(iso) = &self.iso
            && Some(image_digest(runner, iso.image())?) != self.iso_digest
        {
            return Err("ISO changed after inspection; inspect again".into());
        }
        let work = self.directory.join(ID);
        DirBuilder::new()
            .mode(0o700)
            .create(&work)
            .map_err(|e| e.to_string())?;
        let result = self
            .prepare_entry(runner, &work)
            .and_then(|entry| self.publish(runner, &work, &entry));
        if result
            .as_ref()
            .is_err_and(|error| !error.contains("RESTORE FAILED"))
        {
            // Only known files: never recurse into an ISO mount after failed unmount.
            for name in [
                "loader",
                "entry.cfg",
                "custom.before",
                "grubenv.before",
                "state.before",
            ] {
                let _ = fs::remove_file(work.join(name));
            }
            let _ = fs::remove_dir(&work);
        }
        result
    }
    fn prepare_entry(&self, runner: &mut impl CommandRunner, work: &Path) -> Result<String> {
        if let Some(iso) = &self.iso {
            let entry = super::iso::render_grub_entry(iso);
            let start = entry.find("menuentry ").ok_or("invalid ISO entry")?;
            return Ok(entry[start..].replace(
                " --unrestricted {",
                " --unrestricted --id rustaller-media {",
            ));
        }
        self.prepare_netboot(runner, work)
    }
    fn prepare_netboot(&self, runner: &mut impl CommandRunner, work: &Path) -> Result<String> {
        let name = if self.firmware == BootMode::Bios {
            "netboot.xyz.lkrn"
        } else {
            "netboot.xyz.efi"
        };
        let manifest = fetch(runner, "netboot.xyz-sha256-checksums.txt", 1024 * 1024)?;
        let digest = checksum(
            std::str::from_utf8(&manifest).map_err(|_| "invalid checksum text")?,
            name,
        )?;
        let bytes = fetch(runner, name, 16 * 1024 * 1024)?;
        validate_netboot(&bytes, self.firmware)?;
        let hash = capture_checked(
            runner,
            &command("sha256sum", [] as [&str; 0]).stdin(CommandInput::Bytes(bytes.clone())),
        )
        .map_err(|e| e.to_string())?;
        if String::from_utf8_lossy(&hash).split_whitespace().next() != Some(&digest) {
            return Err("netboot SHA-256 mismatch".into());
        }
        atomic_replace(&work.join("loader"), &bytes, 0o600).map_err(|e| e.to_string())?;
        let path = Path::new(&self.grub_path)
            .parent()
            .ok_or("invalid GRUB path")?
            .join(ID)
            .join("loader");
        let load = if self.firmware == BootMode::Bios {
            "linux16"
        } else {
            "chainloader"
        };
        Ok(format!(
            "menuentry 'rustaller ({})' --id {ID} {{\n search --no-floppy --fs-uuid --set=root {}\n {load} {}\n}}\n",
            self.target,
            self.uuid,
            path.display()
        ))
    }
    fn publish(&self, runner: &mut impl CommandRunner, work: &Path, entry: &str) -> Result<()> {
        self.unchanged()?;
        let mut output = self.custom.clone().unwrap_or_default();
        output.extend_from_slice(
            format!("\nif [ \"$default\" = \"{ID}\" ]; then\n{entry}fi\n").as_bytes(),
        );
        let checked = work.join("entry.cfg");
        atomic_replace(&checked, &output, 0o600).map_err(|e| e.to_string())?;
        run_checked(
            runner,
            &command(
                format!("{}-script-check", self.prefix),
                [checked.as_os_str()],
            ),
        )
        .map_err(|e| e.to_string())?;
        atomic_replace(
            &work.join("custom.before"),
            self.custom.as_deref().unwrap_or_default(),
            0o600,
        )
        .map_err(|e| e.to_string())?;
        atomic_replace(&work.join("grubenv.before"), &self.environment, 0o600)
            .map_err(|e| e.to_string())?;
        let state = format!(
            "custom_present={}\ncustom_mode={:o}\nenvironment_mode={:o}\n",
            self.custom.is_some(),
            self.custom_mode,
            self.environment_mode
        );
        atomic_replace(&work.join("state.before"), state.as_bytes(), 0o600)
            .map_err(|e| e.to_string())?;
        self.unchanged()?;
        let custom = self.directory.join("custom.cfg");
        let result = (|| {
            atomic_replace(&custom, &output, self.custom_mode).map_err(|e| e.to_string())?;
            let env = self.directory.join("grubenv");
            run_checked(
                runner,
                &command(
                    format!("{}-editenv", self.prefix),
                    [
                        env.as_os_str(),
                        "set".as_ref(),
                        "next_entry=rustaller-media".as_ref(),
                    ],
                ),
            )
            .map_err(|e| e.to_string())?;
            let listed = capture(
                runner,
                &format!("{}-editenv", self.prefix),
                &[env.to_str().ok_or("invalid GRUB path")?, "list"],
            )?;
            if !listed.lines().any(|v| v == "next_entry=rustaller-media") {
                return Err("GRUB did not retain one-shot entry".into());
            }
            run_checked(runner, &self.reboot).map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            let environment_restore = atomic_replace(
                &self.directory.join("grubenv"),
                &self.environment,
                self.environment_mode,
            );
            let custom_restore = match &self.custom {
                Some(bytes) => atomic_replace(&custom, bytes, self.custom_mode),
                None => match fs::remove_file(&custom) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    result => result,
                },
            };
            let restore = environment_restore.and(custom_restore);
            return Err(match restore {
                Ok(()) => error,
                Err(e) => format!(
                    "{error}; BOOT STATE RESTORE FAILED: {e}; backups: {}",
                    work.display()
                ),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandOutput, MockRunner};
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new(mode: BootMode) -> Self {
            let root = std::env::temp_dir().join(format!(
                "rustaller-media-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let dir = root.join("boot/grub");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("grub.cfg"), "load_env\nset default=\"${next_entry}\"\nset next_entry=\nsave_env next_entry\nsource $prefix/custom.cfg\n").unwrap();
            let mut env = b"# GRUB Environment Block\n".to_vec();
            env.resize(1024, b'#');
            fs::write(dir.join("grubenv"), env).unwrap();
            fs::write(dir.join("custom.cfg"), "# existing user entries\n").unwrap();
            let platform = if mode == BootMode::Bios {
                "i386-pc"
            } else {
                "x86_64-efi"
            };
            fs::create_dir(dir.join(platform)).unwrap();
            for module in [
                "normal",
                "loadenv",
                "search_fs_uuid",
                "linux16",
                "chain",
                "loopback",
                "iso9660",
                "linux",
                "lvm",
                "mdraid1x",
                "all_video",
            ] {
                fs::write(dir.join(platform).join(format!("{module}.mod")), "module").unwrap();
            }
            fs::create_dir_all(root.join("proc/1")).unwrap();
            symlink("/usr/lib/systemd/systemd", root.join("proc/1/exe")).unwrap();
            if mode == BootMode::Efi {
                fs::create_dir_all(root.join("sys/firmware/efi/efivars")).unwrap();
                fs::write(root.join("sys/firmware/efi/fw_platform_size"), "64").unwrap();
                // EFI source validation requires a readable SecureBoot variable.
                fs::write(
                    root.join(
                        "sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c",
                    ),
                    [0, 0, 0, 0, 0],
                )
                .unwrap();
            }
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn out(text: &str) -> CommandOutput {
        CommandOutput::new(Some(0), text.as_bytes().to_vec(), vec![])
    }
    fn discovery() -> MockRunner {
        MockRunner::new([
            out(""),
            out(""),
            out("ext2"),
            out("1234-abcd"),
            out("/grub/grub.cfg"),
            out(""),
        ])
    }
    fn artifact(mode: BootMode) -> Vec<u8> {
        let mut bytes = vec![0; 1024];
        if mode == BootMode::Bios {
            bytes[0x1fe..0x200].copy_from_slice(&[0x55, 0xaa]);
            bytes[0x202..0x206].copy_from_slice(b"HdrS");
        } else {
            bytes[..2].copy_from_slice(b"MZ");
            bytes[0x3c..0x40].copy_from_slice(&128u32.to_le_bytes());
            bytes[128..132].copy_from_slice(b"PE\0\0");
            bytes[132..134].copy_from_slice(&[0x64, 0x86]);
            bytes[152..154].copy_from_slice(&[0x0b, 0x02]);
            bytes[220..222].copy_from_slice(&[10, 0]);
        }
        bytes
    }
    fn preparation(mode: BootMode) -> Vec<CommandOutput> {
        let file = if mode == BootMode::Bios {
            "netboot.xyz.lkrn"
        } else {
            "netboot.xyz.efi"
        };
        vec![
            out(&format!("{} *{file}\n", "a".repeat(64))),
            CommandOutput::new(Some(0), artifact(mode), vec![]),
            out(&"a".repeat(64)),
            out(""),
            out(""),
            out("next_entry=rustaller-media\n"),
            out(""),
        ]
    }
    #[test]
    fn one_shot_boots_share_checked_publication_and_preserve_default_configuration() {
        for mode in [BootMode::Bios, BootMode::Efi] {
            let root = Fixture::new(mode);
            let boot = discover_at(
                &mut discovery(),
                &InstallPlan::new(Target::NetbootXyz),
                &root.0,
            )
            .unwrap();
            let mut runner = MockRunner::new(preparation(mode));
            boot.execute(&mut runner, &boot.confirmation).unwrap();
            assert_eq!(read(&boot.directory.join("grub.cfg")).unwrap(), boot.config);
            let custom = fs::read_to_string(boot.directory.join("custom.cfg")).unwrap();
            assert!(custom.starts_with("# existing user entries"));
            assert!(custom.contains("if [ \"$default\" = \"rustaller-media\" ]"));
            assert!(custom.contains(if mode == BootMode::Bios {
                "linux16"
            } else {
                "chainloader"
            }));
            assert_eq!(runner.calls().last().unwrap().args, ["reboot"]);
            assert!(runner.calls().iter().all(|c| {
                !["mkfs", "parted", "efibootmgr"]
                    .iter()
                    .any(|v| c.program == *v)
            }));
        }
    }
    #[test]
    fn every_preparation_failure_restores_original_boot_state() {
        for failure in 0..7 {
            let root = Fixture::new(BootMode::Bios);
            let boot = discover_at(
                &mut discovery(),
                &InstallPlan::new(Target::NetbootXyz),
                &root.0,
            )
            .unwrap();
            let mut responses = preparation(BootMode::Bios);
            responses.truncate(failure + 1);
            responses[failure] = CommandOutput::new(Some(1), vec![], vec![]);
            assert!(
                boot.execute(&mut MockRunner::new(responses), &boot.confirmation)
                    .is_err()
            );
            assert_eq!(
                read(&boot.directory.join("grubenv")).unwrap(),
                boot.environment
            );
            assert_eq!(
                optional(&boot.directory.join("custom.cfg")).unwrap(),
                boot.custom
            );
            assert!(!boot.directory.join(ID).exists());
        }
    }
    #[test]
    fn cancelled_stale_or_unsafe_preparations_never_arm_a_boot() {
        let root = Fixture::new(BootMode::Bios);
        let boot = discover_at(
            &mut discovery(),
            &InstallPlan::new(Target::NetbootXyz),
            &root.0,
        )
        .unwrap();
        let mut runner = MockRunner::empty();
        assert!(boot.execute(&mut runner, "yes").is_err());
        fs::write(boot.directory.join("custom.cfg"), "changed").unwrap();
        assert!(boot.execute(&mut runner, &boot.confirmation).is_err());
        assert!(runner.calls().is_empty());
        fs::remove_file(boot.directory.join("grubenv")).unwrap();
        symlink("/etc/passwd", boot.directory.join("grubenv")).unwrap();
        assert!(
            discover_at(
                &mut discovery(),
                &InstallPlan::new(Target::NetbootXyz),
                &root.0
            )
            .is_err()
        );
    }
    #[test]
    fn malformed_or_mismatched_downloads_are_rejected() {
        for mode in [BootMode::Bios, BootMode::Efi] {
            assert!(validate_netboot(&artifact(mode), mode).is_ok());
            assert!(
                validate_netboot(
                    &artifact(mode),
                    if mode == BootMode::Bios {
                        BootMode::Efi
                    } else {
                        BootMode::Bios
                    }
                )
                .is_err()
            );
        }
        assert!(checksum("bad *netboot.xyz.efi", "netboot.xyz.efi").is_err());
        let line = format!("{} *netboot.xyz.efi\n", "a".repeat(64));
        assert!(checksum(&format!("{line}{line}"), "netboot.xyz.efi").is_err());
        assert!(checksum(&format!("bad *netboot.xyz.efi\\n{line}"), "netboot.xyz.efi").is_err());
        let root = Fixture::new(BootMode::Bios);
        let boot = discover_at(
            &mut discovery(),
            &InstallPlan::new(Target::NetbootXyz),
            &root.0,
        )
        .unwrap();
        let mut responses = preparation(BootMode::Bios);
        responses[2] = out(&"b".repeat(64));
        assert!(
            boot.execute(&mut MockRunner::new(responses), &boot.confirmation)
                .is_err()
        );
        assert_eq!(
            optional(&boot.directory.join("custom.cfg")).unwrap(),
            boot.custom
        );
    }

    #[test]
    fn preflight_rejects_pending_boot_missing_modules_and_unknown_grub_semantics() {
        for scenario in ["pending", "module", "configuration", "firmware"] {
            let root = Fixture::new(BootMode::Bios);
            let mut plan = InstallPlan::new(Target::NetbootXyz);
            let mut runner = discovery();
            match scenario {
                "pending" => runner = MockRunner::new([out("next_entry=another-entry")]),
                "module" => fs::remove_file(root.0.join("boot/grub/i386-pc/linux16.mod")).unwrap(),
                "configuration" => {
                    fs::write(root.0.join("boot/grub/grub.cfg"), "menuentry old {}").unwrap()
                }
                _ => plan.force_boot_mode = Some(BootMode::Efi),
            }
            assert!(
                discover_at(&mut runner, &plan, &root.0).is_err(),
                "{scenario}"
            );
            assert!(!root.0.join("boot/grub/rustaller-media").exists());
        }
    }

    struct IsoRunner {
        inner: MockRunner,
        kernel: bool,
    }
    impl CommandRunner for IsoRunner {
        fn run(&mut self, spec: &CommandSpec) -> std::io::Result<CommandOutput> {
            if spec.program == "mount" {
                let path = PathBuf::from(spec.args.last().unwrap());
                fs::create_dir_all(path.join("boot/grub"))?;
                fs::write(path.join("boot/grub/loopback.cfg"), "menuentry Linux {}")?;
                fs::write(path.join("vmlinuz"), "Linux fixture")?;
                fs::write(path.join("initrd"), "initrd fixture")?;
            } else if spec.program == "umount" {
                let path = PathBuf::from(&spec.args[0]);
                // Simulate disappearance of mounted files in this private fixture.
                fs::remove_dir_all(&path)?;
                fs::create_dir(&path)?;
            }
            let _ = self.kernel;
            self.inner.run(spec)
        }
        fn run_pipeline(
            &mut self,
            specs: &[CommandSpec],
        ) -> std::io::Result<crate::command::PipelineOutput> {
            self.inner.run_pipeline(specs)
        }
    }
    #[test]
    fn both_iso_modes_reach_one_shot_execution_without_download_or_disk_operations() {
        for mode in [BootMode::Bios, BootMode::Efi] {
            for kernel in [false, true] {
                let root = Fixture::new(mode);
                let image = root.0.join("live.iso");
                fs::write(&image, "trusted Linux ISO fixture").unwrap();
                let mut plan = InstallPlan::new(Target::LocalIso);
                plan.iso = Some(crate::model::IsoOptions {
                    path: image.display().to_string(),
                    config: None,
                    kernel: kernel.then(|| "/vmlinuz".into()),
                    initrd: kernel.then(|| "/initrd".into()),
                    cmdline: kernel.then(|| "iso=%ISO_PATH%".into()),
                });
                let mut responses = vec![
                    out(""),
                    out(""),
                    out("ext2"),
                    out("1234-abcd"),
                    out("/grub/grub.cfg"),
                    out(""),
                ];
                responses.extend([
                    out(""),
                    out("abcd-1234"),
                    out("/live.iso"),
                    out(""),
                    out(""),
                    out(&"c".repeat(64)),
                ]);
                let mut runner = IsoRunner {
                    inner: MockRunner::new(responses),
                    kernel,
                };
                let boot = discover_at(&mut runner, &plan, &root.0).unwrap();
                assert!(boot.confirmation.contains("ISO-SHA256="));
                let mut changed = MockRunner::new([out(&"d".repeat(64))]);
                assert!(boot.execute(&mut changed, &boot.confirmation).is_err());
                assert!(!boot.directory.join(ID).exists());
                let mut execute = MockRunner::new([
                    out(&"c".repeat(64)),
                    out(""),
                    out(""),
                    out("next_entry=rustaller-media"),
                    out(""),
                ]);
                boot.execute(&mut execute, &boot.confirmation).unwrap();
                let rendered = fs::read_to_string(boot.directory.join("custom.cfg")).unwrap();
                assert!(rendered.contains(if kernel {
                    "linux (loop)/vmlinuz"
                } else {
                    "configfile (loop)/boot/grub/loopback.cfg"
                }));
                assert!(!execute.calls().iter().any(|c| c.program == "curl"));
                assert_eq!(execute.calls().last().unwrap().args, ["reboot"]);
            }
        }
    }
}
