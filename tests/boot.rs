mod support;
use rustaller::shared::storage::{DiskLayout, PartitionRole};
use rustaller::worker::live::validate_live_plan_with_uid;
use rustaller::{
    config::RootFilesystem,
    model::{BootMode, InstallPlan, Target},
};
fn entry() -> rustaller_boot_core::Entry {
    rustaller_boot_core::Entry {
        name: "current".into(),
        version: None,
        kernel: "/rustaller-1-1/kernel-0".into(),
        initrds: vec!["/rustaller-1-1/initrd-0".into()],
    }
}
#[test]
fn alpine_live_cmdline_keeps_strict_url_and_path_validation() {
    use rustaller::host::boot::build_nextos_cmdline;
    assert!(
        build_nextos_cmdline(
            "https://dl-cdn.alpinelinux.org/alpine/v3.24/main",
            "/boot/modloop-virt"
        )
        .unwrap()
        .contains("console=ttyS0")
    );
    for (repo, path) in [
        ("https://", "/boot/modloop"),
        ("https://example.org/a?x=1", "/boot/modloop"),
        ("https://example.org/a", "/boot/../modloop"),
    ] {
        assert!(build_nextos_cmdline(repo, path).is_err());
    }
}
#[test]
fn all_24_target_firmware_filesystem_plans_have_bootable_loader_paths() {
    for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
        for boot in [BootMode::Bios, BootMode::Efi] {
            for filesystem in RootFilesystem::ALL {
                let mut plan = InstallPlan::new(target);
                plan.system.root_filesystem = *filesystem;
                let plan = validate_live_plan_with_uid(
                    plan,
                    "x86_64",
                    4096,
                    rustaller::RUNTIME_VERSION,
                    0,
                )
                .unwrap();
                let layout = DiskLayout::new(&plan, "nvme0n1", boot, 8 * 1024_u64.pow(3)).unwrap();
                assert_eq!(layout.partitions().len(), 2);
                assert_eq!(layout.root_partition(), "/dev/nvme0n1p2");
                assert_eq!(layout.boot_partition(), "/dev/nvme0n1p1");
                let config =
                    rustaller_boot_core::create("UUID=1234-abcd", filesystem.as_str(), &[entry()])
                        .unwrap();
                let cfg = rustaller_boot_core::Config::parse(&config).unwrap();
                let cmd = cfg.cmdline().unwrap();
                assert!(cmd.contains("console=ttyS0"));
                assert_eq!(
                    cmd.contains("rootflags=subvol=@"),
                    *filesystem == RootFilesystem::Btrfs
                );
                assert_eq!(
                    layout.table(),
                    if boot == BootMode::Bios {
                        "msdos"
                    } else {
                        "gpt"
                    }
                );
            }
        }
    }
}
#[test]
fn large_bios_disks_reserve_a_dedicated_gpt_stage2_partition_and_reject_firmware_override() {
    let plan = InstallPlan::new(Target::Debian);
    let plan =
        validate_live_plan_with_uid(plan, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0).unwrap();
    let layout = DiskLayout::new(&plan, "vda", BootMode::Bios, 3 * 1024_u64.pow(4)).unwrap();
    assert_eq!(layout.table(), "gpt");
    assert_eq!(layout.partitions().len(), 3);
    assert_eq!(layout.role_number(PartitionRole::BiosBoot), Some(1));
    assert_eq!(layout.boot_partition(), "/dev/vda2");
    assert_eq!(layout.root_partition(), "/dev/vda3");
    let mut plan = InstallPlan::new(Target::Debian);
    plan.force_boot_mode = Some(BootMode::Efi);
    let plan =
        validate_live_plan_with_uid(plan, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0).unwrap();
    assert!(DiskLayout::new(&plan, "vda", BootMode::Bios, 8 * 1024_u64.pow(3)).is_err());
}
#[test]
fn btrfs_swap_and_unsafe_configurations_fail_before_mutation() {
    let mut plan = InstallPlan::new(Target::Arch);
    plan.system.root_filesystem = RootFilesystem::Btrfs;
    plan.system.swap.size_mib = Some(1024);
    assert!(plan.validate().is_err());
    assert!(rustaller_boot_core::create("UUID=x;bad", "ext4", &[entry()]).is_err());
    let mut e = entry();
    e.kernel = "/../vmlinuz".into();
    assert!(rustaller_boot_core::create("UUID=1234-abcd", "xfs", &[e]).is_err());
}

