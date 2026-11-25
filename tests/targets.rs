mod support;
mod debian {
    use rustaller::command::{CommandOutput, MockRunner};
    use rustaller::config::RootFilesystem;
    use rustaller::model::{InstallPlan, Target};
    use rustaller::targets::debian::{
        apt_package_list_for_plan, apt_sources_text, bootstrap_rootfs, install_apt_packages,
        write_apt_sources,
    };
    use std::fs;

    use crate::support::Scratch;

    fn success_outputs(count: usize) -> Vec<CommandOutput> {
        (0..count)
            .map(|_| CommandOutput::new(Some(0), Vec::new(), Vec::new()))
            .collect()
    }

    #[test]
    fn apt_sources_match_the_debian_stable_contract() {
        assert_eq!(
            apt_sources_text("stablecodename", None).unwrap(),
            "# Debian stable repositories\ndeb https://deb.debian.org/debian stablecodename main contrib non-free non-free-firmware\ndeb https://deb.debian.org/debian stablecodename-updates main contrib non-free non-free-firmware\ndeb https://security.debian.org/debian-security stablecodename-security main contrib non-free non-free-firmware\n"
        );
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
        let mut plan = InstallPlan::new(Target::Debian);
        let packages = apt_package_list_for_plan(&plan, &firmware).unwrap();
        assert_eq!(
            packages,
            [
                "ca-certificates",
                "systemd-sysv",
                "systemd-resolved",
                "openssh-server",
                "xfsprogs",
                "linux-image-cloud-amd64",
                "dosfstools",
                "initramfs-tools",
                "iproute2",
                "sudo",
                "firmware-linux"
            ]
        );
        plan.username = Some("root".into());
        plan.system.root_filesystem = RootFilesystem::Ext4;
        let root = apt_package_list_for_plan(&plan, &[]).unwrap();
        assert!(root.iter().any(|name| name == "e2fsprogs"));
        assert!(
            !root
                .iter()
                .any(|name| matches!(name.as_str(), "sudo" | "xfsprogs" | "linux-image-amd64"))
        );

        let scratch = Scratch::new("debian");
        let mut runner = MockRunner::new(success_outputs(3));
        install_apt_packages(&mut runner, &scratch.0, &packages).unwrap();
        assert_eq!(runner.calls().len(), 3);
        assert_eq!(runner.calls()[0].args[1..], ["apt-get", "update"]);
        assert_eq!(
            runner.calls()[1].args[1..4],
            ["apt-get", "--simulate", "install"]
        );
        assert_eq!(
            runner.calls()[2].args[1..5],
            ["apt-get", "install", "-y", "--no-install-recommends"]
        );
        assert_eq!(runner.calls()[2].args[5], "ca-certificates");
        assert_eq!(
            runner.calls()[0]
                .env
                .get(std::ffi::OsStr::new("DEBIAN_FRONTEND")),
            Some(&std::ffi::OsString::from("noninteractive"))
        );
    }

    #[test]
    fn fat32_kernel_policy_disables_symlinks_and_keeps_unrelated_settings() {
        let scratch = Scratch::new("debian-fat32");
        fs::create_dir_all(scratch.0.join("etc")).unwrap();
        fs::write(
            scratch.0.join("etc/kernel-img.conf"),
            "do_symlinks = yes\nlink_in_boot=yes\ndo_initrd = yes\n",
        )
        .unwrap();
        rustaller::targets::debian::configure_fat32_boot(&scratch.0).unwrap();
        rustaller::targets::debian::configure_fat32_boot(&scratch.0).unwrap();
        let text = fs::read_to_string(scratch.0.join("etc/kernel-img.conf")).unwrap();
        assert!(text.contains("do_initrd = yes"));
        assert_eq!(text.matches("do_symlinks = no").count(), 1);
        assert_eq!(text.matches("link_in_boot = no").count(), 1);
        assert!(!text.contains("= yes\nlink_in_boot"));
    }

