mod support;
mod debian {
    use rustaller::command::{CommandOutput, MockRunner};
    use rustaller::model::BootMode;
    use rustaller::targets::debian::{
        apt_package_list, apt_sources_text, bootstrap_rootfs, debian_kernel_package,
        install_apt_packages, write_apt_sources,
    };
    use std::fs;

    use crate::support::Scratch;

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
        let scratch = Scratch::new("debian");
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
                "dosfstools"
            ]
        );

        let scratch = Scratch::new("debian");
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
    fn debian_bootstrap_uses_the_required_minbase_debootstrap_arguments_in_order() {
        let scratch = Scratch::new("debian");
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
                "--include=ca-certificates",
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
}

mod target_config {
    use rustaller::command::{CommandInput, OutputMode};
    use rustaller::model::{BootMode, InstallPlan, Target};
    use rustaller::postinstall::KernelChoice;
    use rustaller::targets::{debian, packages};
    use std::path::Path;

    #[test]
    fn default_plan_adapters_preserve_existing_kernel_and_package_selection() {
        for no_cloud in [false, true] {
            let mut alpine_plan = InstallPlan::new(Target::Alpine);
            alpine_plan.no_cloud_kernel = no_cloud;
            for is_vm in [false, true] {
                let configuration =
                    packages::configuration_for_plan(&alpine_plan, Target::Alpine, is_vm).unwrap();
                assert_eq!(configuration.kernel.alpine_flavor, Some("lts"));
                assert!(configuration.extra_packages.is_empty());
            }
            let mut debian_plan = InstallPlan::new(Target::Debian);
            debian_plan.no_cloud_kernel = no_cloud;
            for boot_mode in [BootMode::Bios, BootMode::Efi] {
                assert_eq!(
                    debian::apt_package_list_for_plan(&debian_plan, boot_mode, &[]).unwrap(),
                    debian::apt_package_list(true, boot_mode, "root", &[])
                );
            }
        }
        for boot_mode in [BootMode::Bios, BootMode::Efi] {
            assert_eq!(
                packages::emerge_package_set_for_plan(
                    &InstallPlan::new(Target::Gentoo),
                    boot_mode,
                    "ext4",
                    &[]
                )
                .unwrap(),
                packages::emerge_package_set(boot_mode, "ext4", "root", &[])
            );
        }
        let command = packages::kernel_install_command_for_plan(
            &InstallPlan::new(Target::Arch),
            Path::new("/os"),
        )
        .unwrap();
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
        let mut alpine_plan = InstallPlan::new(Target::Alpine);
        alpine_plan.post_install.kernel = KernelChoice::Cloud;
        alpine_plan.post_install.timezone = Some("Asia/Tokyo".into());
        let configuration =
            packages::configuration_for_plan(&alpine_plan, Target::Alpine, false).unwrap();
        assert_eq!(configuration.kernel.alpine_flavor, Some("virt"));
        assert_eq!(configuration.extra_packages, ["tzdata"]);

        let mut debian_plan = InstallPlan::new(Target::Debian);
        debian_plan.post_install.kernel = KernelChoice::Generic;
        debian_plan.post_install.timezone = Some("Etc/UTC".into());
        let packages =
            debian::apt_package_list_for_plan(&debian_plan, BootMode::Bios, &[]).unwrap();
        assert_eq!(packages[5], "linux-image-amd64");
        assert!(
            !packages
                .iter()
                .any(|package| package == "linux-image-cloud-amd64")
        );
        assert_eq!(packages.last().unwrap(), "tzdata");

        let mut arch_plan = InstallPlan::new(Target::Arch);
        arch_plan.post_install.kernel = KernelChoice::Lts;
        arch_plan.post_install.timezone = Some("Etc/UTC".into());
        let command =
            packages::kernel_install_command_for_plan(&arch_plan, Path::new("/os")).unwrap();
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

        let mut gentoo_plan = InstallPlan::new(Target::Gentoo);
        gentoo_plan.post_install.kernel = KernelChoice::Standard;
        gentoo_plan.post_install.timezone = Some("Etc/UTC".into());
        let packages =
            packages::emerge_package_set_for_plan(&gentoo_plan, BootMode::Bios, "ext4", &[])
                .unwrap();
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
            Target::Reset,
        ] {
            let mut candidate = InstallPlan::new(target);
            candidate.hold = Some(1);
            assert!(packages::configuration_for_plan(&candidate, Target::Alpine, true).is_err());
            assert!(debian::apt_package_list_for_plan(&candidate, BootMode::Bios, &[]).is_err());
            assert!(
                packages::kernel_install_command_for_plan(&candidate, Path::new("/os")).is_err()
            );
            assert!(
                packages::emerge_package_set_for_plan(&candidate, BootMode::Bios, "ext4", &[])
                    .is_err()
            );
            candidate.hold = None;
            if target != Target::Alpine {
                assert!(
                    packages::configuration_for_plan(&candidate, Target::Alpine, true).is_err()
                );
            }
            if target != Target::Debian {
                assert!(
                    debian::apt_package_list_for_plan(&candidate, BootMode::Bios, &[]).is_err()
                );
            }
            if target != Target::Arch {
                assert!(
                    packages::kernel_install_command_for_plan(&candidate, Path::new("/os"))
                        .is_err()
                );
            }
            if target != Target::Gentoo {
                assert!(
                    packages::emerge_package_set_for_plan(&candidate, BootMode::Bios, "ext4", &[])
                        .is_err()
                );
            }
        }
        let mut invalid = InstallPlan::new(Target::Arch);
        invalid.schema_version += 1;
        assert!(packages::kernel_install_command_for_plan(&invalid, Path::new("/os")).is_err());
        let mut invalid = InstallPlan::new(Target::Debian);
        invalid.ssh_port = Some(0);
        assert!(debian::apt_package_list_for_plan(&invalid, BootMode::Bios, &[]).is_err());
    }

    #[test]
    fn pacstrap_package_order_matches_boot_mode_filesystem_and_user() {
        let firmware = vec!["intel-ucode".to_owned()];
        assert_eq!(
            packages::pacstrap_packages(BootMode::Efi, "btrfs", "admin", &firmware),
            [
                "base",
                "dosfstools",
                "openssh",
                "btrfs-progs",
                "sudo",
                "intel-ucode"
            ]
        );
        assert_eq!(
            packages::pacstrap_packages(BootMode::Bios, "ext4", "root", &[]),
            ["base", "dosfstools", "openssh", "e2fsprogs"]
        );
        assert_eq!(
            packages::pacstrap_packages(BootMode::Bios, "xfs", "root", &[]),
            ["base", "dosfstools", "openssh", "xfsprogs"]
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
                "sys-fs/dosfstools",
                "sys-fs/e2fsprogs",
                "sys-kernel/gentoo-kernel-bin"
            ]
        );
    }
}

