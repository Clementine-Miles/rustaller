//! Standalone static guest probe used only by the disposable VM harness.
//! It is not linked into either production binary.
use std::fs;
use std::net::ToSocketAddrs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("VM acceptance probe: {error}");
            ExitCode::FAILURE
        }
    }
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3
        || !matches!(args[0].as_str(), "alpine" | "debian" | "arch" | "gentoo")
        || !matches!(args[1].as_str(), "bios" | "efi")
    {
        return Err("expected TARGET bios|efi TRANSPORT_FILE".into());
    }
    let target = &args[0];
    let firmware = &args[1];
    let transport = load_transport(Path::new(&args[2]))?;
    let os = fs::read_to_string("/etc/os-release").map_err(|e| e.to_string())?;
    let id = os
        .lines()
        .find_map(|line| line.strip_prefix("ID="))
        .map(|id| id.trim_matches('"'))
        .ok_or("missing os-release ID")?;
    if id != target {
        return Err(format!("expected {target}, got {id}"));
    }
    let actual_hostname = run_capture("hostname", &[], &transport)?;
    if actual_hostname.trim() != format!("rustaller-{target}") {
        return Err("target hostname is not applied".into());
    }
    if Path::new("/sys/firmware/efi").is_dir() != (firmware == "efi") {
        return Err("guest firmware does not match requested boot mode".into());
    }
    if run_capture("uname", &["-m"], &transport)?.trim() != "x86_64" {
        return Err("target architecture is not x86_64".into());
    }
    let kernel = run_capture("uname", &["-r"], &transport)?;
    if target == "debian" && !kernel.contains("cloud") {
        return Err("Debian target did not boot its selected cloud kernel".into());
    }
    if target == "alpine" && !kernel.contains("virt") {
        return Err("Alpine target did not boot its selected virt kernel".into());
    }
    let mounts = fs::read_to_string("/proc/mounts").map_err(|e| e.to_string())?;
    if !mounts.lines().any(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        fields.get(1) == Some(&"/") && fields.get(2) == Some(&"xfs")
    }) {
        return Err("target root is not xfs".into());
    }
    let fstab = fs::read_to_string("/etc/fstab").map_err(|e| e.to_string())?;
    // Read-only post-boot diagnostics; activation belongs to native init.
    println!("{}", run_capture("free", &["-h"], &transport)?);
    println!("{}", run_capture("swapon", &["--show"], &transport)?);
    println!("{fstab}");
    if !fstab
        .lines()
        .any(|line| line.starts_with("UUID=") && line.split_whitespace().nth(1) == Some("/"))
    {
        return Err("root fstab lacks its UUID binding".into());
    }
    if fstab
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("/swapfile"))
        .count()
        != 1
    {
        return Err("swapfile fstab entry is not unique".into());
    }
    let swap = fs::read_to_string("/proc/swaps").map_err(|e| e.to_string())?;
    if !swap
        .lines()
        .skip(1)
        .any(|line| line.split_whitespace().next() == Some("/swapfile"))
    {
        return Err("target swapfile was not activated during boot".into());
    }
    if fs::metadata("/swapfile")
        .map_err(|e| e.to_string())?
        .permissions()
        .mode()
        & 0o777
        != 0o600
    {
        return Err("swapfile permissions differ from 0600".into());
    }
    if fs::read_to_string("/proc/sys/vm/swappiness")
        .map_err(|e| e.to_string())?
        .trim()
        != "10"
    {
        return Err("target swappiness was not applied".into());
    }
    if !Path::new("/boot/grub/grub.cfg").is_file() {
        return Err("target GRUB configuration is absent".into());
    }
    if firmware == "efi" {
        let efi = if matches!(target.as_str(), "alpine" | "debian") {
            "/boot/efi/EFI/BOOT/BOOTX64.EFI"
        } else {
            "/efi/EFI/BOOT/BOOTX64.EFI"
        };
        if !Path::new(efi).is_file() {
            return Err("UEFI removable boot loader is absent".into());
        }
    }
    let route = run_capture("ip", &["-4", "route", "show", "default"], &transport)?;
    if route.trim().is_empty() {
        return Err("target has no default IPv4 route".into());
    }
    let resolv = fs::read_to_string("/etc/resolv.conf").map_err(|e| e.to_string())?;
    if !resolv
        .lines()
        .any(|line| line.split_whitespace().next() == Some("nameserver"))
    {
        return Err("target DNS resolver configuration is absent".into());
    }
    if !("dl-cdn.alpinelinux.org", 443)
        .to_socket_addrs()
        .map_err(|e| format!("target DNS lookup failed: {e}"))?
        .any(|address| !address.ip().is_unspecified())
    {
        return Err("target DNS lookup returned no address".into());
    }
    let cmdline = fs::read_to_string("/proc/cmdline").map_err(|e| e.to_string())?;
    if ["password=", "http_proxy=", "https_proxy=", "transport.env"]
        .iter()
        .any(|value| cmdline.to_ascii_lowercase().contains(value))
    {
        return Err("private installation values appeared in the kernel command line".into());
    }
    println!(
        "RUSTALLER_VM_TARGET_BOOT target={target} firmware={firmware} kernel={}",
        kernel.trim()
    );
    println!(
        "RUSTALLER_VM_TARGET_NETWORK IPv4 default route and DNS lookup passed; public-key-only SSH authenticated by host harness"
    );
    println!("RUSTALLER_VM_TARGET_SWAP file active, 0600, fstab unique, swappiness=10");
    let (program, arguments): (&str, &[&str]) = match target.as_str() {
        "alpine" => ("apk", &["update"]),
        "debian" => ("apt-get", &["update"]),
        "arch" => ("pacman", &["-Sy", "--noconfirm"]),
        "gentoo" => ("emerge-webrsync", &[]),
        _ => unreachable!(),
    };
    fs::write(
        "/run/rustaller-vm/wgetrc",
        "ca_certificate = /etc/ssl/certs/rustaller-vm-ca.pem\n",
    )
    .map_err(|e| e.to_string())?;
    let status = Command::new(program)
        .args(arguments)
        .envs(transport.iter().map(|(key, value)| (key, value)))
        .env("WGETRC", "/run/rustaller-vm/wgetrc")
        .status()
        .map_err(|e| format!("cannot start native package update: {e}"))?;
    if !status.success() {
        return Err(format!(
            "native package update failed with {:?}",
            status.code()
        ));
    }
    println!("RUSTALLER_VM_ACCEPTANCE_PASS target={target} firmware={firmware}");
    Ok(())
}
fn load_transport(path: &Path) -> Result<Vec<(String, String)>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > 65536
    {
        return Err("transport file must be a private root-owned regular file".into());
    }
    let mut result = Vec::new();
    for line in fs::read_to_string(path).map_err(|e| e.to_string())?.lines() {
        let (key, value) = line.split_once('=').ok_or("invalid transport file")?;
        if !matches!(
            key,
            "http_proxy"
                | "https_proxy"
                | "HTTP_PROXY"
                | "HTTPS_PROXY"
                | "NO_PROXY"
                | "no_proxy"
                | "SSL_CERT_FILE"
                | "SSL_CA_CERT_FILE"
                | "CURL_CA_BUNDLE"
        ) || value.chars().any(char::is_control)
        {
            return Err("unsupported transport setting".into());
        }
        result.push((key.to_owned(), value.to_owned()));
    }
    Ok(result)
}
fn run_capture(
    program: &str,
    args: &[&str],
    transport: &[(String, String)],
) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .envs(transport.iter().map(|(key, value)| (key, value)))
        .output()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    if !output.status.success() {
        return Err(format!("{program} failed with {:?}", output.status.code()));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("{program} returned non-UTF8 data"))
}
