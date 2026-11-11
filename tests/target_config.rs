use rustaller::command::{CommandInput, OutputMode};
use rustaller::model::{BootMode, InstallPlan, Target};
use rustaller::postinstall::KernelChoice;
use rustaller::targets::{debian, packages};
use std::path::Path;

fn plan(target: Target) -> InstallPlan {
    let mut plan = InstallPlan::new(target);
    plan.confhome = Some("https://example.org/runtime".into());
    plan
}

#[test]
fn default_plan_adapters_preserve_existing_kernel_and_package_selection() {
    for no_cloud in [false, true] {
        let mut alpine_plan = plan(Target::Alpine);
        alpine_plan.no_cloud_kernel = no_cloud;
        for is_vm in [false, true] {
            let configuration =
                packages::configuration_for_plan(&alpine_plan, Target::Alpine, is_vm).unwrap();
            assert_eq!(
                configuration.kernel.alpine_flavor,
                Some(if !no_cloud && is_vm { "virt" } else { "lts" })
            );
            assert!(configuration.extra_packages.is_empty());
        }
        let mut debian_plan = plan(Target::Debian);
        debian_plan.no_cloud_kernel = no_cloud;
        for boot_mode in [BootMode::Bios, BootMode::Efi] {
            assert_eq!(
                debian::apt_package_list_for_plan(&debian_plan, boot_mode, &[]).unwrap(),
                debian::apt_package_list(no_cloud, boot_mode, "root", &[])
            );
        }
    }
    for boot_mode in [BootMode::Bios, BootMode::Efi] {
        assert_eq!(
            packages::emerge_package_set_for_plan(&plan(Target::Gentoo), boot_mode, "ext4", &[])
                .unwrap(),
            packages::emerge_package_set(boot_mode, "ext4", "root", &[])
        );
    }
    let command =
        packages::kernel_install_command_for_plan(&plan(Target::Arch), Path::new("/os")).unwrap();
    assert_eq!(command.program, "chroot");
    assert_eq!(
        command.args,
        ["/os", "pacman", "-Syu", "--noconfirm", "linux"]
    );
    assert_eq!(command.stdin, CommandInput::Null);
    assert_eq!(command.stdout_mode, OutputMode::Stream);
    assert_eq!(command.stderr_mode, OutputMode::Stream);
    assert!(command.cwd.is_none());
    assert!(command.env.is_empty());
}

#[test]
fn official_kernel_and_timezone_choices_reach_the_target_specific_package_phase() {
    let mut alpine_plan = plan(Target::Alpine);
    alpine_plan.post_install.kernel = KernelChoice::Cloud;
    alpine_plan.post_install.timezone = Some("Asia/Tokyo".into());
    let configuration =
        packages::configuration_for_plan(&alpine_plan, Target::Alpine, false).unwrap();
    assert_eq!(configuration.kernel.alpine_flavor, Some("virt"));
    assert_eq!(configuration.extra_packages, ["tzdata"]);

    let mut debian_plan = plan(Target::Debian);
    debian_plan.post_install.kernel = KernelChoice::Generic;
    debian_plan.post_install.timezone = Some("Etc/UTC".into());
    let packages = debian::apt_package_list_for_plan(&debian_plan, BootMode::Bios, &[]).unwrap();
    assert_eq!(packages[5], "linux-image-amd64");
    assert!(
        !packages
            .iter()
            .any(|package| package == "linux-image-cloud-amd64")
    );
    assert_eq!(packages.last().unwrap(), "tzdata");

    let mut arch_plan = plan(Target::Arch);
    arch_plan.post_install.kernel = KernelChoice::Lts;
    arch_plan.post_install.timezone = Some("Etc/UTC".into());
    let command = packages::kernel_install_command_for_plan(&arch_plan, Path::new("/os")).unwrap();
    assert_eq!(
        command.args,
        [
            "/os",
            "pacman",
            "-Syu",
            "--noconfirm",
            "linux-lts",
            "tzdata"
        ]
    );
    assert!(
        !packages::pacstrap_packages(BootMode::Bios, "ext4", "root", &[])
            .iter()
            .any(|package| package.starts_with("linux"))
    );

    let mut gentoo_plan = plan(Target::Gentoo);
    gentoo_plan.post_install.kernel = KernelChoice::Standard;
    gentoo_plan.post_install.timezone = Some("Etc/UTC".into());
    let packages =
        packages::emerge_package_set_for_plan(&gentoo_plan, BootMode::Bios, "ext4", &[]).unwrap();
    assert_eq!(
        packages
            .iter()
            .filter(|package| *package == "sys-kernel/gentoo-kernel-bin")
            .count(),
        1
    );
    assert_eq!(packages.last().unwrap(), "sys-libs/timezone-data");
}

