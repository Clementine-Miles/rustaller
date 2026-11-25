//! Disposable, unprivileged QEMU acceptance. No host block device is attached.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustaller::model::Target;
use rustaller::postinstall::required_kernel;

const HELP: &str = "Usage: cargo xtask vm-acceptance ACTION [OPTIONS]\n\nActions:\n  smoke           BIOS/UEFI Alpine Live boot smoke, not an installation test\n  prepare-source  Provision a disposable Debian Cloud source for kexec tests\n  install         Reinstall disposable source overlays and verify target boots\n\nCommon options:\n  --tools PATH       Extracted Debian QEMU/OVMF/xorriso root\n  --accel MODE       tcg|kvm (default tcg; no automatic fallback)\n  --memory-mib N     Guest RAM in MiB (512-65536; default 3072)\n  --output PATH      New private result directory (must not exist)\n  --firmware MODE    bios|efi|both (default both)\n  --timeout SECONDS  Per-case deadline (default 1800)\n\nSmoke: --iso FILE --sha256 HEX\nSource/install: --base FILE --sha512 HEX --key FILE --public-key FILE\n  --disk-bus BUS     virtio-blk|virtio-scsi|sata|nvme|pvscsi (default virtio-blk)\nInstall: --installer STATIC_ELF --target alpine|debian|arch|gentoo|all\n  --filesystem FS   xfs|ext4|all (default xfs)\n  --account MODE    root|admin|alternating (default alternating)\n  --kernel-update B  yes|no (default no); upgrade, regenerate, reboot, verify\n  --ipv6-acceptance  off|local|external static dual-stack checks\n  --reset-check B   yes|no (default no); verify held cancellation before install\n  --boot-size-mib N  FAT32 capacity (512-4096); consider 1024 for upgrades\n  --root-size-mib N  Optional bounded root partition for large sparse disks\n  --keep-disks B     yes|no (default no); retain successful guest overlays\n  --disk-size-gib N  Optional sparse overlay expansion (3072 tests BIOS/GPT)\n\nAll disk files and EFI variables are fresh disposable copies. No host devices,\nnetwork-policy changes, host reboot, or privileged mounts are used.\n";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Firmware {
    Bios,
    Efi,
}

const VM_IPV6_ADDRESS: &str = "fd42:5253:544c:1::15/64";
const VM_IPV6_GATEWAY: &str = "fd42:5253:544c:1::2";
const VM_IPV6_PREFIX: &str = "fd42:5253:544c:1::/64";
const EXTERNAL_IPV6_HOST: &str = "www.debian.org";
const VM_TRANSPORT_CA: &str = "/var/tmp/rustaller-vm/transport-ca.pem";
const VM_TRANSPORT_PATH: &str = "/run/rustaller-vm/transport";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ipv6Acceptance {
    Off,
    Local,
    External,
}

struct LocalIpv6Endpoint {
    server: Child,
    port: u16,
    cert: PathBuf,
}

impl LocalIpv6Endpoint {
    fn start(output: &Path) -> Result<Self, String> {
        let key = output.join("ipv6-local.key");
        let cert = output.join("ipv6-local.pem");
        private_file(&key)?;
        let generate = Command::new("openssl")
            .args([
                "req",
                "-new",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:P-256",
                "-nodes",
                "-days",
                "2",
                "-subj",
                "/CN=rustaller-vm-ipv6",
                "-addext",
                &format!("subjectAltName=IP:{VM_IPV6_GATEWAY}"),
                "-keyout",
            ])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .status()
            .map_err(|e| format!("cannot generate the disposable IPv6 TLS certificate: {e}"))?;
        if !generate.success() {
            return Err("cannot generate the disposable IPv6 TLS certificate".into());
        }
        let port = TcpListener::bind("[::1]:0")
            .and_then(|socket| socket.local_addr())
            .map_err(|e| format!("IPv6 loopback is required for local acceptance: {e}"))?
            .port();
        let server = Command::new("openssl")
            .args([
                "s_server",
                "-www",
                "-accept",
                &format!("[::1]:{port}"),
                "-cert",
            ])
            .arg(&cert)
            .arg("-key")
            .arg(&key)
            .stdout(private_file(&output.join("ipv6-local-tls.log"))?)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut endpoint = Self { server, port, cert };
        for _ in 0..30 {
            if endpoint
                .server
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                break;
            }
            if TcpStream::connect_timeout(
                &std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
                Duration::from_millis(100),
            )
            .is_ok()
            {
                fs::remove_file(&key).map_err(|e| e.to_string())?;
                return Ok(endpoint);
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err("the disposable IPv6 TLS endpoint did not start".into())
    }
}

impl Drop for LocalIpv6Endpoint {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

impl Ipv6Acceptance {
    fn selected(value: Option<&String>) -> Result<Self, String> {
        match value.map(String::as_str).unwrap_or("off") {
            "off" => Ok(Self::Off),
            "local" => Ok(Self::Local),
            "external" => Ok(Self::External),
            _ => Err("ipv6-acceptance must be off, local or external".into()),
        }
    }
}
impl Firmware {
    fn label(self) -> &'static str {
        match self {
            Self::Bios => "bios",
            Self::Efi => "efi",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiskBus {
    VirtioBlk,
    VirtioScsi,
    Sata,
    Nvme,
    Pvscsi,
}
impl DiskBus {
    fn device(self) -> &'static str {
        match self {
            Self::VirtioBlk => "/dev/vda",
            Self::Nvme => "/dev/nvme0n1",
            _ => "/dev/sda",
        }
    }
    fn attach(self, command: &mut Command, serial: &str) {
        let model = match self {
            Self::VirtioBlk => "virtio-blk-pci",
            Self::VirtioScsi => {
                command.args(["-device", "virtio-scsi-pci,id=storage"]);
                "scsi-hd,bus=storage.0"
            }
            Self::Pvscsi => {
                command.args(["-device", "pvscsi,id=storage"]);
                "scsi-hd,bus=storage.0"
            }
            Self::Sata => "ide-hd,bus=ide.0",
            Self::Nvme => "nvme",
        };
        command
            .arg("-device")
            .arg(format!("{model},drive=system,serial={serial}"));
    }
}
struct Options {
    action: String,
    values: BTreeMap<String, String>,
}
impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        let action = args.first().cloned().ok_or("missing acceptance action")?;
        if !matches!(action.as_str(), "smoke" | "prepare-source" | "install") {
            return Err("unknown acceptance action".into());
        }
        let mut values = BTreeMap::new();
        let mut at = 1;
        while at < args.len() {
            let flag = &args[at];
            if !matches!(
                flag.as_str(),
                "--tools"
                    | "--accel"
                    | "--memory-mib"
                    | "--output"
                    | "--firmware"
                    | "--timeout"
                    | "--iso"
                    | "--sha256"
                    | "--base"
                    | "--sha512"
                    | "--key"
                    | "--public-key"
                    | "--installer"
                    | "--target"
                    | "--disk-bus"
                    | "--filesystem"
                    | "--disk-size-gib"
                    | "--account"
                    | "--kernel-update"
                    | "--ipv6-acceptance"
                    | "--reset-check"
                    | "--source-network-manager"
                    | "--boot-size-mib"
                    | "--root-size-mib"
                    | "--keep-disks"
            ) {
                return Err(format!("unknown acceptance option: {flag}"));
            }
            at += 1;
            let value = args
                .get(at)
                .ok_or_else(|| format!("missing {flag} value"))?;
            if values.insert(flag.clone(), value.clone()).is_some() {
                return Err(format!("duplicate acceptance option: {flag}"));
            }
            at += 1;
        }
        Ok(Self { action, values })
    }
    fn value(&self, name: &str) -> Result<&str, String> {
        self.values
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("{name} is required"))
    }
    fn path(&self, name: &str) -> Result<PathBuf, String> {
        let path = PathBuf::from(self.value(name)?);
        if !path.is_absolute()
            || path
                .to_string_lossy()
                .chars()
                .any(|c| c.is_control() || c == ',')
        {
            return Err(format!(
                "{name} requires an absolute path without control characters or commas"
            ));
        }
        Ok(path)
    }
    fn firmware(&self) -> Result<Vec<Firmware>, String> {
        match self
            .values
            .get("--firmware")
            .map(String::as_str)
            .unwrap_or("both")
        {
            "bios" => Ok(vec![Firmware::Bios]),
            "efi" => Ok(vec![Firmware::Efi]),
            "both" => Ok(vec![Firmware::Bios, Firmware::Efi]),
            _ => Err("firmware must be bios, efi or both".into()),
        }
    }
    fn filesystems(&self) -> Result<Vec<&str>, String> {
        match self
            .values
            .get("--filesystem")
            .map(String::as_str)
            .unwrap_or("xfs")
        {
            "all" => Ok(vec!["xfs", "ext4"]),
            fs @ ("xfs" | "ext4") => Ok(vec![fs]),
            _ => Err("filesystem must be xfs, ext4 or all".into()),
        }
    }
    fn admin_account(&self, firmware: Firmware, filesystem: &str) -> Result<&'static str, String> {
        match self
            .values
            .get("--account")
            .map(String::as_str)
            .unwrap_or("alternating")
        {
            "root" => Ok("root"),
            "admin" => Ok("rustaller-admin"),
            "alternating" => match (firmware, filesystem) {
                (Firmware::Bios, "xfs") | (Firmware::Efi, "ext4") => Ok("root"),
                _ => Ok("rustaller-admin"),
            },
            _ => Err("account must be root, admin or alternating".into()),
        }
    }
    fn kernel_update(&self) -> Result<bool, String> {
        match self
            .values
            .get("--kernel-update")
            .map(String::as_str)
            .unwrap_or("no")
        {
            "yes" => Ok(true),
            "no" => Ok(false),
            _ => Err("kernel-update must be yes or no".into()),
        }
    }
    fn boot_size_mib(&self) -> Result<Option<u32>, String> {
        self.values
            .get("--boot-size-mib")
            .map(|value| {
                value
                    .parse::<u32>()
                    .ok()
                    .filter(|value| (512..=4096).contains(value))
                    .ok_or_else(|| "boot-size-mib must be 512-4096".to_owned())
            })
            .transpose()
    }
    fn disk_bus(&self) -> Result<DiskBus, String> {
        match self
            .values
            .get("--disk-bus")
            .map(String::as_str)
            .unwrap_or("virtio-blk")
        {
            "virtio-blk" => Ok(DiskBus::VirtioBlk),
            "virtio-scsi" => Ok(DiskBus::VirtioScsi),
            "sata" => Ok(DiskBus::Sata),
            "nvme" => Ok(DiskBus::Nvme),
            "pvscsi" => Ok(DiskBus::Pvscsi),
            _ => Err("disk-bus must be virtio-blk, virtio-scsi, sata, nvme or pvscsi".into()),
        }
    }
    fn timeout(&self) -> Result<Duration, String> {
        let seconds = self
            .values
            .get("--timeout")
            .map(String::as_str)
            .unwrap_or("1800")
            .parse::<u64>()
            .map_err(|_| "invalid timeout")?;
        if !(30..=14400).contains(&seconds) {
            return Err("timeout must be 30-14400 seconds".into());
        }
        Ok(Duration::from_secs(seconds))
    }
}
struct Tools {
    root: PathBuf,
    accel: String,
    memory_mib: String,
}
impl Tools {
    fn program(&self, name: &str) -> Command {
        let mut command = Command::new(self.root.join("usr/bin").join(name));
        command.env(
            "LD_LIBRARY_PATH",
            self.root.join("usr/lib/x86_64-linux-gnu"),
        );
        command
    }
    fn qemu(&self) -> Command {
        let mut command = self.program("qemu-system-x86_64");
        command.args([
            "-no-user-config",
            "-nodefaults",
            "-machine",
            "q35",
            "-accel",
            if self.accel == "kvm" {
                "kvm"
            } else {
                "tcg,thread=multi"
            },
            "-cpu",
            if self.accel == "kvm" { "host" } else { "max" },
            "-smp",
            "2",
            "-m",
            &self.memory_mib,
            "-display",
            "none",
        ]);
        command.arg("-L").arg(self.root.join("usr/share/qemu"));
        command.arg("-device").arg(format!(
            "VGA,romfile={}",
            self.root
                .join("usr/share/seabios/vgabios-stdvga.bin")
                .display()
        ));
        command.args(["-device", "virtio-rng-pci"]);
        command
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args.is_empty()
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        println!("{HELP}");
        return Ok(());
    }
    let options = Options::parse(args)?;
    let accel = options
        .values
        .get("--accel")
        .map(String::as_str)
        .unwrap_or("tcg");
    if !matches!(accel, "tcg" | "kvm") {
        return Err("accel must be tcg or kvm".into());
    }
    let memory_mib = options
        .values
        .get("--memory-mib")
        .map(String::as_str)
        .unwrap_or("3072")
        .parse::<u32>()
        .ok()
        .filter(|n| (512..=65536).contains(n))
        .ok_or("memory-mib must be 512-65536")?;
    let tools = Tools {
        root: options.path("--tools")?,
        accel: accel.to_owned(),
        memory_mib: memory_mib.to_string(),
    };
    let output = options.path("--output")?;
    private_directory(&output)?;
    let modes = options.firmware()?;
    let timeout = options.timeout()?;
    let result = match options.action.as_str() {
        "smoke" => smoke(&tools, &options, &output, &modes, timeout),
        "prepare-source" => prepare_source(&tools, &options, &output, timeout),
        "install" => install_matrix(&tools, &options, &output, &modes, timeout),
        _ => unreachable!(),
    };
    if let Err(error) = &result {
        write_private(&output.join("failure.txt"), error.as_bytes())?;
    }
    result
}