mod cachyos {
    use crate::support::Scratch;
    use rustaller::command::{CommandOutput, MockRunner};
    use rustaller::targets::cachyos::{self, IsaLevel, RELEASE_FINGERPRINT};

    fn root() -> Scratch {
        let dir = Scratch::with_etc("cachyos");
        dir.write("etc/arch-release", "");
        dir
    }
    fn ok() -> CommandOutput {
        CommandOutput::new(Some(0), vec![], vec![])
    }
    #[test]
    fn isa_selection_uses_os_usable_glibc_hwcaps_instead_of_cpu_flags() {
        assert_eq!(
            cachyos::isa_level("x86-64-v4 (supported)\nx86-64-v3 (supported, searched)\n"),
            IsaLevel::V3
        );
        assert_eq!(
            cachyos::isa_level("x86-64-v4 (supported, searched)\nx86-64-v3 (supported, searched)"),
            IsaLevel::V4
        );
        assert_eq!(
            cachyos::isa_level("x86-64-v2 (supported, searched)"),
            IsaLevel::V2
        );
        assert_eq!(
            cachyos::isa_level("AVX512 avx2 x86-64-v4\n"),
            IsaLevel::Baseline
        );
    }
    #[test]
    fn release_key_matches_exactly_one_pinned_primary_not_a_short_id_or_subkey() {
        let good = format!(
            "pub:::::::::\nfpr:::::::::{RELEASE_FINGERPRINT}:\nsub:::::::::\nfpr:::::::::OTHER:\n"
        );
        assert!(cachyos::valid_primary_key(&good));
        assert!(!cachyos::valid_primary_key(
            &good.replace(RELEASE_FINGERPRINT, "F3B607488DB35A47")
        ));
        assert!(!cachyos::valid_primary_key(&format!(
            "{good}pub:::::::::\nfpr:::::::::OTHER:\n"
        )));
        assert!(!cachyos::valid_primary_key(&format!(
            "sub:::::::::\nfpr:::::::::{RELEASE_FINGERPRINT}:\n"
        )));
    }
    #[test]
    fn audited_official_script_is_noninteractive_and_uses_pinned_full_key() {
        let original = concat!(
            "pacman-key --recv-keys F3B607488DB35A47 --keyserver keyserver.ubuntu.com\n",
            "pacman-key --lsign-key F3B607488DB35A47\n",
            "gawk -i inplace -f $gawk_script $pacman_conf_cachyos || true\n",
            "pacman -U \"${mirror_url}package\"\n",
            "    pacman -Syu\n",
        );
        let adapted = cachyos::adapt_official_script(original).unwrap();
        assert!(adapted.contains("pacman-key --add ./rustaller-cachyos.asc"));
        assert!(adapted.contains(&format!("pacman-key --lsign-key {RELEASE_FINGERPRINT}")));
        assert!(!adapted.contains("--recv-keys"));
        assert!(adapted.contains("pacman --noconfirm -U"));
        assert!(adapted.contains("pacman --noconfirm -Syu"));
        assert!(!adapted.contains("gawk -i inplace -f $gawk_script $pacman_conf_cachyos || true"));
        assert!(
            cachyos::adapt_official_script(
                &original.replace("    pacman -Syu\n", "    pacman -Syyu\n")
            )
            .is_err()
        );
    }
    #[test]
    fn repo_policy_rejects_signature_downgrades_and_unsupported_isa() {
        let v3 = "[options]\nSigLevel = Required DatabaseOptional\n[cachyos-v3]\n[cachyos-core-v3]\n[cachyos-extra-v3]\n[cachyos]\n[core]\n";
        assert!(cachyos::validate_repository_config(v3, IsaLevel::V3).is_ok());
        assert!(cachyos::validate_repository_config(v3, IsaLevel::V2).is_err());
        assert!(cachyos::validate_repository_config(v3, IsaLevel::V4).is_err());
        assert!(
            cachyos::validate_repository_config(
                &v3.replace("Required DatabaseOptional", "Never"),
                IsaLevel::V3
            )
            .is_err()
        );
        assert!(
            cachyos::validate_repository_config(
                &format!("{v3}SigLevel = Optional TrustAll\n"),
                IsaLevel::V3
            )
            .is_err()
        );
    }
    #[test]
    fn firstboot_failure_retains_pending_update_and_success_rebuilds_then_cleans() {
        for failure in 0..3 {
            let root = root();
            let worker = format!("fake-test-ELF {}", rustaller::RUNTIME_VERSION);
            cachyos::install_firstboot_update_with_worker(&root.0, worker.as_bytes()).unwrap();
            let mut outputs = (0..failure).map(|_| ok()).collect::<Vec<_>>();
            outputs.push(CommandOutput::new(Some(1), vec![], vec![]));
            let mut runner = MockRunner::new(outputs);
            assert!(cachyos::run_firstboot_update(&mut runner, &root.0).is_err());
            assert!(
                root.0
                    .join("var/lib/rustaller/cachyos-firstboot.pending")
                    .is_file()
            );
            assert!(
                root.0
                    .join("usr/local/libexec/rustaller-cachyos-worker")
                    .is_file()
            );
            assert_eq!(runner.calls().len(), failure + 1);
            let mut retry = MockRunner::new([ok(), ok(), ok()]);
            cachyos::run_firstboot_update(&mut retry, &root.0).unwrap();
            assert_eq!(
                retry
                    .calls()
                    .iter()
                    .map(|c| c.phase.as_str())
                    .collect::<Vec<_>>(),
                [
                    "cachyos-firstboot-full-upgrade",
                    "cachyos-firstboot-initramfs",
                    "cachyos-firstboot-limine"
                ]
            );
            assert!(
                !root
                    .0
                    .join("var/lib/rustaller/cachyos-firstboot.pending")
                    .exists()
            );
            assert!(
                !root
                    .0
                    .join("usr/local/libexec/rustaller-cachyos-worker")
                    .exists()
            );
        }
    }
}