#[test]
fn target_fstab_centralizes_uuid_paths_and_fsck_policy() {
    use rustaller::shared::storage::render_fstab;
    for filesystem in RootFilesystem::ALL {
        let mut plan = InstallPlan::new(Target::Debian);
        plan.system.root_filesystem = *filesystem;
        let plan = validate_live_plan_with_uid(plan, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0)
            .unwrap();
        let layout = DiskLayout::new(&plan, "vda", BootMode::Bios, 8 * 1024_u64.pow(3)).unwrap();
        let fstab = render_fstab(&layout, "abcd-1234", "1234-abcd").unwrap();
        assert!(fstab.contains("/boot vfat fmask=0177,dmask=0077,quiet 0 2"));
        assert_eq!(
            fstab.contains("subvol=@,compress=zstd:3"),
            *filesystem == RootFilesystem::Btrfs
        );
        assert!(
            fstab
                .lines()
                .next()
                .unwrap()
                .ends_with(if *filesystem == RootFilesystem::Ext4 {
                    "0 1"
                } else {
                    "0 0"
                })
        );
        assert!(render_fstab(&layout, "bad/uuid", "abcd").is_err());
    }
}
mod generations {
    use super::*;
    use rustaller::{
        command::{CommandOutput, MockRunner},
        targets::boot::*,
    };
    use std::{fs, os::unix::fs::symlink};
    use support::Scratch;

