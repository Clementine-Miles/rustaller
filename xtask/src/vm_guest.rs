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
    if args.len() != 7
        || !matches!(args[0].as_str(), "alpine" | "debian" | "arch" | "gentoo")
        || !matches!(args[1].as_str(), "bios" | "efi")
        || !matches!(args[2].as_str(), "xfs" | "ext4" | "btrfs")
        || !matches!(args[4].as_str(), "native" | "openrc" | "systemd")
        || !matches!(args[5].as_str(), "check" | "update")
    {
        return Err(
            "expected TARGET bios|efi xfs|ext4|btrfs KERNEL INIT check|update TRANSPORT_FILE"
                .into(),
        );
    }
    let target = &args[0];
    let firmware = &args[1];
    let filesystem = &args[2];
    let selected_kernel = &args[3];
    let selected_init = &args[4];
    let update = args[5] == "update";
    let transport = load_transport(Path::new(&args[6]))?;
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
    if Path::new("/sys/firmware/boot").is_dir() != (firmware == "efi") {
        return Err("guest firmware does not match requested boot mode".into());
    }
    if run_capture("uname", &["-m"], &transport)?.trim() != "x86_64" {
        return Err("target architecture is not x86_64".into());
    }
    let kernel = run_capture("uname", &["-r"], &transport)?;
    verify_kernel(target, selected_kernel, kernel.trim(), &transport)?;
    if target == "gentoo" {
        verify_gentoo_init(selected_init, &transport)?;
    }
    let mounts = fs::read_to_string("/proc/mounts").map_err(|e| e.to_string())?;
    if !mounts.lines().any(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        fields.get(1) == Some(&"/") && fields.get(2) == Some(&filesystem.as_str())
    }) {
        return Err(format!("target root is not {filesystem}"));
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
    if filesystem != "btrfs" {
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
    } else if Path::new("/swapfile").exists() || !fstab.contains("subvol=@,compress=zstd:3") {
        return Err("Btrfs requires @ with compression and no swapfile".into());
    }
    if !mounts.lines().any(|line| {
        let f: Vec<_> = line.split_whitespace().collect();
        f.get(1) == Some(&"/boot") && f.get(2) == Some(&"vfat")
    }) {
        return Err("FAT32 /boot is not mounted".into());
    }
    let state = fs::read_to_string("/etc/rustaller/boot-state").map_err(|e| e.to_string())?;
    let boot_uuid = state.lines().nth(3).ok_or("boot state lacks FAT UUID")?;
    if boot_uuid.is_empty()
        || !run_capture(
            "findmnt",
            &["-nro", "UUID", "--mountpoint", "/boot"],
            &transport,
        )?
        .trim()
        .eq_ignore_ascii_case(boot_uuid)
    {
        return Err("FAT32 /boot UUID does not match recorded identity".into());
    }
    if !fstab.lines().any(|line| {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        fields.first() == Some(&format!("UUID={boot_uuid}").as_str())
            && fields.get(1) == Some(&"/boot")
            && fields.get(2) == Some(&"vfat")
    }) {
        return Err("fstab does not bind FAT32 /boot to expected UUID".into());
    }
    let limine = fs::read_to_string("/boot/limine.conf").map_err(|e| e.to_string())?;
    verify_limine(&limine, filesystem)?;
    if !Path::new("/usr/local/libexec/rustaller-limine-worker").is_file() {
        return Err("Limine refresh worker absent".into());
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
    println!("RUSTALLER_VM_TARGET_SWAP policy verified for {filesystem}");
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
    if update {
        native_kernel_update(target, selected_kernel, &transport)?;
    }
    println!("RUSTALLER_VM_ACCEPTANCE_PASS target={target} firmware={firmware}");
    Ok(())
}
fn verify_limine(limine: &str, filesystem: &str) -> Result<(), String> {
    if !limine.contains("protocol: linux")
        || !limine.contains("root=UUID=")
        || !limine.contains("console=ttyS0")
        || (filesystem == "btrfs" && !limine.contains("rootflags=subvol=@"))
    {
        return Err("Limine configuration is incomplete".into());
    }
    let mut payloads = 0;
    for path in limine.lines().filter_map(|line| {
        line.trim()
            .strip_prefix("path: boot():/")
            .or_else(|| line.trim().strip_prefix("module_path: boot():/"))
    }) {
        if path.is_empty()
            || Path::new(path)
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("unsafe Limine payload path".into());
        }
        let metadata =
            fs::symlink_metadata(Path::new("/boot").join(path)).map_err(|e| e.to_string())?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err("Limine payload absent or empty".into());
        }
        payloads += 1;
    }
    if payloads < 2 {
        return Err("no complete kernel/initramfs references".into());
    }
    if !Path::new("/boot/EFI/BOOT/BOOTX64.EFI").is_file()
        || !Path::new("/boot/limine-bios.sys").is_file()
    {
        return Err("Limine firmware payload absent".into());
    }
    Ok(())
}
fn kernel_package(target: &str, choice: &str) -> Result<&'static str, String> {
    match (target, choice) {
        ("alpine", "virt" | "cloud") => Ok("linux-virt"),
        ("alpine", "auto" | "standard" | "lts" | "generic") => Ok("linux-lts"),
        ("debian", "cloud") => Ok("linux-image-cloud-amd64"),
        ("debian", "auto" | "standard" | "generic") => Ok("linux-image-amd64"),
        ("arch", "auto" | "standard") => Ok("linux"),
        ("arch", "lts") => Ok("linux-lts"),
        ("arch", "hardened") => Ok("linux-hardened"),
        ("arch", "zen") => Ok("linux-zen"),
        ("arch", "cachyos") => Ok("linux-cachyos"),
        ("gentoo", "auto" | "standard") => Ok("sys-kernel/gentoo-kernel-bin"),
        _ => Err("invalid selected kernel family".into()),
    }
}
fn verify_kernel(
    target: &str,
    choice: &str,
    release: &str,
    transport: &[(String, String)],
) -> Result<(), String> {
    let package = kernel_package(target, choice)?;
    let correct = match target {
        "alpine" => release.ends_with(if package == "linux-virt" {
            "-virt"
        } else {
            "-lts"
        }),
        "debian" => {
            release.ends_with("-amd64")
                && release.contains("cloud") == (package == "linux-image-cloud-amd64")
        }
        "arch" => fs::read_to_string(format!("/usr/lib/modules/{release}/pkgbase"))
            .is_ok_and(|value| value.trim() == package),
        "gentoo" => {
            release.contains("gentoo-dist")
                && fs::read_dir("/var/db/pkg/sys-kernel")
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .any(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with("gentoo-kernel-bin-")
                    })
        }
        _ => false,
    };
    if !correct {
        return Err(format!(
            "running kernel {release} does not match selected {package}"
        ));
    }
    let (program, args): (&str, Vec<&str>) = match target {
        "alpine" => ("apk", vec!["info", "--exists", package]),
        "debian" => ("dpkg-query", vec!["-W", "-f=${Status}", package]),
        "arch" => ("pacman", vec!["-Q", package]),
        "gentoo" => return Ok(()),
        _ => unreachable!(),
    };
    let installed = run_capture(program, &args, transport)?;
    if target == "debian" && installed.trim() != "install ok installed" {
        return Err("selected kernel metapackage is not installed".into());
    }
    Ok(())
}
fn verify_gentoo_init(init: &str, transport: &[(String, String)]) -> Result<(), String> {
    let pid1 = fs::read_to_string("/proc/1/comm").map_err(|e| e.to_string())?;
    if (pid1.trim() == "systemd") != (init == "systemd") {
        return Err("Gentoo PID1 differs from selected init".into());
    }
    let profile = fs::canonicalize("/etc/portage/make.profile").map_err(|e| e.to_string())?;
    let profile = profile.to_string_lossy();
    if !profile.contains("/no-multilib") || profile.contains("/systemd") != (init == "systemd") {
        return Err("Gentoo profile does not match nomultilib/init policy".into());
    }
    if run_capture("portageq", &["envvar", "ABI_X86"], transport)?
        .split_whitespace()
        .collect::<Vec<_>>()
        != ["64"]
    {
        return Err("Gentoo ABI_X86 must contain only 64".into());
    }
    if init == "openrc" {
        run_capture("rc-service", &["sshd", "status"], transport)?;
        let runlevel = run_capture("rc-update", &["show", "default"], transport)?;
        if !runlevel.contains("net.") || !runlevel.contains("sshd") {
            return Err("Gentoo OpenRC network/SSH services are not enabled".into());
        }
    } else if init == "systemd" {
        run_capture(
            "systemctl",
            &["is-active", "sshd", "systemd-networkd", "systemd-resolved"],
            transport,
        )?;
    } else {
        return Err("Gentoo init must be explicit".into());
    }
    Ok(())
}
fn native_kernel_update(
    target: &str,
    choice: &str,
    transport: &[(String, String)],
) -> Result<(), String> {
    let package = kernel_package(target, choice)?;
    let before = fs::read_to_string("/boot/limine.conf").map_err(|e| e.to_string())?;
    match target {
        "alpine" => {
            run_capture("apk", &["upgrade", "--available", package], transport)?;
            for entry in fs::read_dir("/lib/modules").map_err(|e| e.to_string())? {
                let release = entry.map_err(|e| e.to_string())?.file_name();
                let release = release.to_str().ok_or("invalid module release")?;
                run_capture("mkinitfs", &[release], transport)?;
            }
        }
        "debian" => {
            run_capture(
                "apt-get",
                &["install", "-y", "--only-upgrade", package],
                transport,
            )?;
            run_capture("update-initramfs", &["-u", "-k", "all"], transport)?;
        }
        "arch" => {
            run_capture("pacman", &["-Syu", "--noconfirm"], transport)?;
            run_capture("mkinitcpio", &["-P", "-S", "autodetect"], transport)?;
        }
        "gentoo" => {
            run_capture(
                "emerge",
                &["--getbinpkg", "--usepkg", "--update", package],
                transport,
            )?;
            run_capture(
                "dracut",
                &["--regenerate-all", "--force", "--no-hostonly"],
                transport,
            )?;
            // Standalone dracut is explicitly followed by Rustaller refresh;
            // Gentoo package installkernel hooks cover kernel transactions.
            run_capture(
                "/usr/local/libexec/rustaller-limine-worker",
                &["limine-refresh"],
                transport,
            )?;
        }
        _ => unreachable!(),
    }
    run_capture(
        "/usr/local/libexec/rustaller-limine-worker",
        &["limine-check"],
        transport,
    )?;
    let after = fs::read_to_string("/boot/limine.conf").map_err(|e| e.to_string())?;
    let previous = fs::read_to_string("/boot/limine.conf.previous").map_err(|e| e.to_string())?;
    if after == before {
        return Err("native initramfs regeneration did not publish a new Limine generation".into());
    }
    verify_limine(&after, "")?;
    verify_limine(&previous, "")?;
    println!(
        "RUSTALLER_VM_KERNEL_UPDATE transaction/rebuild completed; version change depends on repository candidates"
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_package_matrix_rejects_cross_distribution_choices() {
        for (target, choice, package) in [
            ("alpine", "lts", "linux-lts"),
            ("alpine", "virt", "linux-virt"),
            ("debian", "generic", "linux-image-amd64"),
            ("debian", "cloud", "linux-image-cloud-amd64"),
            ("arch", "standard", "linux"),
            ("arch", "lts", "linux-lts"),
            ("arch", "hardened", "linux-hardened"),
            ("arch", "zen", "linux-zen"),
            ("gentoo", "standard", "sys-kernel/gentoo-kernel-bin"),
        ] {
            assert_eq!(kernel_package(target, choice).unwrap(), package);
        }
        assert!(kernel_package("debian", "hardened").is_err());
        assert!(kernel_package("gentoo", "cloud").is_err());
    }
}
