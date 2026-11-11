use rustaller::command::{CommandOutput, MockRunner};
use rustaller::model::BootMode;
use rustaller::targets::debian::{
    apt_package_list, apt_sources_text, bootstrap_rootfs, debian_kernel_package, grub_selections,
    install_apt_packages, install_grub, preseed_grub, write_apt_sources, write_fstab,
};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("rustaller-debian-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).expect("create scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn success_outputs(count: usize) -> Vec<CommandOutput> {
    (0..count)
        .map(|_| CommandOutput::new(Some(0), Vec::new(), Vec::new()))
        .collect()
}

#[test]
fn apt_sources_and_kernel_selection_match_the_debian_stable_contract() {
    assert_eq!(
        apt_sources_text("stablecodename", None).unwrap(),
        "# Debian stable repositories\ndeb https://deb.debian.org/debian stablecodename main contrib non-free non-free-firmware\ndeb https://deb.debian.org/debian stablecodename-updates main contrib non-free non-free-firmware\ndeb https://security.debian.org/debian-security stablecodename-security main contrib non-free non-free-firmware\n"
    );
    assert_eq!(debian_kernel_package(false), "linux-image-cloud-amd64");
    assert_eq!(debian_kernel_package(true), "linux-image-amd64");
    assert!(apt_sources_text("bookworm1", None).is_err());
    assert!(apt_sources_text("bookworm", Some("http://mirror.example/debian")).is_err());
}

#[test]
fn apt_sources_replace_atomically_and_remove_the_deb822_default() {
    let scratch = Scratch::new();
    let apt = scratch.0.join("etc/apt");
    fs::create_dir_all(apt.join("sources.list.d")).unwrap();
    fs::write(apt.join("sources.list"), b"old source\n").unwrap();
    fs::write(
        apt.join("sources.list.d/debian.sources"),
        b"old deb822 source\n",
    )
    .unwrap();

    write_apt_sources(&scratch.0, "bookworm", None).unwrap();

    assert_eq!(
        fs::read(apt.join("sources.list")).unwrap(),
        apt_sources_text("bookworm", None).unwrap().as_bytes()
    );
    assert!(!apt.join("sources.list.d/debian.sources").exists());
}

#[test]
fn apt_install_order_and_package_selection_preserve_networkd_systemd_requirements() {
    let firmware = vec!["firmware-linux".to_owned()];
    assert_eq!(
        apt_package_list(false, BootMode::Efi, "admin", &firmware),
        [
            "ca-certificates",
            "systemd-sysv",
            "systemd-resolved",
            "openssh-server",
            "xfsprogs",
            "linux-image-cloud-amd64",
            "grub-efi-amd64",
            "efibootmgr",
            "dosfstools",
            "sudo",
            "firmware-linux"
        ]
    );
    assert_eq!(
        apt_package_list(true, BootMode::Bios, "root", &[]),
        [
            "ca-certificates",
            "systemd-sysv",
            "systemd-resolved",
            "openssh-server",
            "xfsprogs",
            "linux-image-amd64",
            "grub-pc"
        ]
    );

    let scratch = Scratch::new();
    let mut runner = MockRunner::new(success_outputs(2));
    install_apt_packages(
        &mut runner,
        &scratch.0,
        &apt_package_list(false, BootMode::Efi, "admin", &firmware),
    )
    .unwrap();
    assert_eq!(runner.calls().len(), 2);
    assert_eq!(runner.calls()[0].args[1..], ["apt-get", "update"]);
    assert_eq!(
        runner.calls()[1].args[1..5],
        ["apt-get", "install", "-y", "--no-install-recommends"]
    );
    assert_eq!(runner.calls()[1].args[5], "ca-certificates");
    assert_eq!(
        runner.calls()[0]
            .env
            .get(std::ffi::OsStr::new("DEBIAN_FRONTEND")),
        Some(&std::ffi::OsString::from("noninteractive"))
    );
}

#[test]
fn fstab_uses_only_validated_root_and_optional_efi_uuids_without_swap() {
    let scratch = Scratch::new();
    write_fstab(
        &scratch.0,
        "1234abcd-0000-1111-2222-abcdefabcdef",
        Some("efef1234-1111-2222-3333-abcdefabcdef"),
        BootMode::Efi,
    )
    .unwrap();
    let efi = String::from_utf8(fs::read(scratch.0.join("etc/fstab")).unwrap()).unwrap();
    assert_eq!(
        efi,
        "# <file system> <mount point> <type> <options> <dump> <pass>\nUUID=1234abcd-0000-1111-2222-abcdefabcdef / xfs defaults 0 0\nUUID=efef1234-1111-2222-3333-abcdefabcdef /boot/efi vfat umask=0077 0 1\n"
    );
    assert!(!efi.contains("swap"));

    write_fstab(&scratch.0, "aabbccdd", None, BootMode::Bios).unwrap();
    let bios = fs::read_to_string(scratch.0.join("etc/fstab")).unwrap();
    assert!(bios.contains("UUID=aabbccdd / xfs"));
    assert!(!bios.contains("/boot/efi"));
    assert!(write_fstab(&scratch.0, "bad/uuid", None, BootMode::Bios).is_err());
}

#[test]
fn grub_preseed_preserves_bios_and_nvram_safe_uefi_inputs() {
    assert_eq!(
        grub_selections(BootMode::Efi, "ignored").unwrap(),
        b"grub-efi-amd64 grub-efi/install_devices_empty boolean true\ngrub-efi-amd64 grub-efi/install_devices multiselect\n"
    );
    assert_eq!(
        grub_selections(BootMode::Bios, "nvme0n1").unwrap(),
        b"grub-pc grub-pc/install_devices multiselect /dev/nvme0n1\n"
    );
    assert!(grub_selections(BootMode::Bios, "../../dev/sda").is_err());

    let scratch = Scratch::new();
    let mut runner = MockRunner::new(success_outputs(1));
    preseed_grub(&mut runner, &scratch.0, BootMode::Efi, "").unwrap();
    assert_eq!(runner.calls().len(), 1);
    assert_eq!(runner.calls()[0].program, "chroot");
    assert_eq!(runner.calls()[0].args[1], "debconf-set-selections");
    assert_eq!(
        runner.calls()[0].stdin,
        rustaller::command::CommandInput::Bytes(grub_selections(BootMode::Efi, "").unwrap())
    );
}

#[test]
fn debian_bootstrap_uses_the_required_minbase_debootstrap_arguments_in_order() {
    let scratch = Scratch::new();
    let mut runner = MockRunner::new(success_outputs(2));
    bootstrap_rootfs(&mut runner, &scratch.0, "bookworm", None).unwrap();

    assert_eq!(runner.calls().len(), 2);
    assert_eq!(runner.calls()[0].program, "apk");
    assert_eq!(runner.calls()[0].args, ["add", "debootstrap"]);
    assert_eq!(runner.calls()[1].program, "debootstrap");
    assert_eq!(
        runner.calls()[1]
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        [
            "--arch=amd64",
            "--variant=minbase",
            "--include=ca-certificates,debconf-utils",
            "bookworm",
            scratch.0.to_str().unwrap(),
            "https://deb.debian.org/debian"
        ]
    );
    assert_eq!(
        runner.calls()[1].cwd.as_deref().unwrap().to_str(),
        Some("/")
    );
}

#[test]
fn grub_install_order_is_bios_uefi_specific_and_forces_no_nvram_write() {
    let scratch = Scratch::new();
    fs::create_dir_all(scratch.0.join("etc/default")).unwrap();
    fs::write(scratch.0.join("etc/default/grub"), b"GRUB_TIMEOUT=5\n").unwrap();
    let mut runner = MockRunner::new(success_outputs(2));

    install_grub(
        &mut runner,
        &scratch.0,
        BootMode::Efi,
        "ignored",
        "console=ttyS0,115200n8 console=tty0",
    )
    .unwrap();

    assert_eq!(runner.calls().len(), 2);
    assert_eq!(runner.calls()[0].program, "chroot");
    assert_eq!(
        runner.calls()[0]
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        [
            scratch.0.to_str().unwrap(),
            "grub-install",
            "--target=x86_64-efi",
            "--efi-directory=/boot/efi",
            "--bootloader-id=debian",
            "--no-nvram",
            "--removable"
        ]
    );
    assert!(
        fs::read_to_string(scratch.0.join("etc/default/grub"))
            .unwrap()
            .ends_with(
                "GRUB_CMDLINE_LINUX=\"$GRUB_CMDLINE_LINUX console=ttyS0,115200n8 console=tty0\"\n"
            )
    );
    assert_eq!(runner.calls()[1].args[1], "grub-mkconfig");
}