    const CONFIG: &str = "boot/rustaller/boot.toml";
    fn write_kernel(d: &Scratch, path: &str, label: &str) {
        let mut bytes = vec![0u8; 4096];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..70].copy_from_slice(b"PE\0\0\x64\x86");
        bytes[84..86].copy_from_slice(&240u16.to_le_bytes());
        bytes[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
        bytes[132] = 1;
        bytes[156] = 10;
        bytes[0x1fe..0x200].copy_from_slice(&[0x55, 0xaa]);
        bytes[0x202..0x206].copy_from_slice(b"HdrS");
        bytes[0x206..0x208].copy_from_slice(&0x20cu16.to_le_bytes());
        bytes[0x211] = 1;
        bytes[0x234] = 1;
        bytes[0x236] = 1;
        bytes.extend_from_slice(label.as_bytes());
        let path = d.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn fixture() -> Scratch {
        let d = Scratch::new("boot-generation");
        write_kernel(&d, "boot/vmlinuz-linux", "kernel");
        d.write("boot/initramfs-linux.img", "initramfs");
        write_kernel(&d, "boot/rustaller-1-1/kernel-0", "original");
        d.write("boot/rustaller-1-1/initrd-0", "original-initrd");
        d.write(
            CONFIG,
            &rustaller_boot_core::create("UUID=1234-abcd", "btrfs", &[entry()]).unwrap(),
        );
        d.write(
            "etc/rustaller/boot-state",
            "RUSTALLER-BOOT-1-EFI\n1234-abcd\nbtrfs\nabcd-5678\n",
        );
        write_kernel(&d, "boot/EFI/BOOT/BOOTX64.EFI", "loader");
        d
    }
    fn runner(root: &Scratch, free: u64) -> MockRunner {
        let mut sector = vec![0u8; 512];
        sector[510..].copy_from_slice(&[0x55, 0xaa]);
        sector[36] = 8;
        sector[44] = 2;
        sector[67..71].copy_from_slice(&0xabcd5678u32.to_le_bytes());
        MockRunner::new([
            CommandOutput::new(Some(0),format!("vfat {} abcd-5678 / /dev/vda1\n",root.0.join("boot").display()).into_bytes(),vec![]),
            CommandOutput::new(Some(0),sector,vec![]),
            CommandOutput::new(Some(0),format!("Filesystem 1024-blocks Used Available Capacity Mounted\n/dev/vda1 524288 100 {free} 1% /boot\n").into_bytes(),vec![]),
        ])
    }
    #[test]
    fn refuses_a_bind_subdirectory_and_nonfat32_boot_record() {
        let d = fixture();
        for (fsroot, sector) in [("/subdir", vec![]), ("/", vec![0; 512])] {
            let mount = format!(
                "vfat {} abcd-5678 {fsroot} /dev/vda1\n",
                d.0.join("boot").display()
            );
            let mut runner = MockRunner::new([
                CommandOutput::new(Some(0), mount.into_bytes(), vec![]),
                CommandOutput::new(Some(0), sector, vec![]),
            ]);
            assert!(verify_mount(&mut runner, &d.0, "abcd-5678").is_err());
        }
    }
    #[test]
    fn refresh_keeps_previous_generation_until_complete_publication_and_supports_updates() {
        let d = fixture();
        refresh(&mut runner(&d, 500000), &d.0).unwrap();
        let first = rustaller_boot_core::Config::parse(&d.read(CONFIG)).unwrap();
        d.write("boot/rustaller---/kernel-0", "unmanaged file");
        d.write(
            CONFIG,
            &d.read(CONFIG).replace(
                "extra_cmdline = \"\"",
                "extra_cmdline = \"panic=3\" # retain me",
            ),
        );
        d.write("boot/intel-ucode.img", "early microcode");
        write_kernel(&d, "boot/vmlinuz-lts", "new-kernel");
        d.write("boot/initramfs-lts.img", "new-initramfs");
        refresh(&mut runner(&d, 500000), &d.0).unwrap();
        let text = d.read(CONFIG);
        let next = rustaller_boot_core::Config::parse(&text).unwrap();
        assert!(text.contains("# retain me"));
        assert!(next.cmdline().unwrap().ends_with("panic=3"));
        assert!(next.cmdline().unwrap().contains("rootflags=subvol=@"));
        assert_eq!(
            next.entry("previous").unwrap().kernel,
            first.default_entry().kernel
        );
        assert_eq!(next.entries.len(), 3);
        assert!(next.default_entry().initrds[0].contains("microcode-0"));
        assert!(next.default_entry().initrds[1].contains("initrd-0"));
        assert!(
            d.0.join("boot")
                .join(&first.default_entry().kernel[1..])
                .is_file()
        );
        verify(&d.0).unwrap();
        refresh(&mut runner(&d, 500000), &d.0).unwrap();
        assert_eq!(d.read(CONFIG), text);
        let count = fs::read_dir(d.0.join("boot"))
            .unwrap()
            .filter(|v| {
                v.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("rustaller-")
            })
            .count();
        assert_eq!(
            count, 3,
            "two managed generations plus the untouched unrelated directory"
        );
        assert_eq!(d.read("boot/rustaller---/kernel-0"), "unmanaged file");
    }
    #[test]
    fn native_kernel_families_match_their_own_initramfs_names_and_retain_removals_as_fallback() {
        for (kernel, initramfs) in [
            ("linux", "initramfs-linux.img"),
            ("linux-lts", "initramfs-linux-lts.img"),
            ("linux-hardened", "initramfs-linux-hardened.img"),
            ("linux-zen", "initramfs-linux-zen.img"),
            ("lts", "initramfs-lts"),
            ("virt", "initramfs-virt"),
            ("6.12.1-amd64", "initrd.img-6.12.1-amd64"),
            ("6.12.1-cloud-amd64", "initrd.img-6.12.1-cloud-amd64"),
            ("6.12.1-gentoo-dist", "initramfs-6.12.1-gentoo-dist.img"),
        ] {
            let d = fixture();
            fs::remove_file(d.0.join("boot/vmlinuz-linux")).unwrap();
            fs::remove_file(d.0.join("boot/initramfs-linux.img")).unwrap();
            write_kernel(&d, &format!("boot/vmlinuz-{kernel}"), kernel);
            d.write(&format!("boot/{initramfs}"), "family-initramfs");
            refresh(&mut runner(&d, 500000), &d.0).unwrap();
            let cfg = rustaller_boot_core::Config::parse(&d.read(CONFIG)).unwrap();
            assert_eq!(cfg.default_entry().version.as_deref(), Some(kernel));
            write_kernel(&d, "boot/vmlinuz-additional", "second");
            d.write("boot/initramfs-additional.img", "second initramfs");
            refresh(&mut runner(&d, 500000), &d.0).unwrap();
            fs::remove_file(d.0.join(format!("boot/vmlinuz-{kernel}"))).unwrap();
            fs::remove_file(d.0.join(format!("boot/{initramfs}"))).unwrap();
            refresh(&mut runner(&d, 500000), &d.0).unwrap();
            let cfg = rustaller_boot_core::Config::parse(&d.read(CONFIG)).unwrap();
            assert_eq!(cfg.default_entry().version.as_deref(), Some("additional"));
            assert!(cfg.entries.iter().any(|e|e.name.starts_with("previous")&& e.version.as_deref()==Some(kernel)));
        }
    }
    #[test]
    fn interrupted_publication_recovers_a_missing_active_file_and_preserves_user_edits() {
        let d = fixture();
        refresh(&mut runner(&d, 500000), &d.0).unwrap();
        d.write("boot/rustaller-333-444/kernel-0", "incomplete copy");
        fs::remove_file(d.0.join(CONFIG)).unwrap();
        refresh(&mut runner(&d, 500000), &d.0).unwrap();
        verify(&d.0).unwrap();
        assert!(!d.0.join("boot/rustaller-333-444").exists());
        d.write(CONFIG, "unsupported user edit");
        assert!(refresh(&mut runner(&d, 500000), &d.0).is_err());
        assert_eq!(d.read(CONFIG), "unsupported user edit");
        assert!(
            rustaller_boot_core::Config::parse(&d.read("boot/rustaller/boot.toml.previous"))
                .is_ok()
        );
    }
    #[test]
    fn legacy_efi_state_is_never_reinterpreted_as_a_boot_mount() {
        let d = fixture();
        d.write(
            "etc/rustaller/boot-state",
            &format!("RUSTALLER-LIMINE-{VERSION}\n1234-abcd\next4\nabcd-5678\n"),
        );
        let mut r = MockRunner::empty();
        assert!(refresh(&mut r, &d.0).is_err());
        assert!(r.calls().is_empty());
        assert!(d.0.join(CONFIG).is_file());
    }
    #[test]
    fn newest_numeric_kernel_is_default_and_gentoo_backup_files_are_ignored() {
        let d = fixture();
        fs::remove_file(d.0.join("boot/vmlinuz-linux")).unwrap();
        for version in ["6.9+dist", "6.10+dist"] {
            write_kernel(&d, &format!("boot/vmlinuz-{version}"), version);
            d.write(&format!("boot/initramfs-{version}.img"), "initramfs");
        }
        d.write("boot/vmlinuz-6.10+dist.old", "backup kernel");
        d.write("boot/initramfs-6.10+dist.img.old", "backup initramfs");
        refresh(&mut runner(&d, 500000), &d.0).unwrap();
        let cfg = rustaller_boot_core::Config::parse(&d.read(CONFIG)).unwrap();
        assert_eq!(cfg.default_entry().version.as_deref(), Some("6.10+dist"));
        assert_eq!(
            cfg.entry("current-2").unwrap().version.as_deref(),
            Some("6.9+dist")
        );
    }
    #[test]
    fn failed_refresh_preserves_published_configuration_and_rejects_unsafe_inputs() {
        for failure in [
            "capacity",
            "missing-initrd",
            "symlink",
            "hardlink",
            "config-link",
            "unmounted",
            "wrong-boot-uuid",
            "invalid-state",
            "efi-corrupt",
        ] {
            let d = fixture();
            refresh(&mut runner(&d, 500000), &d.0).unwrap();
            let before = d.read(CONFIG);
            match failure {
                "capacity" => d.write("boot/initramfs-linux.img", "changed"),
                "missing-initrd" => {
                    fs::remove_file(d.0.join("boot/initramfs-linux.img")).unwrap();
                }
                "symlink" => {
                    fs::remove_file(d.0.join("boot/vmlinuz-linux")).unwrap();
                    symlink("/etc/passwd", d.0.join("boot/vmlinuz-linux")).unwrap();
                }
                "hardlink" => {
                    fs::hard_link(d.0.join("boot/vmlinuz-linux"), d.0.join("kernel-copy")).unwrap();
                }
                "config-link" => {
                    fs::rename(d.0.join(CONFIG), d.0.join("original")).unwrap();
                    symlink(d.0.join("original"), d.0.join(CONFIG)).unwrap();
                }
                "invalid-state" => {
                    d.write("etc/rustaller/boot-state", "bad\n");
                }
                "efi-corrupt" => {
                    d.write("boot/EFI/BOOT/BOOTX64.EFI", "not PE");
                }
                _ => {}
            }
            let mut r = if failure == "wrong-boot-uuid" {
                MockRunner::new([CommandOutput::new(
                    Some(0),
                    format!("vfat {} 0000-0000\n", d.0.join("boot").display()).into_bytes(),
                    vec![],
                )])
            } else if failure == "unmounted" {
                MockRunner::new([CommandOutput::new(Some(0), b"ext4 /\n".to_vec(), vec![])])
            } else {
                runner(&d, if failure == "capacity" { 0 } else { 500000 })
            };
            assert!(refresh(&mut r, &d.0).is_err(), "{failure}");
            assert_eq!(d.read(CONFIG), before, "{failure}");
        }
    }
    #[test]
    fn refresh_refuses_partial_copy_and_loader_paths_cannot_traverse() {
        let d = fixture();
        refresh(&mut runner(&d, 500000), &d.0).unwrap();
        let before = d.read(CONFIG);
        // A native filesystem write failure, independent of mocked commands.
        fs::remove_file(d.0.join("boot/initramfs-linux.img")).unwrap();
        fs::create_dir(d.0.join("boot/initramfs-linux.img")).unwrap();
        assert!(refresh(&mut runner(&d, 500000), &d.0).is_err());
        assert_eq!(d.read(CONFIG), before);
        for path in [
            "relative",
            "/a/../b",
            "/a//b",
            "/a/./b",
            "/bad\nentry",
            "/a\\b",
        ] {
            assert!(!rustaller_boot_core::valid_boot_path(path), "{path}");
        }
    }
    #[test]
    fn authenticated_resources_and_hooks_fail_closed() {
        let d = Scratch::new("unbundled-firmware");
        if rustaller::boot_artifacts::UPDATE.is_empty() {
            assert!(prepare(BootMode::Efi, &d.0).is_err());
            assert!(!d.0.join("BOOTX64.EFI").exists());
        }
        for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
            let d = Scratch::new("boot-hooks");
            assert!(install_hooks(&d.0, target, b"wrong worker").is_err());
            if target == Target::Debian {
                d.write("etc/initramfs-tools/initramfs.conf", "MODULES=most\n");
            }
            install_hooks(&d.0, target, rustaller::RUNTIME_VERSION.as_bytes()).unwrap();
            assert!(
                d.0.join("usr/local/libexec/rustaller-boot-update")
                    .is_file()
            );
            let path = match target {
                Target::Alpine => "etc/apk/commit_hooks.d/zz-rustaller-boot",
                Target::Debian | Target::Gentoo => "etc/kernel/postinst.d/zz-rustaller-boot",
                Target::Arch => "etc/pacman.d/hooks/zz-rustaller-boot.hook",
                _ => unreachable!(),
            };
            assert!(fs::symlink_metadata(d.0.join(path)).is_ok());
            if target == Target::Debian {
                assert_eq!(
                    d.read("etc/initramfs-tools/initramfs.conf"),
                    "MODULES=most\n"
                );
                let hook =
                    d.0.join("etc/initramfs-tools/hooks/zz-rustaller-boot-check");
                assert!(!fs::symlink_metadata(&hook).unwrap().is_symlink());
                assert!(
                    std::process::Command::new("sh")
                        .arg("-n")
                        .arg(&hook)
                        .status()
                        .unwrap()
                        .success()
                );
                assert!(
                    std::process::Command::new(&hook)
                        .arg("prereqs")
                        .status()
                        .unwrap()
                        .success()
                );
                assert!(
                    fs::read_to_string(hook)
                        .unwrap()
                        .contains("exec /usr/local/libexec/rustaller-boot-update check")
                );
            }
            if target == Target::Gentoo {
                assert!(
                    fs::symlink_metadata(d.0.join("etc/kernel/preinst.d/zz-rustaller-boot-check"))
                        .is_ok()
                );
                let wrapper = d.read("usr/local/sbin/dracut");
                assert!(wrapper.contains("/usr/bin/dracut \"$@\""));
                assert!(wrapper.find("check").unwrap() < wrapper.find("/usr/bin/dracut").unwrap());
                assert!(
                    wrapper.find("/usr/bin/dracut").unwrap()
                        < wrapper.find("rustaller-boot-update refresh").unwrap()
                );
            }
            let generator_config = match target {
                Target::Arch => Some("etc/mkinitcpio.conf"),
                Target::Debian => Some("usr/share/initramfs-tools/conf-hooks.d/00-rustaller-mount"),
                Target::Alpine => Some("etc/mkinitfs/mkinitfs.conf"),
                Target::Gentoo => Some("etc/dracut.conf.d/00-rustaller-mount.conf"),
                _ => None,
            };
            if let Some(config) = generator_config {
                assert!(d.read(config).contains("check || exit 1"));
                assert!(
                    std::process::Command::new("sh")
                        .args(["-n"])
                        .arg(d.0.join(config))
                        .status()
                        .unwrap()
                        .success()
                );
            }
            if target == Target::Arch {
                assert!(
                    d.read("etc/pacman.d/hooks/00-rustaller-boot-mount.hook")
                        .contains("AbortOnFail")
                );
                assert!(
                    fs::symlink_metadata(d.0.join("etc/initcpio/post/zz-rustaller-boot")).is_ok()
                );
                let hook = d.read(path);
                assert!(hook.contains("Target=usr/lib/modules/*/pkgbase"));
                assert!(hook.contains("Target=usr/lib/initcpio/*"));
            }
        }
        assert!(!hook_action(&["pre-commit".into()]));
        assert!(hook_action(&["post-commit".into()]));
    }
    #[test]
    fn gentoo_dracut_wrapper_defers_staged_output_and_only_refreshes_successful_native_rebuilds() {
        use std::os::unix::fs::PermissionsExt;
        let d = Scratch::new("dracut-wrapper");
        install_hooks(&d.0, Target::Gentoo, rustaller::RUNTIME_VERSION.as_bytes()).unwrap();
        let worker = d.0.join("fake-worker");
        let dracut = d.0.join("fake-dracut");
        d.write(
            "fake-worker",
            "#!/bin/sh\nprintf 'worker %s\\n' \"$*\" >> \"$TEST_LOG\"\n",
        );
        d.write("fake-dracut","#!/bin/sh\nprintf 'dracut %s\\n' \"$*\" >> \"$TEST_LOG\"\nexit \"${TEST_DRACUT_STATUS:-0}\"\n");
        for path in [&worker, &dracut] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let wrapper = d
            .read("usr/local/sbin/dracut")
            .replace(
                "/usr/local/libexec/rustaller-boot-update",
                worker.to_str().unwrap(),
            )
            .replace("/usr/bin/dracut", dracut.to_str().unwrap());
        d.write("wrapper", &wrapper);
        for (args, status, refreshes) in [
            (vec![], 0, true),
            (vec!["--force", "--regenerate-all"], 0, true),
            (vec!["--help"], 0, false),
            (vec!["--list-modules"], 0, false),
            (
                vec!["--force", "/tmp/installkernel-initrd", "6.12.1"],
                0,
                false,
            ),
            (vec!["--force"], 9, false),
        ] {
            let log = d.0.join("log");
            let _ = fs::remove_file(&log);
            let outcome = std::process::Command::new("sh")
                .arg(d.0.join("wrapper"))
                .args(args)
                .env("TEST_LOG", &log)
                .env("TEST_DRACUT_STATUS", status.to_string())
                .status()
                .unwrap();
            assert_eq!(outcome.code(), Some(status));
            let calls = d.read("log");
            assert!(calls.starts_with("worker check\n"));
            assert_eq!(calls.contains("worker refresh\n"), refreshes);
        }
    }
}