#[test]
fn plan_adapters_reject_wrong_targets_rescue_and_invalid_plans_before_producing_actions() {
    for target in [
        Target::Alpine,
        Target::Debian,
        Target::Arch,
        Target::Gentoo,
        Target::LocalIso,
        Target::NetbootXyz,
        Target::Reset,
    ] {
        let mut candidate = plan(target);
        candidate.hold = Some(1);
        assert!(packages::configuration_for_plan(&candidate, Target::Alpine, true).is_err());
        assert!(debian::apt_package_list_for_plan(&candidate, BootMode::Bios, &[]).is_err());
        assert!(packages::kernel_install_command_for_plan(&candidate, Path::new("/os")).is_err());
        assert!(
            packages::emerge_package_set_for_plan(&candidate, BootMode::Bios, "ext4", &[]).is_err()
        );
        candidate.hold = None;
        if target != Target::Alpine {
            assert!(packages::configuration_for_plan(&candidate, Target::Alpine, true).is_err());
        }
        if target != Target::Debian {
            assert!(debian::apt_package_list_for_plan(&candidate, BootMode::Bios, &[]).is_err());
        }
        if target != Target::Arch {
            assert!(
                packages::kernel_install_command_for_plan(&candidate, Path::new("/os")).is_err()
            );
        }
        if target != Target::Gentoo {
            assert!(
                packages::emerge_package_set_for_plan(&candidate, BootMode::Bios, "ext4", &[])
                    .is_err()
            );
        }
    }
    let mut invalid = plan(Target::Arch);
    invalid.schema_version += 1;
    assert!(packages::kernel_install_command_for_plan(&invalid, Path::new("/os")).is_err());
    let mut invalid = plan(Target::Debian);
    invalid.confhome = Some("http://invalid-resource-base.example".into());
    assert!(debian::apt_package_list_for_plan(&invalid, BootMode::Bios, &[]).is_err());
}

#[test]
fn pacstrap_package_order_matches_boot_mode_filesystem_and_user() {
    let firmware = vec!["intel-ucode".to_owned()];
    assert_eq!(
        packages::pacstrap_packages(BootMode::Efi, "btrfs", "admin", &firmware),
        [
            "base",
            "grub",
            "openssh",
            "efibootmgr",
            "dosfstools",
            "btrfs-progs",
            "sudo",
            "intel-ucode"
        ]
    );
    assert_eq!(
        packages::pacstrap_packages(BootMode::Bios, "ext4", "root", &[]),
        ["base", "grub", "openssh", "e2fsprogs"]
    );
    assert_eq!(
        packages::pacstrap_packages(BootMode::Bios, "xfs", "root", &[]),
        ["base", "grub", "openssh", "xfsprogs"]
    );
}

#[test]
fn gentoo_packages_preserve_systemd_kernel_efi_filesystem_and_sudo_selection() {
    let firmware = vec!["sys-kernel/linux-firmware".to_owned()];
    assert_eq!(
        packages::emerge_package_set(BootMode::Efi, "xfs", "admin", &firmware),
        [
            "sys-block/io-scheduler-udev-rules",
            "sys-fs/dosfstools",
            "sys-boot/efibootmgr",
            "sys-fs/xfsprogs",
            "app-admin/sudo",
            "sys-kernel/linux-firmware",
            "sys-kernel/gentoo-kernel-bin"
        ]
    );
    assert_eq!(
        packages::emerge_package_set(BootMode::Bios, "ext4", "root", &[]),
        [
            "sys-block/io-scheduler-udev-rules",
            "sys-fs/e2fsprogs",
            "sys-kernel/gentoo-kernel-bin"
        ]
    );
    assert_eq!(packages::grub_platforms(BootMode::Efi), "efi-64");
    assert_eq!(packages::grub_platforms(BootMode::Bios), "pc");
}