fn smoke(
    tools: &Tools,
    options: &Options,
    output: &Path,
    modes: &[Firmware],
    timeout: Duration,
) -> Result<(), String> {
    let iso = options.path("--iso")?;
    verify_hash(&iso, options.value("--sha256")?, "sha256sum", 64)?;
    let modified = serial_iso(tools, &iso, output)?;
    let mut results = Vec::new();
    for &firmware in modes {
        let case = output.join(firmware.label());
        private_directory(&case)?;
        let mut command = tools.qemu();
        configure_firmware(&mut command, tools, &case, firmware)?;
        command.args(["-boot", "order=d", "-nic", "none"]);
        command.arg("-drive").arg(format!(
            "file={},format=raw,if=ide,index=2,media=cdrom,readonly=on",
            modified.display()
        ));
        let mut guest = Guest::start(command, &case)?;
        let deadline = Instant::now() + timeout;
        let mut passed = false;
        while Instant::now() < deadline {
            let serial = read_log(&case.join("serial.log"));
            if serial.contains("Welcome to Alpine Linux") && serial.contains("Linux version") {
                passed = true;
                break;
            }
            if guest.child.try_wait().map_err(|e| e.to_string())?.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(250));
        }
        guest.stop();
        results.push(format!(
            "{} firmware / Alpine Live boot: {}",
            firmware.label(),
            if passed { "PASS" } else { "FAIL" }
        ));
        write_private(
            &output.join("results.txt"),
            format!(
                "Scope: firmware-live-smoke only; no OS installation accepted\n{}\n",
                results.join("\n")
            )
            .as_bytes(),
        )?;
        if !passed {
            return Err(format!(
                "{} Alpine Live smoke did not reach its boot marker; see serial.log",
                firmware.label()
            ));
        }
    }
    println!(
        "Firmware/Live smoke passed; this does not establish installation acceptance. Results: {}",
        output.display()
    );
    Ok(())
}