    #[test]
    fn unavailable_selected_kernel_stops_before_installation() {
        let scratch = Scratch::new("debian-unavailable");
        let mut runner = MockRunner::new([
            CommandOutput::new(Some(0), vec![], vec![]),
            CommandOutput::new(Some(100), vec![], b"No candidate".to_vec()),
        ]);
        assert!(
            install_apt_packages(
                &mut runner,
                &scratch.0,
                &apt_package_list_for_plan(&InstallPlan::new(Target::Debian), &[]).unwrap()
            )
            .is_err()
        );
        assert_eq!(runner.calls().len(), 2);
        assert!(
            runner
                .calls()
                .iter()
                .all(|call| call.phase != "debian-apt-install")
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
    use rustaller::command::{CommandInput, CommandOutput, MockRunner, OutputMode};
    use rustaller::config::{InstallProfile, RootFilesystem};
    use rustaller::model::{BootMode, InstallPlan, Target};
    use rustaller::targets::{debian, packages};
    use std::path::Path;

    #[test]
    fn every_package_plan_contains_only_the_fixed_kernel() {
        let required = [
            (Target::Alpine, "linux-virt"),
            (Target::Debian, "linux-image-cloud-amd64"),
            (Target::Arch, "linux-cachyos-server"),
            (Target::Gentoo, "sys-kernel/gentoo-kernel-bin"),
        ];
        for (target, kernel) in required {
            for profile in [InstallProfile::Minimal, InstallProfile::Standard] {
                let mut plan = InstallPlan::new(target);
                plan.system.profile = profile;
                for mode in [BootMode::Bios, BootMode::Efi] {
                    let packages = packages::package_plan(&plan, mode, &[]).unwrap();
                    assert!(
                        packages
                            .bootstrap_packages
                            .iter()
                            .all(|name| name != kernel)
                    );
                    assert_eq!(
                        packages
                            .later_packages
                            .iter()
                            .filter(|name| *name == kernel)
                            .count(),
                        1
                    );
                    for (_, other) in required {
                        if other != kernel {
                            assert!(!packages.later_packages.iter().any(|name| name == other));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn timezone_is_planned_for_the_native_package_manager() {
        let mut alpine = InstallPlan::new(Target::Alpine);
        alpine.post_install.timezone = Some("Asia/Tokyo".into());
        let config = packages::configuration_for_plan(&alpine, Target::Alpine).unwrap();
        assert_eq!(config.kernel, "linux-virt");
        assert_eq!(config.extra_packages, ["tzdata"]);

        let mut debian = InstallPlan::new(Target::Debian);
        debian.post_install.timezone = Some("Etc/UTC".into());
        let packages = debian::apt_package_list_for_plan(&debian, &[]).unwrap();
        assert_eq!(packages.last().unwrap(), "tzdata");

        let mut arch = InstallPlan::new(Target::Arch);
        arch.post_install.timezone = Some("Etc/UTC".into());
        let command = packages::kernel_install_command_for_plan(
            &arch,
            Path::new("/os"),
            "cachyos/linux-cachyos-server",
        )
        .unwrap();
        assert_eq!(command.program, "chroot");
        assert_eq!(
            command.args,
            [
                "/os",
                "pacman",
                "-Syu",
                "--noconfirm",
                "cachyos/linux-cachyos-server",
                "mkinitcpio",
                "tzdata"
            ]
        );
        assert_eq!(command.stdin, CommandInput::Null);
        assert_eq!(command.stdout_mode, OutputMode::Stream);
        assert_eq!(command.stderr_mode, OutputMode::Stream);

        let mut gentoo = InstallPlan::new(Target::Gentoo);
        gentoo.post_install.timezone = Some("Etc/UTC".into());
        let packages = packages::emerge_package_set_for_plan(&gentoo, &[]).unwrap();
        assert!(packages.iter().any(|name| name == "sys-libs/timezone-data"));
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
            let mut plan = InstallPlan::new(target);
            plan.hold = Some(1);
            let kernel = "cachyos/linux-cachyos-server";
            assert!(packages::configuration_for_plan(&plan, Target::Alpine).is_err());
            assert!(debian::apt_package_list_for_plan(&plan, &[]).is_err());
            assert!(
                packages::kernel_install_command_for_plan(&plan, Path::new("/os"), kernel).is_err()
            );
            assert!(packages::emerge_package_set_for_plan(&plan, &[]).is_err());
            plan.hold = None;
            assert_eq!(
                packages::configuration_for_plan(&plan, Target::Alpine).is_ok(),
                target == Target::Alpine
            );
            assert_eq!(
                debian::apt_package_list_for_plan(&plan, &[]).is_ok(),
                target == Target::Debian
            );
            assert_eq!(
                packages::kernel_install_command_for_plan(&plan, Path::new("/os"), kernel).is_ok(),
                target == Target::Arch
            );
            assert_eq!(
                packages::emerge_package_set_for_plan(&plan, &[]).is_ok(),
                target == Target::Gentoo
            );
        }
        let mut plan = InstallPlan::new(Target::Arch);
        plan.schema_version += 1;
        assert!(
            packages::kernel_install_command_for_plan(
                &plan,
                Path::new("/os"),
                "cachyos/linux-cachyos-server"
            )
            .is_err()
        );
        let mut plan = InstallPlan::new(Target::Debian);
        plan.ssh_port = Some(0);
        assert!(debian::apt_package_list_for_plan(&plan, &[]).is_err());
    }

    #[test]
    fn pacstrap_installs_only_userspace_and_the_selected_filesystem_tools() {
        let firmware = vec!["intel-ucode".to_owned()];
        let xfs = packages::pacstrap_packages(RootFilesystem::Xfs, "admin", &firmware);
        for required in [
            "base",
            "dosfstools",
            "openssh",
            "archlinux-keyring",
            "xfsprogs",
            "sudo",
            "intel-ucode",
        ] {
            assert!(xfs.iter().any(|name| name == required));
        }
        assert!(
            !xfs.iter()
                .any(|name| name.starts_with("linux") || name == "e2fsprogs")
        );
        let ext4 = packages::pacstrap_packages(RootFilesystem::Ext4, "root", &[]);
        assert!(ext4.iter().any(|name| name == "e2fsprogs"));
        assert!(
            !ext4
                .iter()
                .any(|name| matches!(name.as_str(), "xfsprogs" | "sudo"))
        );
    }

    #[test]
    fn kernel_availability_resolves_the_fixed_native_family() {
        for (target, package, native, check) in [
            (Target::Alpine, "linux-virt", "apk", "--simulate"),
            (
                Target::Gentoo,
                "sys-kernel/gentoo-kernel-bin",
                "emerge",
                "--pretend",
            ),
        ] {
            let plan = InstallPlan::new(target);
            let mut runner = MockRunner::new([CommandOutput::new(Some(0), vec![], vec![])]);
            packages::verify_kernel_available(&mut runner, Path::new("/os"), &plan).unwrap();
            let args = &runner.calls()[0].args;
            assert_eq!(args[1], native);
            assert_eq!(args.iter().filter(|arg| *arg == package).count(), 1);
            assert!(args.iter().any(|arg| arg == check));
            let mut unavailable = MockRunner::new([CommandOutput::new(Some(1), vec![], vec![])]);
            assert!(
                packages::verify_kernel_available(&mut unavailable, Path::new("/os"), &plan)
                    .is_err()
            );
        }
    }

    #[test]
    fn all_installed_kernels_need_filesystem_and_guest_storage_drivers() {
        use crate::support::Scratch;
        use std::fs;
        let scratch = Scratch::new("kernel-drivers");
        for release in ["6.18-test", "6.19-test"] {
            let path = scratch.0.join("lib/modules").join(release);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("modules.dep"), "").unwrap();
            fs::write(path.join("modules.builtin"), "kernel/fs/ext4/ext4.ko\nkernel/fs/fat/vfat.ko\nkernel/drivers/block/virtio_blk.ko\n").unwrap();
        }
        packages::verify_boot_driver_support(&scratch.0, "ext4", "/dev/vd-rustaller-test", &[])
            .unwrap();
        assert!(
            packages::verify_boot_driver_support(&scratch.0, "xfs", "/dev/vd-rustaller-test", &[])
                .is_err()
        );
        fs::write(
            scratch.0.join("lib/modules/6.19-test/modules.builtin"),
            "kernel/fs/ext4/ext4.ko\nkernel/fs/fat/vfat.ko\n",
        )
        .unwrap();
        assert!(
            packages::verify_boot_driver_support(&scratch.0, "ext4", "/dev/vd-rustaller-test", &[])
                .is_err()
        );
        fs::write(
            scratch.0.join("lib/modules/6.19-test/modules.dep"),
            "kernel/drivers/block/virtio_blk.ko.zst:\n",
        )
        .unwrap();
        assert!(
            packages::verify_boot_driver_support(&scratch.0, "ext4", "/dev/vd-rustaller-test", &[])
                .is_err()
        );
        let module = scratch
            .0
            .join("lib/modules/6.19-test/kernel/drivers/block/virtio_blk.ko.zst");
        fs::create_dir_all(module.parent().unwrap()).unwrap();
        fs::write(&module, "module").unwrap();
        packages::verify_boot_driver_support(&scratch.0, "ext4", "/dev/vd-rustaller-test", &[])
            .unwrap();
        fs::write(
            scratch.0.join("lib/modules/6.19-test/modules.dep"),
            "../escape.ko:\n",
        )
        .unwrap();
        assert!(
            packages::verify_boot_driver_support(&scratch.0, "ext4", "/dev/vd-rustaller-test", &[])
                .is_err()
        );
    }

    #[test]
    fn gentoo_keeps_openrc_network_and_binary_kernel_and_selected_filesystem_tools() {
        let firmware = vec!["sys-kernel/linux-firmware".to_owned()];
        let mut plan = InstallPlan::new(Target::Gentoo);
        let xfs = packages::emerge_package_set_for_plan(&plan, &firmware).unwrap();
        for atom in [
            "sys-kernel/gentoo-kernel-bin",
            "sys-kernel/dracut",
            "sys-kernel/installkernel",
            "sys-fs/dosfstools",
            "sys-fs/xfsprogs",
            "sys-apps/openrc",
            "net-misc/netifrc",
            "net-misc/dhcpcd",
            "net-dns/openresolv",
            "app-admin/sudo",
            "sys-kernel/linux-firmware",
        ] {
            assert_eq!(xfs.iter().filter(|name| *name == atom).count(), 1);
        }
        assert!(
            !xfs.iter()
                .any(|name| name.contains("systemd") || name.ends_with("e2fsprogs"))
        );
        plan.username = Some("root".into());
        plan.system.root_filesystem = RootFilesystem::Ext4;
        let ext4 = packages::emerge_package_set_for_plan(&plan, &[]).unwrap();
        assert!(ext4.iter().any(|name| name == "sys-fs/e2fsprogs"));
        assert!(
            !ext4
                .iter()
                .any(|name| matches!(name.as_str(), "sys-fs/xfsprogs" | "app-admin/sudo"))
        );
    }
}

mod cachyos {
    use crate::support::Scratch;
    use rustaller::command::{CommandOutput, MockRunner};
    use rustaller::host::inspect::cpu::IsaLevel;
    use rustaller::targets::cachyos::{self, BASELINE_REPOSITORY, RELEASE_FINGERPRINT};

    const BASE: &str = "[options]\nArchitecture = x86_64\nSigLevel = Required DatabaseOptional\n[core]\nServer = https://geo.mirror.pkgbuild.com/$repo/os/$arch\n";

    fn root() -> Scratch {
        let dir = Scratch::with_etc("cachyos");
        dir.write("etc/arch-release", "");
        dir.write(
            "etc/pacman.conf",
            &cachyos::repository_config(BASE, IsaLevel::V1).unwrap(),
        );
        dir
    }

    #[test]
    fn repositories_enable_only_the_usable_guest_tier_and_require_signatures() {
        for isa in [IsaLevel::V1, IsaLevel::V2, IsaLevel::V3, IsaLevel::V4] {
            let config = cachyos::repository_config(BASE, isa).unwrap();
            assert!(config.contains(&format!(
                "[cachyos]\nSigLevel = Required\nServer = {BASELINE_REPOSITORY}"
            )));
            assert!(!config.contains("znver"));
            for (level, tier) in [(IsaLevel::V3, "v3"), (IsaLevel::V4, "v4")] {
                for repository in [
                    format!("cachyos-{tier}"),
                    format!("cachyos-core-{tier}"),
                    format!("cachyos-extra-{tier}"),
                ] {
                    let policy = format!("[{repository}]\nSigLevel = Required");
                    assert_eq!(config.contains(&policy), isa == level);
                }
            }
            assert!(cachyos::repository_config(&config, isa).is_err());
        }
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
    fn firstboot_failure_retains_pending_update_and_success_rebuilds_then_cleans() {
        let root = root();
        let worker = format!("fake-test-ELF {}", rustaller::RUNTIME_VERSION);
        cachyos::install_firstboot_update_with_worker(&root.0, worker.as_bytes()).unwrap();
        let ok = || CommandOutput::new(Some(0), vec![], vec![]);
        let mut runner = MockRunner::new([ok(), CommandOutput::new(Some(1), vec![], vec![])]);
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
        assert_eq!(
            runner.calls()[1].args[1..],
            [
                "pacman",
                "-Syu",
                "--noconfirm",
                "--needed",
                "cachyos/linux-cachyos-server"
            ]
        );

        let module = "usr/lib/modules/7.3.5-cachyos-server";
        std::fs::create_dir_all(root.0.join(module)).unwrap();
        std::fs::create_dir_all(root.0.join("boot")).unwrap();
        root.write(&format!("{module}/pkgbase"), "linux-cachyos-server\n");
        root.write(&format!("{module}/vmlinuz"), "kernel");
        root.write("boot/vmlinuz-linux-cachyos-server", "kernel");
        root.write("boot/initramfs-linux-cachyos-server.img", "initramfs");
        let text = |message: String| CommandOutput::new(Some(0), message.into_bytes(), vec![]);
        let hash = "A".repeat(64);
        let mut retry = MockRunner::new([
            ok(),
            ok(),
            ok(),
            text(format!("/{module}/vmlinuz\n")),
            text(format!("{hash}  /a\n{hash}  /b\n")),
            text(format!("{module}/modules.dep\n")),
            ok(),
        ]);
        cachyos::run_firstboot_update(&mut retry, &root.0).unwrap();
        assert_eq!(
            retry
                .calls()
                .iter()
                .map(|call| call.phase.as_str())
                .collect::<Vec<_>>(),
            [
                "cachyos-firstboot-check-boot-mount",
                "cachyos-firstboot-full-upgrade",
                "cachyos-firstboot-initramfs",
                "cachyos-list-installed-server-files",
                "cachyos-compare-native-server-kernel",
                "cachyos-verify-matching-initramfs",
                "cachyos-firstboot-boot-refresh",
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