#[test]
fn btrfs_options_roundtrip_and_invalid_boot_geometry_fail_closed() {
    use rustaller::host::args::{CliOutcome, parse_args};
    for level in [0, 3, 15] {
        let level = level.to_string();
        let CliOutcome::Plan(plan) = parse_args([
            "debian",
            "--filesystem=btrfs",
            "--boot-size-mib=1024",
            "--btrfs-compression",
            &level,
        ])
        .unwrap() else {
            panic!("plan")
        };
        let bytes = rustaller::codec::encode_plan(&plan).unwrap();
        let decoded = rustaller::codec::decode_plan(&bytes).unwrap();
        assert_eq!(decoded.system, plan.system);
        assert_eq!(
            decoded.schema_version,
            rustaller::model::CURRENT_SCHEMA_VERSION
        );
    }
    for args in [
        vec!["debian", "--boot-size-mib=511"],
        vec!["arch", "--boot-size-mib=4097"],
        vec!["gentoo", "--filesystem=btrfs", "--btrfs-compression=16"],
        vec!["alpine", "--filesystem=xfs", "--btrfs-compression=3"],
        vec!["debian", "--filesystem=btrfs", "--swapfile-mib=64"],
    ] {
        assert!(parse_args(args).is_err());
    }
    assert!(
        rustaller::shared::storage::planned_partitions(BootMode::Bios, 1024 * 1024 * 1024, 512)
            .is_err()
    );
}
#[test]
fn btrfs_format_and_mount_create_at_root_and_unmount_top_on_failure() {
    use rustaller::command::{CommandOutput, MockRunner};
    use rustaller::shared::storage::{btrfs_options, create_partitions, mount_target};
    for level in [0, 3] {
        let mut p = InstallPlan::new(Target::Alpine);
        p.system.root_filesystem = RootFilesystem::Btrfs;
        p.system.btrfs_compression = Some(level);
        let p =
            validate_live_plan_with_uid(p, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0).unwrap();
        let l = DiskLayout::new(&p, "vda", BootMode::Bios, 8 * 1024_u64.pow(3)).unwrap();
        let d = support::Scratch::new("btrfs-mount");
        let mut r = MockRunner::new((0..100).map(|_| CommandOutput::new(Some(0), vec![], vec![])));
        create_partitions(&mut r, &l, &Default::default()).unwrap();
        assert!(
            r.calls()
                .iter()
                .any(|c| c.program == "mkfs.btrfs" && c.args == ["-f", "-K", "/dev/vda2"])
        );
        mount_target(&mut r, &l, &d.0).unwrap();
        assert!(
            r.calls()
                .iter()
                .any(|c| c.program == "btrfs" && c.args[..2] == ["subvolume", "create"])
        );
        assert!(r.calls().iter().any(|c| c.phase == "mount-btrfs-root"
            && c.args.iter().any(|v| v == btrfs_options(level).as_str())));
        assert!(r.calls().iter().any(|c| c.phase == "live-mount-boot"
            && c.args.iter().any(|v| v == d.0.join("boot").as_os_str())));
        let mut r =
            MockRunner::new([0, 1, 0].map(|code| CommandOutput::new(Some(code), vec![], vec![])));
        assert!(mount_target(&mut r, &l, &d.0).is_err());
        assert_eq!(r.calls().last().unwrap().program, "umount");
        assert!(!r.calls().iter().any(|c| c.phase == "live-mount-boot"));
    }
}