fn serial_iso(tools: &Tools, iso: &Path, output: &Path) -> Result<PathBuf, String> {
    let syslinux = output.join("syslinux.cfg");
    let grub = output.join("grub.cfg");
    let mut extract = tools.program("xorriso");
    extract
        .args(["-osirrox", "on", "-indev"])
        .arg(iso)
        .args(["-extract", "/boot/syslinux/syslinux.cfg"])
        .arg(&syslinux)
        .args(["-extract", "/boot/grub/grub.cfg"])
        .arg(&grub);
    checked(&mut extract, &output.join("iso-extract.log"))?;
    for path in [&syslinux, &grub] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        let original = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let modified = original
            .lines()
            .map(|line| {
                if line.starts_with("APPEND ") || line.trim_start().starts_with("linux") {
                    format!("{} console=ttyS0,115200", line.replace(" quiet", ""))
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, format!("{modified}\n")).map_err(|e| e.to_string())?;
    }
    let modified = output.join("serial-test.iso");
    let mut remaster = tools.program("xorriso");
    remaster
        .arg("-indev")
        .arg(iso)
        .arg("-outdev")
        .arg(&modified)
        .args(["-boot_image", "any", "replay", "-map"])
        .arg(&syslinux)
        .arg("/boot/syslinux/syslinux.cfg")
        .arg("-map")
        .arg(&grub)
        .arg("/boot/grub/grub.cfg");
    checked(&mut remaster, &output.join("iso-remaster.log"))?;
    Ok(modified)
}

fn prepare_source(
    tools: &Tools,
    options: &Options,
    output: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let base = options.path("--base")?;
    verify_hash(&base, options.value("--sha512")?, "sha512sum", 128)?;
    check_base(tools, &base, output)?;
    let key = options.path("--key")?;
    regular(&key)?;
    let public = public_key(&options.path("--public-key")?)?;
    let transport = Transport::new()?;
    let relay = Relay::start()?;
    let case = output.join("source");
    private_directory(&case)?;
    let seed = create_seed(tools, &case, &public, &transport, &relay, false)?;
    let disk = create_overlay(tools, &base, &case)?;
    let port = free_port()?;
    let mut guest = start_disk_guest(
        tools,
        &case,
        &disk,
        &seed,
        Firmware::Bios,
        port,
        None,
        "source",
        options.disk_bus()?,
        false,
    )?;
    let ssh = Ssh {
        port,
        key,
        known: case.join("known-hosts"),
        account: "root",
    };
    ssh.wait_ready(&mut guest, Instant::now() + timeout)?;
    if let Err(error) = ssh.checked(
        &["cloud-init", "status", "--wait"],
        &case.join("cloud-init.log"),
        timeout,
    ) {
        if let Ok(detail) = ssh.capture(
            &["cloud-init", "schema", "--system"],
            Duration::from_secs(30),
        ) {
            write_private(&case.join("cloud-init-schema.log"), &detail.stdout)?;
        }
        return Err(error);
    }
    ssh.checked(
        &["test", "-x", "/sbin/kexec"],
        &case.join("kexec-tool.log"),
        Duration::from_secs(20),
    )?;
    let _ = ssh.capture(&["systemctl", "poweroff"], Duration::from_secs(20));
    if !guest.wait_exit(Duration::from_secs(90))? {
        return Err("source guest did not power off cleanly".into());
    }
    let prepared = output.join("source-prepared.qcow2");
    let mut convert = tools.program("qemu-img");
    convert
        .args(["convert", "-f", "qcow2", "-O", "qcow2"])
        .arg(&disk)
        .arg(&prepared);
    checked(&mut convert, &output.join("source-convert.log"))?;
    let digest = hash(&prepared, "sha512sum", 128)?;
    write_private(
        &output.join("SOURCE-SHA512SUMS"),
        format!("{digest}  source-prepared.qcow2\n").as_bytes(),
    )?;
    write_private(
        &output.join("results.txt"),
        b"Debian Cloud source provisioning: PASS\nNo target installation was executed.\n",
    )?;
    println!("Prepared disposable source: {}", prepared.display());
    Ok(())
}

fn install_matrix(
    tools: &Tools,
    options: &Options,
    output: &Path,
    modes: &[Firmware],
    timeout: Duration,
) -> Result<(), String> {
    let base = options.path("--base")?;
    verify_hash(&base, options.value("--sha512")?, "sha512sum", 128)?;
    check_base(tools, &base, output)?;
    let installer = options.path("--installer")?;
    regular(&installer)?;
    let key = options.path("--key")?;
    regular(&key)?;
    let public_path = options.path("--public-key")?;
    let public = public_key(&public_path)?;
    let targets = match options.value("--target")? {
        "all" => vec![Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo],
        "alpine" => vec![Target::Alpine],
        "debian" => vec![Target::Debian],
        "arch" => vec![Target::Arch],
        "gentoo" => vec![Target::Gentoo],
        _ => return Err("invalid acceptance target".into()),
    };
    let filesystems = options.filesystems()?;
    let bus = options.disk_bus()?;
    let mut cases = Vec::new();
    for target in targets {
        for &filesystem in &filesystems {
            for &mode in modes {
                cases.push((
                    target,
                    filesystem,
                    mode,
                    options.admin_account(mode, filesystem)?,
                ));
            }
        }
    }
    let keep_disks = match options
        .values
        .get("--keep-disks")
        .map(String::as_str)
        .unwrap_or("no")
    {
        "yes" => true,
        "no" => false,
        _ => return Err("keep-disks must be yes or no".into()),
    };
    let kernel_update = options.kernel_update()?;
    let source_nm = match options
        .values
        .get("--source-network-manager")
        .map(String::as_str)
    {
        None | Some("networkd") => false,
        Some("networkmanager") => true,
        _ => return Err("source-network-manager must be networkd or networkmanager".into()),
    };
    let reset_check = match options.values.get("--reset-check").map(String::as_str) {
        None | Some("no") => false,
        Some("yes") => true,
        _ => return Err("reset-check must be yes or no".into()),
    };
    let ipv6 = Ipv6Acceptance::selected(options.values.get("--ipv6-acceptance"))?;
    let local_ipv6 = if ipv6 == Ipv6Acceptance::Local {
        Some(LocalIpv6Endpoint::start(output)?)
    } else {
        None
    };
    if ipv6 == Ipv6Acceptance::External {
        let mut curl = Command::new("curl");
        curl.args([
            "--disable",
            "--ipv6",
            "--noproxy",
            "*",
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "15",
            "--output",
            "/dev/null",
            &format!("https://{EXTERNAL_IPV6_HOST}/"),
        ]);
        if !capture_timeout(curl, Duration::from_secs(20))?.success {
            return Err("external IPv6 NOT VERIFIED: the VM host cannot reach the HTTPS endpoint directly over IPv6".into());
        }
    }
    let boot_size_mib = options.boot_size_mib()?;
    let root_size_mib = options
        .values
        .get("--root-size-mib")
        .map(|v| {
            v.parse::<u32>()
                .ok()
                .filter(|n| *n >= 512)
                .ok_or("invalid root-size-mib")
        })
        .transpose()?;
    let probe = output.join("guest-probe");
    build_probe(&probe, &output.join("probe-build.log"))?;
    let unauthorized = UnauthorizedKey::generate(output)?;
    let transport = Transport::new()?;
    let relay = Relay::start()?;
    let mut results = Vec::new();
    let mut failed = false;
    for (target, filesystem, mode, account) in cases {
        let name = format!("{target}-{}-{filesystem}-{account}", mode.label()).to_ascii_lowercase();
        let case = output.join(&name);
        private_directory(&case)?;
        let result = install_case(
            tools,
            &base,
            &installer,
            &probe,
            &key,
            &unauthorized.0,
            &public_path,
            &public,
            &transport,
            &relay,
            target,
            account,
            kernel_update,
            source_nm,
            reset_check,
            ipv6,
            local_ipv6.as_ref(),
            boot_size_mib,
            root_size_mib,
            filesystem,
            options.values.get("--disk-size-gib").map(String::as_str),
            mode,
            bus,
            &case,
            timeout,
        );
        let passed = result.is_ok();
        let line = match result {
            Ok(()) => format!("{name}: PASS"),
            Err(error) => {
                failed = true;
                write_private(&case.join("failure.txt"), error.as_bytes())?;
                format!("{name}: FAIL ({error})")
            }
        };
        println!("{line}");
        results.push(line);
        write_private(
            &output.join("results.txt"),
            format!(
                "Scope: full install acceptance, fresh disposable source overlays\n{}\n",
                results.join("\n")
            )
            .as_bytes(),
        )?;
        // Failed images remain available for offline guest/boot diagnosis.
        if passed && !keep_disks {
            let _ = fs::remove_file(case.join("disk.qcow2"));
        }
    }
    if failed {
        Err("one or more disposable VM installation cases failed; see results.txt".into())
    } else {
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn install_case(
    tools: &Tools,
    base: &Path,
    installer: &Path,
    probe: &Path,
    key: &Path,
    unauthorized_key: &Path,
    public_path: &Path,
    public: &str,
    transport: &Transport,
    relay: &Relay,
    target: Target,
    account: &'static str,
    kernel_update: bool,
    source_nm: bool,
    reset_check: bool,
    ipv6: Ipv6Acceptance,
    local_ipv6: Option<&LocalIpv6Endpoint>,
    boot_size_mib: Option<u32>,
    root_size_mib: Option<u32>,
    filesystem: &str,
    disk_size_gib: Option<&str>,
    mode: Firmware,
    bus: DiskBus,
    case: &Path,
    timeout: Duration,
) -> Result<(), String> {
    let kernel = required_kernel(target).ok_or("acceptance target has no kernel policy")?;
    let init = target
        .init_system()
        .ok_or("acceptance target has no init policy")?;
    let target = target.to_string().to_ascii_lowercase();
    let target = target.as_str();
    let seed = create_seed(tools, case, public, transport, relay, source_nm)?;
    let disk = create_overlay(tools, base, case)?;
    let large_disk = disk_size_gib
        .map(|v| {
            v.parse::<u64>()
                .ok()
                .filter(|n| (64..=4096).contains(n))
                .ok_or("disk-size-gib must be 64-4096")
        })
        .transpose()?;
    let port = free_port()?;
    let target_ssh = if account == "root" {
        None
    } else {
        let forward = free_port()?;
        if forward == port {
            return Err("source and target need distinct host SSH ports".into());
        }
        Some((forward, 2222))
    };
    let (target_port, target_guest_port) = target_ssh.unwrap_or((port, 22));
    write_private(
        &case.join("hardware.txt"),
        format!(
            "accelerator={}\nmemory_mib={}\nvcpus=2\nmachine=q35\nfirmware={}\ndisk_bus={bus:?}\n",
            tools.accel,
            tools.memory_mib,
            mode.label()
        )
        .as_bytes(),
    )?;
    let mut guest = start_disk_guest(
        tools,
        case,
        &disk,
        &seed,
        mode,
        port,
        target_ssh,
        target,
        bus,
        ipv6 != Ipv6Acceptance::Off,
    )?;
    let source = Ssh {
        port,
        key: key.to_owned(),
        known: case.join("known-source"),
        account: "root",
    };
    let deadline = Instant::now() + timeout;
    source.wait_ready(&mut guest, deadline)?;
    source.checked(
        &["cloud-init", "status", "--wait"],
        &case.join("cloud-init.log"),
        remaining(deadline)?,
    )?;
    if let Some(gib) = large_disk {
        // Resize only after cloud-init finishes on the disposable source disk.
        // Otherwise it can try to grow the old root filesystem to several TiB.
        let bytes = gib << 30;
        writeln!(guest.monitor,"{{\"execute\":\"block_resize\",\"arguments\":{{\"device\":\"system\",\"size\":{bytes}}}}}").map_err(|e|e.to_string())?;
        let mut observed = false;
        for _ in 0..10 {
            let value = source.capture(
                &["blockdev", "--getsize64", bus.device()],
                Duration::from_secs(5),
            )?;
            if value.success && String::from_utf8_lossy(&value.stdout).trim() == bytes.to_string() {
                observed = true;
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }
        if !observed {
            return Err("guest did not report requested disk capacity after QMP resize".into());
        }
    }
    source.checked(
        &["mkdir", "-m", "700", "-p", "/run/rustaller-vm"],
        &case.join("source-mkdir.log"),
        Duration::from_secs(20),
    )?;
    source.checked(
        &["mkdir", "-m", "700", "-p", "/var/tmp/rustaller-vm"],
        &case.join("source-executable-directory.log"),
        Duration::from_secs(20),
    )?;
    source.checked(
        &["findmnt", "-nro", "TARGET,OPTIONS", "/run"],
        &case.join("source-run-mount.log"),
        Duration::from_secs(20),
    )?;
    source.copy(
        installer,
        "/var/tmp/rustaller-vm/rustaller",
        &case.join("copy-installer.log"),
    )?;
    source.copy(
        public_path,
        "/run/rustaller-vm/client.pub",
        &case.join("copy-public.log"),
    )?;
    source.checked(
        &["chmod", "700", "/var/tmp/rustaller-vm/rustaller"],
        &case.join("source-chmod.log"),
        Duration::from_secs(20),
    )?;
    let mut arguments = vec![
        "/var/tmp/rustaller-vm/rustaller".to_owned(),
        target.to_owned(),
        format!("--target-disk={}", bus.device()),
        format!("--username={account}"),
        format!("--ssh-port={target_guest_port}"),
        "--ssh-key=/run/rustaller-vm/client.pub".into(),
        format!("--hostname=rustaller-{target}"),
        "--timezone=Etc/UTC".into(),
        // The source uses static SLIRP addressing; there is no target DHCP
        // lease from which systemd-resolved can learn its DNS server.
        "--dns=10.0.2.3".into(),
        format!("--filesystem={filesystem}"),
    ];
    if ipv6 != Ipv6Acceptance::Off {
        arguments.extend([
            "--network-mac=52:54:00:12:34:56".into(),
            "--ipv4-address=10.0.2.15/24".into(),
            "--ipv4-gateway=10.0.2.2".into(),
            format!("--ipv6-address={VM_IPV6_ADDRESS}"),
            format!("--ipv6-gateway={VM_IPV6_GATEWAY}"),
            "--ipv6=enable".into(),
            // The optional IPv6 probe uses native curl and certificate validation.
            "--with-tools".into(),
        ]);
    }
    if let Some(size) = root_size_mib {
        arguments.push(format!("--root-size-mib={size}"));
    }
    if let Some(size) = boot_size_mib {
        arguments.push(format!("--boot-size-mib={size}"));
    }
    arguments.extend(["--swapfile-mib=64".into(), "--swappiness=10".into()]);
    source.checked(
        &["sh", "-c", "uname -a; lsblk -o NAME,TYPE,SIZE,SERIAL,FSTYPE,MOUNTPOINTS; ip -brief address; ip route; cat /etc/resolv.conf; networkctl status --no-pager; free -m"],
        &case.join("source-state.log"),
        Duration::from_secs(30),
    )?;
    if source_nm {
        source.checked(
            &[
                "nmcli",
                "-t",
                "-f",
                "UUID,TYPE,DEVICE",
                "connection",
                "show",
                "--active",
            ],
            &case.join("source-networkmanager.log"),
            Duration::from_secs(15),
        )?;
    }
    let mut inspect = arguments.clone();
    inspect.push("--inspect".into());
    let output = source.capture_owned(&inspect, Duration::from_secs(120))?;
    write_private(&case.join("inspect.log"), &output.stdout)?;
    if !output.success {
        for (name, args) in [
            (
                "source-efivarfs.log",
                vec!["findmnt", "-n", "-t", "efivarfs"],
            ),
            (
                "source-efi-variables.log",
                vec!["ls", "-la", "/sys/firmware/efi/efivars"],
            ),
        ] {
            if let Ok(detail) = source.capture(&args, Duration::from_secs(10)) {
                write_private(&case.join(name), &detail.stdout)?;
            }
        }
        return Err("source installer inspection failed; see inspect.log".into());
    }
    let text = String::from_utf8(output.stdout).map_err(|_| "inspection returned invalid UTF8")?;
    let token = text
        .lines()
        .filter_map(|line| line.strip_prefix("Confirmation token: "))
        .collect::<Vec<_>>();
    if token.len() != 1 || !token[0].starts_with(&format!("ERASE {} ", bus.device())) {
        return Err(
            "inspection did not return one identity-bound target-disk confirmation token".into(),
        );
    }
    verify_invalid_client_keys(&source, &arguments, token[0], case, deadline)?;
    if reset_check {
        verify_preparation_reset(&source, &arguments, token[0], bus.device(), case, deadline)?;
    }
    let reset_before = guest.resets.load(Ordering::Relaxed);
    let firmware_before = read_log(&case.join("firmware-debug.log")).len();
    let mut install = arguments;
    install.extend(["--confirm-erase".into(), token[0].into()]);
    let mut command = source.command_owned(&install);
    command
        .stdout(private_file(&case.join("install.stdout"))?)
        .stderr(private_file(&case.join("install.stderr"))?);
    let mut dispatch = command
        .spawn()
        .map_err(|e| format!("cannot dispatch guest installation: {e}"))?;
    let destination = Ssh {
        port: target_port,
        key: key.to_owned(),
        known: case.join("known-target"),
        account,
    };
    let live = Ssh {
        port: target_port,
        key: key.to_owned(),
        known: case.join("known-live"),
        account,
    };
    let mut last_live_log = Instant::now();
    let mut live_authenticated = false;
    let expected = format!("rustaller-{target}");
    let mut ready = false;
    while Instant::now() < deadline {
        if guest.child.try_wait().map_err(|e| e.to_string())?.is_some() {
            break;
        }
        if let Some(status) = dispatch.try_wait().map_err(|e| e.to_string())?
            && status.code().is_some_and(|code| code != 0 && code != 255)
        {
            return Err(format!(
                "source installer exited with {:?}; see install.stderr",
                status.code()
            ));
        }
        let serial = read_log(&case.join("serial.log"));
        let live_probe_interval = Duration::from_secs(if live_authenticated { 10 } else { 2 });
        if last_live_log.elapsed() >= live_probe_interval && serial.contains("RUSTALLER_LIVE_START")
        {
            last_live_log = Instant::now();
            if let Ok(log) =
                live.capture_root(&["cat", "/var/log/rustaller.log"], Duration::from_secs(5))
                && log.success
            {
                if !live_authenticated {
                    live.verify_key_only(unauthorized_key, &case.join("live-ssh-policy.log"))?;
                    live_authenticated = true;
                }
                write_private(&case.join("live-install.log"), &log.stdout)?;
                if String::from_utf8_lossy(&log.stdout)
                    .lines()
                    .any(|line| line.starts_with("rustaller-worker:"))
                {
                    if let Ok(detail) = live.capture_root(&["sh", "-c",
                        "ps; findmnt -R /os; for p in /proc/[0-9]*; do for link in \"$p/root\" \"$p/cwd\" \"$p\"/fd/*; do value=$(readlink \"$link\" 2>/dev/null) || continue; case \"$value\" in /os|/os/*) printf '%s -> %s: ' \"$link\" \"$value\"; tr '\\000' ' ' < \"$p/cmdline\"; printf '\\n';; esac; done; done"], Duration::from_secs(30)) {
                        write_private(&case.join("live-failure-state.log"), &detail.stdout)?;
                    }
                    return Err("Live worker stopped; see live-install.log".into());
                }
            }
        }
        if serial
            .lines()
            .any(|line| line.starts_with("rustaller-worker:"))
        {
            return Err("Live worker stopped; see serial.log".into());
        }
        if serial.contains("RUSTALLER_INSTALL_COMPLETE")
            && let Ok(result) = destination.capture(
                &["cat", "/proc/sys/kernel/hostname"],
                Duration::from_secs(10),
            )
            && result.success
            && String::from_utf8_lossy(&result.stdout).trim() == expected
        {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_secs(2));
    }
    let _ = dispatch.kill();
    let _ = dispatch.wait();
    if !ready {
        return Err(
            "new system did not become ready before deadline; see serial/install logs".into(),
        );
    }
    let serial = read_log(&case.join("serial.log"));
    if !serial.contains("RUSTALLER_LIVE_START") {
        return Err("no validated Alpine Live entry was observed".into());
    }
    if !live_authenticated {
        return Err(
            "installer did not observe key-only SSH in Alpine Live; see serial logs".into(),
        );
    }
    destination.verify_key_only(unauthorized_key, &case.join("target-ssh-policy.log"))?;
    let resets = guest
        .resets
        .load(Ordering::Relaxed)
        .saturating_sub(reset_before);
    let firmware_log = read_log(&case.join("firmware-debug.log"));
    let reset_check = validate_final_reboot(
        mode,
        resets,
        firmware_log.get(firmware_before..).unwrap_or_default(),
    );
    write_private(
        &case.join("firmware-reset-check.log"),
        reset_check
            .as_ref()
            .unwrap_or_else(|error| error)
            .as_bytes(),
    )?;
    let reset_failure = reset_check.err();
    // A reset-count failure must remain a failure, but should not discard the
    // opportunity to collect installed-target and native-update diagnostics.
    destination.checked(
        &["mkdir", "-m", "700", "-p", "/var/tmp/rustaller-vm"],
        &case.join("target-executable-directory.log"),
        Duration::from_secs(20),
    )?;
    destination.copy(
        probe,
        "/var/tmp/rustaller-vm/probe",
        &case.join("copy-probe.log"),
    )?;
    destination.copy(
        &transport.ca_file,
        VM_TRANSPORT_CA,
        &case.join("copy-ca.log"),
    )?;
    let env = case.join("target-transport");
    write_private(
        &env,
        transport.environment(relay, VM_TRANSPORT_CA)?.as_bytes(),
    )?;
    destination.copy(
        &env,
        "/var/tmp/rustaller-vm/transport",
        &case.join("copy-transport.log"),
    )?;
    prepare_probe_transport(&destination, case, "target")?;
    destination.checked(
        &["chmod", "700", "/var/tmp/rustaller-vm/probe"],
        &case.join("probe-chmod.log"),
        Duration::from_secs(20),
    )?;
    if let Some(endpoint) = local_ipv6 {
        destination.copy(
            &endpoint.cert,
            "/var/tmp/rustaller-vm/ipv6-local.pem",
            &case.join("copy-ipv6-cert.log"),
        )?;
    }
    verify_guest_ipv6(
        &destination,
        target_guest_port,
        ipv6,
        local_ipv6,
        &case.join("target-ipv6.log"),
        deadline,
    )?;
    destination.checked_root(
        &[
            "/var/tmp/rustaller-vm/probe",
            target,
            mode.label(),
            filesystem,
            kernel,
            if kernel_update { "update" } else { "check" },
            VM_TRANSPORT_PATH,
        ],
        &case.join("target-probe.log"),
        remaining(deadline)?,
    )?;
    if kernel_update {
        let before = destination.capture(
            &["cat", "/proc/sys/kernel/random/boot_id"],
            Duration::from_secs(10),
        )?;
        if !before.success {
            return Err("cannot record pre-update reboot identity".into());
        }
        // Reboot through the target's own init, then require a different kernel
        // boot ID before accepting SSH; a still-live old session cannot pass.
        let update_reset_before = guest.resets.load(Ordering::Relaxed);
        let update_firmware_before = read_log(&case.join("firmware-debug.log")).len();
        let _ = destination.capture_root(&["reboot"], Duration::from_secs(15));
        let mut rebooted = false;
        while Instant::now() < deadline {
            if read_log(&case.join("serial.log")).contains("Kernel panic - not syncing:") {
                return Err("kernel update reboot panicked; see serial.log".into());
            }
            if guest.child.try_wait().map_err(|e| e.to_string())?.is_some() {
                break;
            }
            if let Ok(after) = destination.capture(
                &["cat", "/proc/sys/kernel/random/boot_id"],
                Duration::from_secs(8),
            ) && after.success
                && after.stdout != before.stdout
            {
                rebooted = true;
                break;
            }
            thread::sleep(Duration::from_secs(2));
        }
        if !rebooted {
            return Err("kernel update reboot did not restore SSH before deadline".into());
        }
        destination.verify_key_only(unauthorized_key, &case.join("post-update-ssh-policy.log"))?;
        prepare_probe_transport(&destination, case, "post-update")?;
        destination.checked_root(
            &[
                "/var/tmp/rustaller-vm/probe",
                target,
                mode.label(),
                filesystem,
                kernel,
                "check",
                VM_TRANSPORT_PATH,
            ],
            &case.join("post-update-probe.log"),
            remaining(deadline)?,
        )?;
        verify_guest_ipv6(
            &destination,
            target_guest_port,
            ipv6,
            local_ipv6,
            &case.join("post-update-ipv6.log"),
            deadline,
        )?;
        let firmware_log = read_log(&case.join("firmware-debug.log"));
        let check = validate_final_reboot(
            mode,
            guest
                .resets
                .load(Ordering::Relaxed)
                .saturating_sub(update_reset_before),
            firmware_log
                .get(update_firmware_before..)
                .unwrap_or_default(),
        );
        write_private(
            &case.join("post-update-reset-check.log"),
            check.as_ref().unwrap_or_else(|error| error).as_bytes(),
        )?;
        check?;
    }
    if let Some(error) = reset_failure {
        return Err(error);
    }
    let ipv6_result = match ipv6 {
        Ipv6Acceptance::Off => "IPv6 Internet connectivity: NOT TESTED",
        Ipv6Acceptance::Local => {
            "Static dual stack, AAAA, controlled IPv6 TCP/TLS and forwarded IPv6 SSH: PASS\nIPv6 Internet connectivity: NOT VERIFIED"
        }
        Ipv6Acceptance::External => {
            "Static dual stack, AAAA and direct external IPv6 TCP/TLS: PASS\nForwarded IPv6 SSH: PASS"
        }
    };
    write_private(&case.join("accepted.txt"),format!("target={target}\nkernel_package={kernel}\ninit={init}\naccount={account}\nssh_port={target_guest_port}\nkernel_update_requested={kernel_update}\nfirmware={}\nraw_qmp_resets_after_dispatch={resets}\nos_requested_final_reboots=1\nKey-only Live and target SSH, invalid source keys, native package manager, kernel, hostname, {filesystem}/UUID fstab, rustaller-boot artifacts, DNS lookup, IPv4 route and swap: PASS\n{ipv6_result}\n",mode.label()).as_bytes())?;
    guest.stop();
    Ok(())
}

fn prepare_probe_transport(destination: &Ssh, case: &Path, label: &str) -> Result<(), String> {
    destination.checked_root(
        &["mkdir", "-m", "700", "-p", "/run/rustaller-vm"],
        &case.join(format!("{label}-transport-directory.log")),
        Duration::from_secs(20),
    )?;
    destination.checked_root(
        &[
            "install",
            "-m",
            "600",
            "/var/tmp/rustaller-vm/transport",
            VM_TRANSPORT_PATH,
        ],
        &case.join(format!("{label}-transport-install.log")),
        Duration::from_secs(20),
    )
}

fn verify_preparation_reset(
    source: &Ssh,
    arguments: &[String],
    confirmation: &str,
    disk: &str,
    case: &Path,
    deadline: Instant,
) -> Result<(), String> {
    let boot = || {
        source.capture(
            &["cat", "/proc/sys/kernel/random/boot_id"],
            Duration::from_secs(12),
        )
    };
    let table = || {
        source.capture(
            &["lsblk", "-dnro", "PTUUID", "--", disk],
            Duration::from_secs(12),
        )
    };
    let (before_boot, before_table) = (boot()?, table()?);
    if !before_boot.success || !before_table.success || before_table.stdout.trim_ascii().is_empty()
    {
        return Err("cannot record the original guest boot and partition identity".into());
    }
    let reset = [arguments[0].as_str(), "reset"];
    source.checked(
        &reset,
        &case.join("reset-idle.log"),
        Duration::from_secs(12),
    )?;
    let mut prepare = arguments.to_vec();
    prepare.extend([
        "--hold=0".into(),
        "--confirm-erase".into(),
        confirmation.into(),
    ]);
    let result =
        source.capture_owned(&prepare, remaining(deadline)?.min(Duration::from_secs(900)))?;
    write_private(&case.join("reset-prepare.log"), &result.stdout)?;
    if !result.success
        || !String::from_utf8_lossy(&result.stdout).contains("Handoff paused by --hold 0")
    {
        return Err("hold preparation did not complete; see reset-prepare.log".into());
    }
    let state = "/var/tmp/rustaller-handoff/state";
    source.checked(
        &["test", "-s", state],
        &case.join("reset-staged-state.log"),
        Duration::from_secs(12),
    )?;
    source.checked(
        &reset,
        &case.join("reset-cancelled.log"),
        Duration::from_secs(20),
    )?;
    source.checked(
        &reset,
        &case.join("reset-repeat.log"),
        Duration::from_secs(12),
    )?;
    source.checked(
        &["test", "!", "-e", state],
        &case.join("reset-cleared-state.log"),
        Duration::from_secs(12),
    )?;
    let work = source.capture(
        &[
            "find",
            "/var/tmp/rustaller-handoff",
            "-maxdepth",
            "1",
            "-name",
            "operation-*",
        ],
        Duration::from_secs(12),
    )?;
    let kexec = source.capture(
        &["cat", "/sys/kernel/kexec_loaded"],
        Duration::from_secs(12),
    )?;
    let (after_boot, after_table) = (boot()?, table()?);
    if !work.success
        || !work.stdout.is_empty()
        || !kexec.success
        || kexec.stdout.trim_ascii() != b"0"
        || !after_boot.success
        || after_boot.stdout != before_boot.stdout
        || !after_table.success
        || after_table.stdout != before_table.stdout
    {
        return Err(
            "reset did not preserve the original guest boot, disk and clean staging state".into(),
        );
    }
    write_private(&case.join("reset-accepted.txt"),
        b"PASS: idle reset, --hold 0, cancellation and repeated reset. Root SSH, boot ID and partition-table ID unchanged, kexec unloaded and staging removed.\n")
}

fn verify_invalid_client_keys(
    source: &Ssh,
    arguments: &[String],
    confirmation: &str,
    case: &Path,
    deadline: Instant,
) -> Result<(), String> {
    let mut test = arguments
        .iter()
        .filter(|arg| !arg.starts_with("--ssh-key="))
        .cloned()
        .collect::<Vec<_>>();
    // Even if a credential check regresses, keep these source-only tests
    // on the preparation side of kexec.
    test.extend([
        "--hold=0".into(),
        "--confirm-erase".into(),
        confirmation.into(),
    ]);
    for (name, key) in [
        ("missing", None),
        (
            "unreadable",
            Some("--ssh-key=/run/rustaller-vm/nonexistent.pub"),
        ),
        ("malformed", Some("--ssh-key=ssh-ed25519 not-a-public-key")),
    ] {
        let mut command = test.clone();
        command.extend(key.map(str::to_owned));
        let result =
            source.capture_owned(&command, remaining(deadline)?.min(Duration::from_secs(60)))?;
        write_private(&case.join(format!("source-{name}-key.log")), &result.stdout)?;
        let diagnostic = String::from_utf8_lossy(&result.stdout).to_ascii_lowercase();
        if result.success || !diagnostic.contains("key") {
            return Err(format!(
                "source did not reject the {name} SSH key before handoff"
            ));
        }
    }
    source.checked(
        &["test", "!", "-e", "/var/tmp/rustaller-handoff/state"],
        &case.join("source-invalid-key-state.log"),
        remaining(deadline)?.min(Duration::from_secs(12)),
    )
}

fn verify_guest_ipv6(
    guest: &Ssh,
    ssh_port: u16,
    ipv6: Ipv6Acceptance,
    local: Option<&LocalIpv6Endpoint>,
    log: &Path,
    deadline: Instant,
) -> Result<(), String> {
    if ipv6 == Ipv6Acceptance::Off {
        return Ok(());
    }
    let (url, ca) = match local {
        Some(endpoint) => (
            format!("https://[{VM_IPV6_GATEWAY}]:{}/", endpoint.port),
            "/var/tmp/rustaller-vm/ipv6-local.pem",
        ),
        None => (format!("https://{EXTERNAL_IPV6_HOST}/"), "-"),
    };
    let ssh_port_arg = ssh_port.to_string();
    guest.checked(
        &[
            "/var/tmp/rustaller-vm/probe",
            "ipv6",
            VM_IPV6_ADDRESS,
            VM_IPV6_GATEWAY,
            EXTERNAL_IPV6_HOST,
            &url,
            ca,
            &ssh_port_arg,
        ],
        log,
        remaining(deadline)?.min(Duration::from_secs(50)),
    )?;
    let address = VM_IPV6_ADDRESS
        .split_once('/')
        .ok_or("invalid fixture address")?
        .0;
    let connection = guest.ipv6_ssh_connection(address, ssh_port)?;
    let mut evidence = OpenOptions::new()
        .append(true)
        .open(log)
        .map_err(|e| e.to_string())?;
    writeln!(
        evidence,
        "Public-key SSH via an IPv6 target socket: {connection}"
    )
    .map_err(|e| e.to_string())
}

struct Transport {
    ca_file: PathBuf,
    ca: String,
}
impl Transport {
    fn new() -> Result<Self, String> {
        let ca_file = std::env::var_os("SSL_CERT_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| "/etc/ssl/certs/ca-certificates.crt".into());
        regular(&ca_file)?;
        let ca = fs::read_to_string(&ca_file)
            .map_err(|e| format!("cannot read public CA bundle: {e}"))?;
        if ca.len() > 8 * 1024 * 1024
            || ca.contains("PRIVATE KEY")
            || !ca.contains("BEGIN CERTIFICATE")
        {
            return Err("SSL_CERT_FILE must identify a bounded public CA bundle".into());
        }
        Ok(Self { ca_file, ca })
    }
    fn environment(&self, relay: &Relay, ca_path: &str) -> Result<String, String> {
        let mut text = String::new();
        if let Some(url) = relay.guest_url() {
            for name in ["http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY"] {
                text.push_str(&format!("{name}={url}\n"));
            }
        }
        for name in ["SSL_CERT_FILE", "SSL_CA_CERT_FILE", "CURL_CA_BUNDLE"] {
            text.push_str(&format!("{name}={ca_path}\n"));
        }
        if let Ok(value) = std::env::var("NO_PROXY") {
            if value.chars().any(char::is_control) {
                return Err("NO_PROXY contains control characters".into());
            }
            text.push_str(&format!("NO_PROXY={value}\nno_proxy={value}\n"));
        }
        Ok(text)
    }
}
struct Relay {
    stop: Arc<AtomicBool>,
    port: Option<u16>,
    userinfo: String,
}
impl Relay {
    fn start() -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let value = std::env::var("HTTPS_PROXY")
            .or_else(|_| std::env::var("HTTP_PROXY"))
            .ok();
        let Some(value) = value else {
            return Ok(Self {
                stop,
                port: None,
                userinfo: String::new(),
            });
        };
        let authority = value
            .strip_prefix("http://")
            .ok_or("test relay supports the inherited plain HTTP CONNECT proxy only")?
            .trim_end_matches('/');
        if authority.contains('/') || authority.chars().any(char::is_control) {
            return Err("invalid inherited proxy authority".into());
        }
        let (userinfo, address) = authority
            .rsplit_once('@')
            .map(|(user, address)| (format!("{user}@"), address))
            .unwrap_or((String::new(), authority));
        let upstream = if address.contains(':') {
            address.to_owned()
        } else {
            format!("{address}:80")
        };
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let active = Arc::clone(&stop);
        thread::spawn(move || {
            while !active.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((client, _)) => {
                        let upstream = upstream.clone();
                        thread::spawn(move || {
                            let _ = relay_connection(client, &upstream);
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            stop,
            port: Some(port),
            userinfo,
        })
    }
    fn guest_url(&self) -> Option<String> {
        self.port
            .map(|port| format!("http://{}10.0.2.2:{port}", self.userinfo))
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
fn relay_connection(mut client: TcpStream, address: &str) -> io::Result<()> {
    let mut server = TcpStream::connect(address)?;
    let mut client_in = client.try_clone()?;
    let mut server_out = server.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut client_in, &mut server_out);
        let _ = server_out.shutdown(Shutdown::Write);
    });
    let _ = io::copy(&mut server, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    Ok(())
}

fn create_seed(
    tools: &Tools,
    case: &Path,
    public: &str,
    transport: &Transport,
    relay: &Relay,
    networkmanager: bool,
) -> Result<PathBuf, String> {
    let seed = case.join("seed");
    private_directory(&seed)?;
    let mut cloud = format!(
        "#cloud-config\ngrowpart:\n  mode: 'off'\nresize_rootfs: false\nhostname: rustaller-source\nmanage_etc_hosts: true\ndisable_root: false\nssh_pwauth: false\nusers:\n  - name: root\n    lock_passwd: true\n    ssh_authorized_keys:\n      - {}\nwrite_files:\n  - path: /etc/rustaller-session-ca.pem\n    permissions: '0644'\n    content: |\n",
        yaml(public)
    );
    for line in transport.ca.lines() {
        cloud.push_str(&format!("      {line}\n"));
    }
    let env = transport.environment(relay, "/etc/rustaller-session-ca.pem")?;
    cloud.push_str("  - path: /etc/environment\n    permissions: '0600'\n    content: |\n");
    for line in env.lines() {
        cloud.push_str(&format!("      {line}\n"));
    }
    cloud.push_str("  - path: /etc/apt/apt.conf.d/89-rustaller-ca\n    permissions: '0600'\n    content: |\n      Acquire::https::CaInfo \"/etc/rustaller-session-ca.pem\";\n");
    cloud.push_str("  - path: /etc/default/kexec\n    permissions: '0644'\n    content: |\n      LOAD_KEXEC=false\n      USE_GRUB_CONFIG=false\n");
    // The prepared image boots with one explicit networkd owner on the next
    // run. No live interface migration is required during source provisioning.
    cloud.push_str("  - path: /etc/systemd/network/05-rustaller-source.network\n    permissions: '0644'\n    content: |\n      [Match]\n      MACAddress=52:54:00:12:34:56\n      [Network]\n      DHCP=no\n      IPv6AcceptRA=no\n      Address=10.0.2.15/24\n      Gateway=10.0.2.2\n      DNS=10.0.2.3\n");
    if networkmanager {
        cloud.push_str("  - path: /etc/NetworkManager/system-connections/rustaller-source.nmconnection\n    permissions: '0600'\n    content: |\n      [connection]\n      id=rustaller-source\n      uuid=78b9acff-61f4-4e31-ae93-37e341c68e5c\n      type=ethernet\n      [ethernet]\n      mac-address=52:54:00:12:34:56\n      [ipv4]\n      method=manual\n      address1=10.0.2.15/24,10.0.2.2\n      dns=10.0.2.3;\n      [ipv6]\n      method=disabled\n");
        cloud.push_str("runcmd:\n  - [systemctl, mask, --now, networking.service]\n  - [systemctl, stop, systemd-networkd.service, systemd-networkd.socket]\n  - [systemctl, unmask, NetworkManager.service]\n  - [systemctl, enable, --now, NetworkManager.service]\n  - [nmcli, --wait, '30', connection, up, rustaller-source]\n");
    } else {
        cloud.push_str("runcmd:\n  - [systemctl, mask, networking.service, NetworkManager.service]\n  - [systemctl, enable, systemd-networkd.service]\n  - [sh, -c, 'if systemctl is-active --quiet systemd-networkd; then networkctl reload && networkctl reconfigure $(ls /sys/class/net | grep -vx lo) && /usr/lib/systemd/systemd-networkd-wait-online --timeout=60; fi']\n");
    }
    if let Some(proxy) = relay.guest_url() {
        cloud.push_str(&format!(
            "apt:\n  http_proxy: {}\n  https_proxy: {}\n",
            yaml(&proxy),
            yaml(&proxy)
        ));
    }
    cloud.push_str("package_update: true\npackage_upgrade: false\npackages:\n  - curl\n  - ca-certificates\n  - gnupg\n  - cpio\n  - kexec-tools\n  - fdisk\n  - util-linux\n  - openssh-client\n  - gzip\n  - tar\n");
    if networkmanager {
        cloud.push_str("  - network-manager\n");
    }
    write_private(&seed.join("user-data"), cloud.as_bytes())?;
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    write_private(
        &seed.join("meta-data"),
        format!("instance-id: rustaller-{id}\nlocal-hostname: rustaller-source\n").as_bytes(),
    )?;
    let iso = case.join("seed.iso");
    let mut command = tools.program("xorriso");
    command
        .args([
            "-as", "mkisofs", "-quiet", "-volid", "cidata", "-joliet", "-rock", "-output",
        ])
        .arg(&iso)
        .arg(seed.join("user-data"))
        .arg(seed.join("meta-data"));
    checked(&mut command, &case.join("seed-build.log"))?;
    fs::set_permissions(&iso, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    Ok(iso)
}
fn configure_firmware(
    command: &mut Command,
    tools: &Tools,
    case: &Path,
    mode: Firmware,
) -> Result<(), String> {
    match mode {
        Firmware::Bios => {
            command
                .arg("-bios")
                .arg(tools.root.join("usr/share/seabios/bios-256k.bin"));
        }
        Firmware::Efi => {
            let code = tools.root.join("usr/share/OVMF/OVMF_CODE_4M.fd");
            let vars = case.join("OVMF_VARS.fd");
            fs::copy(tools.root.join("usr/share/OVMF/OVMF_VARS_4M.fd"), &vars)
                .map_err(|e| e.to_string())?;
            fs::set_permissions(&vars, fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
            command.arg("-drive").arg(format!(
                "if=pflash,format=raw,unit=0,readonly=on,file={}",
                code.display()
            ));
            command.arg("-drive").arg(format!(
                "if=pflash,format=raw,unit=1,file={}",
                vars.display()
            ));
        }
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn start_disk_guest(
    tools: &Tools,
    case: &Path,
    disk: &Path,
    seed: &Path,
    firmware: Firmware,
    port: u16,
    target_ssh: Option<(u16, u16)>,
    target: &str,
    bus: DiskBus,
    ipv6: bool,
) -> Result<Guest, String> {
    let mut command = tools.qemu();
    configure_firmware(&mut command, tools, case, firmware)?;
    command.arg("-drive").arg(format!(
        "if=none,id=system,format=qcow2,file={}",
        disk.display()
    ));
    bus.attach(
        &mut command,
        &format!("rustaller-{target}-{}", firmware.label()),
    );
    command.arg("-drive").arg(format!(
        "file={},format=raw,if=ide,index=2,media=cdrom,readonly=on",
        seed.display()
    ));
    let mut network = format!("user,id=net0,hostfwd=tcp:127.0.0.1:{port}-:22");
    if let Some((host, guest)) = target_ssh {
        network.push_str(&format!(",hostfwd=tcp:127.0.0.1:{host}-:{guest}"));
    }
    if ipv6 {
        network.push_str(&format!(
            ",ipv4=on,ipv6=on,ipv6-net={VM_IPV6_PREFIX},ipv6-host={VM_IPV6_GATEWAY}"
        ));
    }
    command.arg("-netdev").arg(network).args([
        "-device",
        "virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56,romfile=",
    ]);
    Guest::start(command, case)
}
fn check_base(tools: &Tools, base: &Path, output: &Path) -> Result<(), String> {
    let data = tools
        .program("qemu-img")
        .args(["info", "--output=json"])
        .arg(base)
        .output()
        .map_err(|e| e.to_string())?;
    write_private(&output.join("base-info.json"), &data.stdout)?;
    if !data.status.success() || String::from_utf8_lossy(&data.stdout).contains("backing-filename")
    {
        return Err(
            "source image must be a standalone qemu-readable regular image without a backing chain"
                .into(),
        );
    }
    Ok(())
}
fn create_overlay(tools: &Tools, base: &Path, case: &Path) -> Result<PathBuf, String> {
    let disk = case.join("disk.qcow2");
    let mut command = tools.program("qemu-img");
    command
        .args(["create", "-f", "qcow2", "-F", "qcow2", "-b"])
        .arg(base)
        .arg(&disk)
        .arg("8G");
    checked(&mut command, &case.join("disk-create.log"))?;
    Ok(disk)
}

struct Guest {
    monitor: UnixStream,
    child: Child,
    resets: Arc<AtomicU64>,
}
impl Guest {
    fn start(mut command: Command, case: &Path) -> Result<Self, String> {
        let serial = case.join("serial.log");
        let _ = private_file(&serial)?;
        command
            .arg("-serial")
            .arg(format!("file:{}", serial.display()));
        // SeaBIOS turns a soft guest reset into a second hard reset. Keep
        // firmware evidence so acceptance distinguishes this from an OS loop.
        let firmware_debug = case.join("firmware-debug.log");
        let _ = private_file(&firmware_debug)?;
        command
            .arg("-debugcon")
            .arg(format!("file:{}", firmware_debug.display()))
            .args(["-global", "isa-debugcon.iobase=0x402"]);
        let qmp = case.join("qmp.sock");
        command
            .arg("-qmp")
            .arg(format!("unix:{},server=on,wait=off", qmp.display()));
        command
            .stdin(Stdio::null())
            .stdout(private_file(&case.join("qemu.stdout"))?)
            .stderr(private_file(&case.join("qemu.stderr"))?);
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start disposable QEMU: {e}"))?;
        let resets = Arc::new(AtomicU64::new(0));
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match UnixStream::connect(&qmp) {
                Ok(stream) => break stream,
                Err(_) => {
                    if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                        return Err(
                            "QEMU exited before its monitor was available; see qemu.stderr".into(),
                        );
                    }
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err("QEMU monitor startup timed out".into());
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        };
        let counter = Arc::clone(&resets);
        let log = private_file(&case.join("qmp.log"))?;
        let monitor = stream.try_clone().map_err(|e| e.to_string())?;
        thread::spawn(move || record_qmp(stream, counter, log));
        Ok(Self {
            child,
            resets,
            monitor,
        })
    }
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
    fn wait_exit(&mut self, timeout: Duration) -> Result<bool, String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Ok(true);
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(false)
    }
}
impl Drop for Guest {
    fn drop(&mut self) {
        self.stop();
    }
}
fn validate_final_reboot(mode: Firmware, resets: u64, firmware: &str) -> Result<String, String> {
    const SOFT_TO_HARD: &str = "In resume (status=0)\nIn 32bit resume\nAttempting a hard reboot\n";
    let conversions = firmware.matches(SOFT_TO_HARD).count();
    let valid = (resets == 1 && conversions == 0)
        || (mode == Firmware::Bios
            && resets == 2
            && conversions == 1
            && firmware.contains("SeaBIOS (version")
            && !firmware.contains("Unable to hard-reboot machine"));
    if !valid {
        return Err(format!(
            "expected one OS-requested firmware reboot; raw QMP RESETs={resets}, SeaBIOS soft-to-hard sequences={conversions}; see qmp.log and firmware-debug.log"
        ));
    }
    Ok(format!(
        "PASS: one OS-requested firmware reboot; raw QMP RESETs={resets}; SeaBIOS soft-to-hard sequences={conversions}\n"
    ))
}

fn record_qmp(mut stream: UnixStream, resets: Arc<AtomicU64>, mut log: File) {
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    let mut lines = BufReader::new(reader).lines();
    if let Some(Ok(line)) = lines.next() {
        let _ = writeln!(log, "{line}");
    }
    let _ = stream.write_all(b"{\"execute\":\"qmp_capabilities\"}\r\n");
    for line in lines.map_while(Result::ok) {
        let compact = line
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect::<String>();
        if compact.contains("\"event\":\"RESET\"") {
            resets.fetch_add(1, Ordering::Relaxed);
        }
        let _ = writeln!(log, "{line}");
    }
}

struct Ssh {
    port: u16,
    key: PathBuf,
    known: PathBuf,
    account: &'static str,
}
struct UnauthorizedKey(PathBuf);
impl UnauthorizedKey {
    fn generate(directory: &Path) -> Result<Self, String> {
        let key = Self(directory.join("unauthorized-test-key"));
        checked(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&key.0),
            &directory.join("unauthorized-test-keygen.log"),
        )?;
        Ok(key)
    }
}
impl Drop for UnauthorizedKey {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(self.0.with_extension("pub"));
    }
}
struct Captured {
    success: bool,
    stdout: Vec<u8>,
}
impl Ssh {
    fn connection_options(&self, command: &mut Command) {
        command.args([
            "-F",
            "/dev/null",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "ConnectionAttempts=1",
        ]);
        command
            .arg("-o")
            .arg(format!("UserKnownHostsFile={}", self.known.display()));
    }
    fn common(&self, command: &mut Command) {
        self.connection_options(command);
        command.args(["-i"]).arg(&self.key).args([
            "-o",
            "BatchMode=yes",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "KbdInteractiveAuthentication=no",
            "-o",
            "PreferredAuthentications=publickey",
            "-o",
            "LogLevel=ERROR",
        ]);
    }
    fn verify_key_only(&self, unauthorized: &Path, log: &Path) -> Result<(), String> {
        let auth = self.capture(&["id", "-un"], Duration::from_secs(8))?;
        if !auth.success || String::from_utf8_lossy(&auth.stdout).trim() != self.account {
            return Err("authorized key did not authenticate as the selected administrator".into());
        }
        let privileged = self.capture_root(&["id", "-u"], Duration::from_secs(8))?;
        if !privileged.success || String::from_utf8_lossy(&privileged.stdout).trim() != "0" {
            return Err(
                "installed administrator cannot run privileged commands without a password".into(),
            );
        }
        let connection = format!("user={},host=localhost,addr=127.0.0.1", self.account);
        let effective = self.capture_root(
            &["/usr/sbin/sshd", "-T", "-C", &connection],
            Duration::from_secs(8),
        )?;
        let effective_config = String::from_utf8_lossy(&effective.stdout);
        let root_login = effective_config
            .lines()
            .filter_map(|line| line.split_once(' '))
            .find_map(|(name, value)| {
                name.eq_ignore_ascii_case("permitrootlogin")
                    .then_some(value)
            });
        let expected_root = if self.account == "root" {
            matches!(root_login, Some("prohibit-password" | "without-password"))
        } else {
            root_login == Some("no")
        };
        if !effective.success
            || !expected_root
            || ![
                "pubkeyauthentication yes",
                "passwordauthentication no",
                "kbdinteractiveauthentication no",
                "permitemptypasswords no",
                "authenticationmethods publickey",
            ]
            .iter()
            .all(|expected| {
                effective_config
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case(expected))
            })
        {
            write_private(log, &effective.stdout)?;
            return Err(format!(
                "effective OpenSSH authentication policy differs; see {}",
                log.display()
            ));
        }
        let ownership = self.capture(
            &["sh", "-c", "uid=$(id -u); test \"$(stat -c '%a:%u' ~/.ssh)\" = \"700:$uid\" && test \"$(stat -c '%a:%u' ~/.ssh/authorized_keys)\" = \"600:$uid\""],
            Duration::from_secs(8),
        )?;
        if !ownership.success {
            write_private(log, &ownership.stdout)?;
            return Err("effective authorized_keys ownership or permissions differ".into());
        }
        let mut evidence = format!(
            "Authorized public-key login as {} and privileged id: PASS\nEffective OpenSSH configuration and authorized_keys modes/ownership: PASS\n",
            self.account
        );
        for (name, account, method, key) in [
            ("unauthorized key", self.account, "publickey", unauthorized),
            ("password", self.account, "password", self.key.as_path()),
            (
                "keyboard-interactive",
                self.account,
                "keyboard-interactive",
                self.key.as_path(),
            ),
            ("root login", "root", "publickey", self.key.as_path()),
        ] {
            if name == "root login" && self.account == "root" {
                continue;
            }
            let mut command = Command::new("ssh");
            self.connection_options(&mut command);
            command
                .args([
                    "-v",
                    "-o",
                    "IdentityAgent=none",
                    "-o",
                    "NumberOfPasswordPrompts=0",
                    "-o",
                    &format!("PreferredAuthentications={method}"),
                    "-i",
                ])
                .arg(key)
                .arg("-p")
                .arg(self.port.to_string())
                .arg(format!("{account}@127.0.0.1"))
                .arg("true")
                .stdin(Stdio::null());
            command.env("LC_ALL", "C");
            let auth = capture_timeout(command, Duration::from_secs(8))?;
            if auth.success || !exclusively_public_key(&String::from_utf8_lossy(&auth.stdout)) {
                write_private(log, &auth.stdout)?;
                return Err(format!(
                    "SSH accepted or did not firmly reject {name}; see {}",
                    log.display()
                ));
            }
            evidence.push_str(&format!("{name} denied; only publickey offered: PASS\n"));
        }
        write_private(log, evidence.as_bytes())
    }
    fn command_owned(&self, args: &[String]) -> Command {
        let mut command = Command::new("ssh");
        self.common(&mut command);
        command
            .arg("-p")
            .arg(self.port.to_string())
            .arg(format!("{}@127.0.0.1", self.account))
            .arg(
                args.iter()
                    .map(|value| quote(value))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        command.stdin(Stdio::null());
        command
    }
    fn ipv6_ssh_connection(&self, address: &str, port: u16) -> Result<String, String> {
        // SLIRP hostfwd accepts IPv4 only. The test probe relays the second
        // SSH stream inside the disposable guest without changing sshd policy.
        let connector = self.command_owned(&[
            "/var/tmp/rustaller-vm/probe".into(),
            "relay-ipv6".into(),
            address.into(),
            port.to_string(),
        ]);
        let proxy = connector
            .get_args()
            .map(|arg| quote(&arg.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ");
        let mut ssh = Command::new("ssh");
        self.common(&mut ssh);
        ssh.args([
            "-6",
            "-p",
            &port.to_string(),
            "-o",
            &format!("ProxyCommand=ssh {proxy}"),
            "-o",
            &format!("HostKeyAlias=[127.0.0.1]:{}", self.port),
            &format!("{}@{address}", self.account),
            "printenv SSH_CONNECTION",
        ])
        .stdin(Stdio::null());
        let result = capture_timeout(ssh, Duration::from_secs(16))?;
        let output = String::from_utf8_lossy(&result.stdout);
        let fields: Vec<_> = output.split_whitespace().collect();
        if !result.success
            || fields.len() != 4
            || fields[0].parse::<std::net::Ipv6Addr>().is_err()
            || fields[1].parse::<u16>().is_err()
            || fields[2].parse::<std::net::Ipv6Addr>().ok() != address.parse().ok()
            || fields[3].parse::<u16>().ok() != Some(port)
        {
            return Err("public-key SSH did not report the expected target IPv6 socket".into());
        }
        Ok(fields.join(" "))
    }
    fn capture_owned(&self, args: &[String], timeout: Duration) -> Result<Captured, String> {
        capture_timeout(self.command_owned(args), timeout)
    }
    fn capture(&self, args: &[&str], timeout: Duration) -> Result<Captured, String> {
        self.capture_owned(
            &args.iter().map(|arg| (*arg).into()).collect::<Vec<_>>(),
            timeout,
        )
    }
    fn capture_root(&self, args: &[&str], timeout: Duration) -> Result<Captured, String> {
        if self.account == "root" {
            self.capture(args, timeout)
        } else {
            self.capture(&[&["sudo", "-n", "--"][..], args].concat(), timeout)
        }
    }
    fn checked(&self, args: &[&str], log: &Path, timeout: Duration) -> Result<(), String> {
        let result = self.capture(args, timeout)?;
        write_private(log, &result.stdout)?;
        if !result.success {
            return Err(format!(
                "guest command {} failed; see {}",
                args.first().copied().unwrap_or(""),
                log.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
        Ok(())
    }
    fn checked_root(&self, args: &[&str], log: &Path, timeout: Duration) -> Result<(), String> {
        if self.account == "root" {
            self.checked(args, log, timeout)
        } else {
            self.checked(&[&["sudo", "-n", "--"][..], args].concat(), log, timeout)
        }
    }
    fn wait_ready(&self, guest: &mut Guest, deadline: Instant) -> Result<(), String> {
        while Instant::now() < deadline {
            if guest.child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err("source guest exited before SSH became ready".into());
            }
            if self
                .capture(&["true"], Duration::from_secs(8))
                .is_ok_and(|result| result.success)
            {
                return Ok(());
            }
            thread::sleep(Duration::from_secs(1));
        }
        Err("source guest SSH did not become ready before deadline".into())
    }
    fn copy(&self, source: &Path, destination: &str, log: &Path) -> Result<(), String> {
        let mut command = Command::new("scp");
        self.common(&mut command);
        command
            .arg("-O")
            .arg("-P")
            .arg(self.port.to_string())
            .arg(source)
            .arg(format!("{}@127.0.0.1:{destination}", self.account));
        let result = capture_timeout(command, Duration::from_secs(90))?;
        write_private(log, &result.stdout)?;
        if !result.success {
            return Err("copy to disposable guest failed".into());
        }
        Ok(())
    }
}
fn exclusively_public_key(output: &str) -> bool {
    let mut methods = output
        .lines()
        .filter_map(|line| line.strip_prefix("debug1: Authentications that can continue: "));
    methods.next() == Some("publickey")
        && methods.all(|method| method == "publickey")
        && output
            .lines()
            .any(|line| line.ends_with("Permission denied (publickey)."))
}
fn capture_timeout(mut command: Command, timeout: Duration) -> Result<Captured, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("missing command stdout")?;
    let stderr = child.stderr.take().ok_or("missing command stderr")?;
    let read = |mut stream: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut data = Vec::new();
            let _ = stream
                .by_ref()
                .take(16 * 1024 * 1024)
                .read_to_end(&mut data);
            data
        })
    };
    let out = read(Box::new(stdout));
    let err = read(Box::new(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("bounded guest command timed out".into());
        }
        thread::sleep(Duration::from_millis(50));
    };
    let mut stdout = out.join().map_err(|_| "stdout reader failed")?;
    stdout.extend(err.join().map_err(|_| "stderr reader failed")?);
    Ok(Captured {
        success: status.success(),
        stdout,
    })
}

fn build_probe(output: &Path, log: &Path) -> Result<(), String> {
    let source = Path::new(file!())
        .parent()
        .ok_or("cannot locate guest probe")?
        .join("vm_guest.rs");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let mut command = Command::new(rustc);
    command
        .args([
            "--edition=2024",
            "--target",
            "x86_64-unknown-linux-musl",
            "-C",
            "opt-level=s",
            "-C",
            "panic=abort",
            "-C",
            "strip=symbols",
        ])
        .arg(source)
        .arg("-o")
        .arg(output);
    checked(&mut command, log)
}
fn checked(command: &mut Command, log: &Path) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|e| format!("cannot start verification utility: {e}"))?;
    let mut text = output.stdout;
    text.extend(output.stderr);
    write_private(log, &text)?;
    if !output.status.success() {
        return Err(format!(
            "verification utility failed; see {}",
            log.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    Ok(())
}
fn hash(path: &Path, program: &str, length: usize) -> Result<String, String> {
    regular(path)?;
    let output = Command::new(program)
        .arg("--")
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8(output.stdout).map_err(|_| "invalid checksum output")?;
    let value = text.split_whitespace().next().ok_or("missing checksum")?;
    if !output.status.success()
        || value.len() != length
        || !value.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("invalid checksum result".into());
    }
    Ok(value.to_owned())
}
fn verify_hash(path: &Path, expected: &str, program: &str, length: usize) -> Result<(), String> {
    if expected.len() != length
        || !expected.bytes().all(|b| b.is_ascii_hexdigit())
        || !hash(path, program, length)?.eq_ignore_ascii_case(expected)
    {
        return Err(
            "VM source checksum does not match the authenticated publisher metadata".into(),
        );
    }
    Ok(())
}
fn public_key(path: &Path) -> Result<String, String> {
    regular(path)?;
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let text = text.trim();
    if text.len() > 8192 || text.chars().any(char::is_control) || !text.starts_with("ssh-ed25519 ")
    {
        return Err("acceptance requires one bounded ed25519 public key".into());
    }
    Ok(text.into())
}
fn regular(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(format!(
            "VM input must be a nonempty regular file, never a device/symlink: {}",
            path.display()
        ));
    }
    Ok(())
}
fn private_directory(path: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|e| format!("cannot create fresh private result directory: {e}"))
}
fn private_file(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    Ok(file)
}
fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    private_file(path)?
        .write_all(data)
        .map_err(|e| e.to_string())
}
fn read_log(path: &Path) -> String {
    // Firmware/kernel console writes can interleave inside UTF-8 sequences.
    // Keep the raw file as evidence, but never discard every ASCII stage marker
    // just because one console byte is invalid UTF-8.
    fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}
fn free_port() -> Result<u16, String> {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|socket| socket.local_addr())
        .map(|addr| addr.port())
        .map_err(|e| e.to_string())
}
fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| "VM acceptance deadline expired".into())
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn yaml(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_device_paths_and_duplicate_or_unknown_flags() {
        assert!(
            Options::parse(&[
                "smoke".into(),
                "--output".into(),
                "/tmp/a".into(),
                "--output".into(),
                "/tmp/b".into()
            ])
            .is_err()
        );
        assert!(Options::parse(&["smoke".into(), "--drive".into(), "/dev/sda".into()]).is_err());
        assert!(regular(Path::new("/dev/null")).is_err());
    }
    #[test]
    fn qemu_acceleration_and_memory_are_explicit() {
        for (accel, expected, cpu) in [("tcg", "tcg,thread=multi", "max"), ("kvm", "kvm", "host")] {
            let tools = Tools {
                root: "/unused".into(),
                accel: accel.into(),
                memory_mib: "1536".into(),
            };
            let command = tools.qemu();
            let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
            assert!(args.windows(2).any(|a| a == ["-accel", expected]));
            assert!(args.windows(2).any(|a| a == ["-cpu", cpu]));
            assert!(args.windows(2).any(|a| a == ["-m", "1536"]));
        }
    }

    #[test]
    fn reboot_validation_requires_exact_firmware_evidence_not_time_debouncing() {
        let conversion = "In resume (status=0)\nIn 32bit resume\nAttempting a hard reboot\nSeaBIOS (version 1.16.3)\n";
        assert!(validate_final_reboot(Firmware::Bios, 1, "").is_ok());
        assert!(validate_final_reboot(Firmware::Efi, 1, "").is_ok());
        assert!(validate_final_reboot(Firmware::Bios, 2, conversion).is_ok());
        for (mode, resets, log) in [
            (Firmware::Bios, 0, ""),
            (Firmware::Bios, 2, ""),
            (Firmware::Efi, 2, conversion),
            (Firmware::Bios, 1, conversion),
            (Firmware::Bios, 3, conversion),
            (Firmware::Bios, 2, "Attempting a hard reboot\n"),
        ] {
            assert!(validate_final_reboot(mode, resets, log).is_err());
        }
        assert!(validate_final_reboot(Firmware::Bios, 4, &conversion.repeat(2)).is_err());
        assert!(
            validate_final_reboot(
                Firmware::Bios,
                2,
                &format!("{conversion}Unable to hard-reboot machine")
            )
            .is_err()
        );
    }

    #[test]
    fn console_markers_survive_interleaved_invalid_utf8() {
        let path = std::env::temp_dir().join(format!(
            "rustaller-console-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        write_private(
            &path,
            b"systemd \xe2[kernel interleave]\nRUSTALLER_INSTALL_COMPLETE\n\xff",
        )
        .unwrap();
        assert!(read_log(&path).contains("RUSTALLER_INSTALL_COMPLETE"));
        assert!(fs::read(&path).unwrap().contains(&0xff));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn quotes_remote_native_arguments_without_shell_substitution() {
        assert_eq!(quote("a'$(secret)"), "'a'\\''$(secret)'");
        assert_eq!(yaml("a'b"), "'a''b'");
    }
    #[test]
    fn validates_firmware_and_bounded_deadlines() {
        let mut options = Options {
            action: "smoke".into(),
            values: BTreeMap::new(),
        };
        assert_eq!(
            options.firmware().unwrap(),
            vec![Firmware::Bios, Firmware::Efi]
        );
        options.values.insert("--timeout".into(), "0".into());
        assert!(options.timeout().is_err());
    }
    #[test]
    fn storage_buses_use_matching_guest_names_and_explicit_serials() {
        let mut options = Options {
            action: "install".into(),
            values: BTreeMap::new(),
        };
        for (name, device, controller) in [
            ("virtio-blk", "/dev/vda", "virtio-blk-pci"),
            ("virtio-scsi", "/dev/sda", "virtio-scsi-pci"),
            ("sata", "/dev/sda", "ide-hd"),
            ("nvme", "/dev/nvme0n1", "nvme"),
            ("pvscsi", "/dev/sda", "pvscsi"),
        ] {
            options.values.insert("--disk-bus".into(), name.into());
            let bus = options.disk_bus().unwrap();
            assert_eq!(bus.device(), device);
            let mut command = Command::new("unused-qemu");
            bus.attach(&mut command, "test-serial");
            let args = command
                .get_args()
                .map(|s| s.to_str().unwrap())
                .collect::<Vec<_>>();
            assert!(args.iter().any(|arg| arg.starts_with(controller)));
            assert!(args.iter().any(|arg| arg.contains("serial=test-serial")));
        }
        options
            .values
            .insert("--disk-bus".into(), "arbitrary".into());
        assert!(options.disk_bus().is_err());
    }
    #[test]
    fn account_assignment_covers_both_modes_per_filesystem_and_firmware() {
        let mut options = Options::parse(&["install".into()]).unwrap();
        for (mode, filesystem, account) in [
            (Firmware::Bios, "xfs", "root"),
            (Firmware::Efi, "ext4", "root"),
            (Firmware::Bios, "ext4", "rustaller-admin"),
            (Firmware::Efi, "xfs", "rustaller-admin"),
        ] {
            assert_eq!(options.admin_account(mode, filesystem).unwrap(), account);
        }
        options.values.insert("--account".into(), "admin".into());
        assert_eq!(
            options.admin_account(Firmware::Bios, "xfs").unwrap(),
            "rustaller-admin"
        );
        for removed in ["--kernel", "--gentoo-init", "--password", "--root-password"] {
            assert!(Options::parse(&["install".into(), removed.into(), "any".into()]).is_err());
        }
    }

    #[test]
    fn kernel_update_and_boot_capacity_are_bounded() {
        let mut options = Options::parse(&["install".into()]).unwrap();
        assert!(!options.kernel_update().unwrap());
        options
            .values
            .insert("--kernel-update".into(), "yes".into());
        assert!(options.kernel_update().unwrap());
        options
            .values
            .insert("--boot-size-mib".into(), "1024".into());
        assert_eq!(options.boot_size_mib().unwrap(), Some(1024));
        options
            .values
            .insert("--boot-size-mib".into(), "128".into());
        assert!(options.boot_size_mib().is_err());
    }

    #[test]
    fn filesystem_matrix_accepts_two_roots_and_rejects_btrfs() {
        let mut options = Options::parse(&["install".into()]).unwrap();
        assert_eq!(options.filesystems().unwrap(), ["xfs"]);
        options.values.insert("--filesystem".into(), "all".into());
        assert_eq!(options.filesystems().unwrap(), ["xfs", "ext4"]);
        for filesystem in ["xfs", "ext4"] {
            options
                .values
                .insert("--filesystem".into(), filesystem.into());
            assert_eq!(options.filesystems().unwrap(), [filesystem]);
        }
        options.values.insert("--filesystem".into(), "btrfs".into());
        assert!(options.filesystems().is_err());
    }

    #[test]
    fn authentication_probe_distinguishes_denial_from_connection_failure() {
        let public_key_only = "debug1: Authentications that can continue: publickey
root@127.0.0.1: Permission denied (publickey).
";
        assert!(exclusively_public_key(public_key_only));
        assert!(!exclusively_public_key(
            &public_key_only.replace("continue: publickey", "continue: publickey,password")
        ));
        assert!(!exclusively_public_key(
            "ssh: connect to host 127.0.0.1 port 22: Connection refused"
        ));
    }
}
