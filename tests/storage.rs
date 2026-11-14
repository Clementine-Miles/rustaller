mod support;
mod live_storage {
    use rustaller::command::{CommandOutput, MockRunner};
    use rustaller::model::{BootMode, InstallPlan, Target};
    use rustaller::shared::storage::{DiskLayout, create_partitions};
    use rustaller::worker::live::validate_live_plan_with_uid;

    fn layout(target: Target, mode: BootMode, bytes: u64, disk: &str) -> DiskLayout {
        let plan = InstallPlan::new(target);
        let validated =
            validate_live_plan_with_uid(plan, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0)
                .unwrap();
        DiskLayout::new(&validated, disk, mode, bytes).unwrap()
    }

    #[test]
    fn layout_preserves_bios_gpt_boundary_and_nvme_partition_names() {
        let limit = 2 * 1024_u64.pow(4);
        let bios = layout(Target::Debian, BootMode::Bios, limit, "vda");
        assert_eq!(bios.root_partition(), "/dev/vda1");
        assert_eq!(bios.efi_partition(), None);
        let large = layout(Target::Gentoo, BootMode::Bios, limit + 1, "nvme0n1");
        assert_eq!(large.root_partition(), "/dev/nvme0n1p2");
        let efi = layout(Target::Alpine, BootMode::Efi, 40_000_000_000, "nvme0n1");
        assert_eq!(efi.root_partition(), "/dev/nvme0n1p2");
        assert_eq!(efi.efi_partition().as_deref(), Some("/dev/nvme0n1p1"));
    }

    #[test]
    fn readonly_unknown_and_bios_4kn_volumes_fail_before_any_mutation() {
        use rustaller::shared::storage::validate_disk_geometry;
        for (mode, output, accepted) in [
            (BootMode::Bios, "0 512\n", true),
            (BootMode::Efi, "0 512\n", true),
            (BootMode::Efi, "0 4096\n", true),
            (BootMode::Bios, "0 4096\n", false),
            (BootMode::Efi, "1 512\n", false),
            (BootMode::Efi, "0 2048\n", false),
            (BootMode::Efi, "0 512\n0 512\n", false),
            (BootMode::Efi, "", false),
        ] {
            let mut runner = MockRunner::new([CommandOutput::new(
                Some(0),
                output.as_bytes().to_vec(),
                vec![],
            )]);
            assert_eq!(
                validate_disk_geometry(&mut runner, "/dev/nvme0n1", mode).is_ok(),
                accepted,
                "{mode:?} {output:?}"
            );
            assert_eq!(runner.calls().len(), 1);
            assert_eq!(runner.calls()[0].program, "lsblk");
        }
    }

    #[test]
    fn xfs_space_floor_accounts_for_esp_before_partitioning() {
        let plan = validate_live_plan_with_uid(
            InstallPlan::new(Target::Alpine),
            "x86_64",
            4096,
            rustaller::RUNTIME_VERSION,
            0,
        )
        .unwrap();
        for (mode, minimum_mib) in [(BootMode::Bios, 513), (BootMode::Efi, 1025)] {
            assert!(DiskLayout::new(&plan, "vda", mode, (minimum_mib - 1) * 1024 * 1024).is_err());
            assert!(DiskLayout::new(&plan, "vda", mode, minimum_mib * 1024 * 1024).is_ok());
        }
    }

    #[test]
    fn efi_uses_512_mib_fat32_esp_with_4kn_compatible_cluster_count() {
        let mut runner = MockRunner::new(successes(100));
        create_partitions(
            &mut runner,
            &layout(
                Target::Alpine,
                BootMode::Efi,
                8 * 1024_u64.pow(3),
                "nvme0n1",
            ),
            &Default::default(),
        )
        .unwrap();
        let partition = runner
            .calls()
            .iter()
            .find(|c| c.program == "parted")
            .unwrap();
        assert!(
            partition
                .args
                .windows(2)
                .any(|pair| pair == ["1MiB", "513MiB"])
        );
        assert!(
            partition
                .args
                .windows(3)
                .any(|triple| triple == ["1", "esp", "on"])
        );
        let format = runner
            .calls()
            .iter()
            .find(|c| c.program == "mkfs.fat")
            .unwrap();
        assert_eq!(format.args, ["-F", "32", "-s", "1", "/dev/nvme0n1p1"]);
    }

    #[test]
    fn partition_failure_stops_before_formatting_or_mounting() {
        let layout = layout(Target::Debian, BootMode::Bios, 40_000_000_000, "vda");
        let mut runner = MockRunner::new([
            CommandOutput::new(Some(0), vec![], vec![]),
            CommandOutput::new(Some(0), vec![], vec![]),
            CommandOutput::new(Some(9), vec![], vec![]),
        ]);
        let error = create_partitions(&mut runner, &layout, &Default::default()).unwrap_err();
        assert!(error.to_string().contains("9"));
        assert_eq!(runner.calls().len(), 3);
        assert_eq!(runner.calls()[2].program, "wipefs");
        assert!(
            !runner
                .calls()
                .iter()
                .any(|c| c.program == "mkfs.xfs" || c.program == "mount")
        );
    }

    #[test]
    fn layout_rejects_unvalidated_disk_names_and_conflicting_explicit_disk() {
        let mut plan = InstallPlan::new(Target::Arch);
        plan.target_disk = Some("/dev/vda".into());
        let validated =
            validate_live_plan_with_uid(plan, "x86_64", 1024, rustaller::RUNTIME_VERSION, 0)
                .unwrap();
        for disk in [
            "../sda",
            "/dev/sda",
            "",
            ".",
            "..",
            "-sda",
            "sda\n",
            "nvme0n1p1",
            "sdb",
        ] {
            assert!(
                DiskLayout::new(&validated, disk, BootMode::Bios, 10_000).is_err(),
                "{disk:?}"
            );
        }
    }

    #[test]
    fn rescue_plan_cannot_construct_a_destructive_layout() {
        let mut plan = InstallPlan::new(Target::Alpine);
        plan.hold = Some(1);
        let plan = validate_live_plan_with_uid(plan, "x86_64", 512, rustaller::RUNTIME_VERSION, 0)
            .unwrap();
        assert!(DiskLayout::new(&plan, "vda", BootMode::Bios, 10_000).is_err());
    }

    fn successes(count: usize) -> Vec<CommandOutput> {
        (0..count)
            .map(|_| CommandOutput::new(Some(0), vec![], vec![]))
            .collect()
    }

    #[test]
    fn ext4_selection_controls_partition_format_and_mount_without_swap_activation() {
        use rustaller::config::RootFilesystem;
        use rustaller::shared::storage::mount_target;
        for boot in [BootMode::Bios, BootMode::Efi] {
            let mut install = InstallPlan::new(Target::Debian);
            install.system.root_filesystem = RootFilesystem::Ext4;
            let selected =
                validate_live_plan_with_uid(install, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0)
                    .unwrap();
            let layout = DiskLayout::new(&selected, "nvme0n1", boot, 8 * 1024_u64.pow(3)).unwrap();
            let mut runner = MockRunner::new(successes(100));
            create_partitions(&mut runner, &layout, &Default::default()).unwrap();
            let root = std::env::temp_dir().join(format!(
                "rustaller-ext4-mount-{}-{}",
                std::process::id(),
                boot.as_str()
            ));
            std::fs::create_dir(&root).unwrap();
            mount_target(&mut runner, &layout, &root).unwrap();
            std::fs::remove_dir_all(root).unwrap();
            let calls = runner.calls();
            let partition = calls.iter().find(|c| c.program == "parted").unwrap();
            assert!(partition.args.iter().any(|v| v == "ext4"));
            let format = calls.iter().find(|c| c.program == "mkfs.ext4").unwrap();
            assert_eq!(
                format.args,
                [
                    "-F",
                    "-E",
                    "nodiscard,lazy_itable_init=0,lazy_journal_init=0",
                    &layout.root_partition()
                ]
            );
            let mount = calls.iter().find(|c| c.phase == "live-mount-root").unwrap();
            assert_eq!(mount.args[1], "ext4");
            assert!(!calls.iter().any(|c| {
                ["mkfs.xfs", "swapon", "swapoff", "mkswap"]
                    .iter()
                    .any(|p| c.program == *p)
            }));
        }
    }

    #[test]
    fn partition_refresh_ignores_partprobe_but_stops_on_partx_failure() {
        use rustaller::shared::storage::{PartitionEnvironment, refresh_partition_table};
        let layout = layout(Target::Debian, BootMode::Bios, 40_000_000_000, "vda");
        let mut responses = successes(10);
        responses[3].status = Some(8); // partprobe is best effort
        responses[5].status = Some(9); // partx must succeed
        let mut runner = MockRunner::new(responses);
        assert!(
            refresh_partition_table(
                &mut runner,
                &layout,
                &PartitionEnvironment {
                    has_partprobe: true,
                    has_partx: true,
                    device_links: None,
                }
            )
            .is_err()
        );
        assert_eq!(runner.calls().len(), 6);
        assert_eq!(runner.calls()[5].program, "partx");
        assert!(
            !runner
                .calls()
                .iter()
                .any(|c| c.program == "mdev" || c.program == "mkfs.xfs")
        );
    }

    #[test]
    fn mdev_start_retries_immediately_once_then_uses_the_shell_delay_order() {
        use rustaller::shared::storage::refresh_partition_table;
        let layout = layout(Target::Debian, BootMode::Bios, 40_000_000_000, "vda");
        let mut responses = successes(12);
        responses[7].status = Some(7);
        responses[8].status = Some(8);
        let mut runner = MockRunner::new(responses);
        refresh_partition_table(&mut runner, &layout, &Default::default()).unwrap();
        assert_eq!(runner.calls()[7].args, ["-q", "mdev", "start"]);
        assert_eq!(runner.calls()[8].args, ["-q", "mdev", "start"]);
        assert_eq!(runner.calls()[9].program, "sleep");
        assert_eq!(runner.calls()[9].args, ["5"]);
        assert_eq!(runner.calls()[10].args, ["-q", "mdev", "start"]);
        assert_eq!(runner.calls()[11].args, ["1"]);
    }

    #[test]
    fn mdev_start_exhaustion_preserves_the_last_exit_status() {
        use rustaller::shared::storage::refresh_partition_table;
        let layout = layout(Target::Debian, BootMode::Bios, 40_000_000_000, "vda");
        let mut responses = successes(7);
        responses.push(CommandOutput::new(Some(23), vec![], vec![]));
        for _ in 0..10 {
            responses.push(CommandOutput::new(Some(23), vec![], vec![]));
            responses.push(CommandOutput::new(Some(0), vec![], vec![]));
        }
        let mut runner = MockRunner::new(responses);
        let error = refresh_partition_table(&mut runner, &layout, &Default::default()).unwrap_err();
        assert!(error.to_string().contains("23"), "{error}");
        assert_eq!(runner.calls().len(), 28);
    }

    #[test]
    fn package_spawn_failures_follow_the_same_five_attempt_retry_contract() {
        use rustaller::command::{CommandRunner, CommandSpec, PipelineOutput};
        struct MissingApk {
            calls: Vec<CommandSpec>,
        }
        impl CommandRunner for MissingApk {
            fn run(&mut self, spec: &CommandSpec) -> std::io::Result<CommandOutput> {
                self.calls.push(spec.clone());
                if spec.program == "apk" {
                    Err(std::io::Error::from(std::io::ErrorKind::NotFound))
                } else {
                    Ok(CommandOutput::new(Some(0), vec![], vec![]))
                }
            }
            fn run_pipeline(&mut self, _: &[CommandSpec]) -> std::io::Result<PipelineOutput> {
                panic!("no pipeline")
            }
        }
        let mut runner = MissingApk { calls: vec![] };
        assert!(
            create_partitions(
                &mut runner,
                &layout(Target::Arch, BootMode::Bios, 1_000_000_000, "vda"),
                &Default::default()
            )
            .is_err()
        );
        assert_eq!(runner.calls.len(), 9);
        assert_eq!(
            runner.calls.iter().filter(|c| c.program == "apk").count(),
            5
        );
        assert_eq!(
            runner.calls.iter().filter(|c| c.program == "sleep").count(),
            4
        );
    }
}

mod storage_cleanup {
    use rustaller::command::{CommandOutput, MockRunner};
    use rustaller::model::{BootMode, InstallPlan, Target};
    use rustaller::shared::storage::mount_target;
    use rustaller::shared::storage::{DiskLayout, PartitionEnvironment, refresh_partition_table};
    use rustaller::worker::live::validate_live_plan_with_uid;
    use std::fs;

    use crate::support::Scratch;
    fn plan() -> rustaller::worker::live::ValidatedLivePlan {
        let plan = InstallPlan::new(Target::Debian);
        validate_live_plan_with_uid(plan, "x86_64", 512, rustaller::RUNTIME_VERSION, 0).unwrap()
    }
    fn success(count: usize) -> Vec<CommandOutput> {
        (0..count)
            .map(|_| CommandOutput::new(Some(0), vec![], vec![]))
            .collect()
    }

    #[test]
    fn device_link_cleanup_does_not_follow_links_or_remove_dotfiles() {
        let scratch = Scratch::new("storage_cleanup");
        let links = scratch.0.join("dev/disk");
        fs::create_dir_all(links.join("by-id")).unwrap();
        fs::create_dir(scratch.0.join("outside")).unwrap();
        fs::write(scratch.0.join("outside/keep"), b"keep").unwrap();
        fs::write(links.join(".keep"), b"keep").unwrap();
        std::os::unix::fs::symlink(scratch.0.join("outside"), links.join("by-id/disk")).unwrap();
        std::os::unix::fs::symlink("../../vda1", links.join("by-id/target")).unwrap();
        std::os::unix::fs::symlink("../../vdb1", links.join("by-id/other-disk")).unwrap();
        std::os::unix::fs::symlink("../../vdaa1", links.join("by-id/similar-name")).unwrap();
        let layout = DiskLayout::new(&plan(), "vda", BootMode::Bios, 8 * 1024_u64.pow(3)).unwrap();
        let mut runner = MockRunner::new(success(100));
        refresh_partition_table(
            &mut runner,
            &layout,
            &PartitionEnvironment {
                has_partprobe: false,
                has_partx: false,
                device_links: Some(links.clone()),
            },
        )
        .unwrap();
        assert!(scratch.0.join("outside/keep").is_file());
        assert!(links.join(".keep").is_file());
        assert!(links.join("by-id").is_dir());
        assert!(fs::symlink_metadata(links.join("by-id/target")).is_err());
        for name in ["disk", "other-disk", "similar-name"] {
            assert!(
                fs::symlink_metadata(links.join("by-id").join(name))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
    }

    #[test]
    fn mount_target_refuses_symlinked_roots_before_running_mount() {
        let scratch = Scratch::new("storage_cleanup");
        fs::create_dir(scratch.0.join("outside")).unwrap();
        std::os::unix::fs::symlink(scratch.0.join("outside"), scratch.0.join("os")).unwrap();
        let layout = DiskLayout::new(&plan(), "vda", BootMode::Bios, 8 * 1024_u64.pow(3)).unwrap();
        let mut runner = MockRunner::empty();
        assert!(mount_target(&mut runner, &layout, &scratch.0.join("os/child")).is_err());
        assert!(runner.calls().is_empty());
        assert!(!scratch.0.join("outside/child").exists());
    }
}

mod swap {

    use rustaller::command::{
        CommandOutput, CommandRunner, CommandSpec, MockRunner, PipelineOutput, SwapSafeRunner,
    };

    use rustaller::model::{InstallPlan, Target};
    use rustaller::shared::swapfile::prepare_swapfile;
    use rustaller::worker::live::{ValidatedLivePlan, validate_live_plan_with_uid};
    use std::fs;
    use std::io;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    // Execute only the bounded temporary-file allocator; inventory and mkswap stay
    // mocked. This checks real dd allocation without touching swap or block devices.
    struct SwapIoRunner(MockRunner);
    impl CommandRunner for SwapIoRunner {
        fn run(&mut self, spec: &CommandSpec) -> io::Result<CommandOutput> {
            if spec.program == "dd" {
                let output = std::process::Command::new("dd").args(&spec.args).output()?;
                assert!(output.status.success());
            }
            self.0.run(spec)
        }
        fn run_pipeline(&mut self, specs: &[CommandSpec]) -> io::Result<PipelineOutput> {
            self.0.run_pipeline(specs)
        }
    }
    use crate::support::Scratch;
    fn live(plan: InstallPlan) -> ValidatedLivePlan {
        validate_live_plan_with_uid(plan, "x86_64", 2048, rustaller::RUNTIME_VERSION, 0).unwrap()
    }
    fn out(status: i32, text: &str) -> CommandOutput {
        CommandOutput::new(Some(status), text.as_bytes().to_vec(), vec![])
    }
    #[test]
    fn opt_in_swapfile_is_fully_allocated_private_and_persistent_but_never_activated_on_host() {
        let scratch = Scratch::with_etc("advanced_config");
        scratch.write("etc/fstab", "UUID=original / xfs defaults 0 1\n");
        let mut selected = InstallPlan::new(Target::Debian);
        selected.system.swap.size_mib = Some(64);
        selected.system.swap.swappiness = Some(10);
        let mut runner = SwapSafeRunner::new(SwapIoRunner(MockRunner::new([
            out(0, "xfs\n"),
            out(
                0,
                "Filesystem 1048576-blocks Used Available Capacity Mounted on\n/dev/mock 4096 512 3584 13% /os\n",
            ),
            out(0, ""),
            out(0, ""),
        ])));
        let mut progress = 0;
        assert!(
            prepare_swapfile(&mut runner, &scratch.0, &live(selected), |n, total| {
                assert_eq!(total, 64);
                progress = n;
            })
            .unwrap()
        );
        assert_eq!(progress, 64);
        let metadata = fs::metadata(scratch.0.join("swapfile")).unwrap();
        assert_eq!(metadata.len(), 64 * 1024 * 1024);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            scratch
                .read("etc/fstab")
                .matches("/swapfile none swap defaults 0 0")
                .count(),
            1
        );
        assert!(scratch.read("etc/fstab").contains("UUID=original"));
        assert!(
            scratch
                .read("etc/sysctl.d/90-rustaller-swap.conf")
                .contains("vm.swappiness = 10")
        );
        let calls = runner.into_inner();
        assert_eq!(calls.0.calls().len(), 4);
        assert_eq!(calls.0.calls()[3].program, "mkswap");
        assert!(
            !calls
                .0
                .calls()
                .iter()
                .any(|c| c.program == "swapon" || c.program == "swapoff")
        );
    }

    #[test]
    fn swapfile_default_collisions_unsafe_filesystems_and_low_space_never_allocate_or_format() {
        let scratch = Scratch::with_etc("advanced_config");
        scratch.service("swap");
        let mut empty = MockRunner::empty();
        assert!(
            !prepare_swapfile(
                &mut empty,
                &scratch.0,
                &live(InstallPlan::new(Target::Alpine)),
                |_, _| {}
            )
            .unwrap()
        );
        assert!(empty.calls().is_empty());
        let mut selected = InstallPlan::new(Target::Alpine);
        selected.system.swap.size_mib = Some(64);
        let selected = live(selected);
        scratch.write("swapfile", "original");
        assert!(prepare_swapfile(&mut empty, &scratch.0, &selected, |_, _| {}).is_err());
        assert_eq!(scratch.read("swapfile"), "original");
        fs::remove_file(scratch.0.join("swapfile")).unwrap();
        scratch.write("etc/fstab", "/swapfile none swap defaults 0 0\n");
        assert!(prepare_swapfile(&mut empty, &scratch.0, &selected, |_, _| {}).is_err());
        assert!(empty.calls().is_empty());
        scratch.write("etc/fstab", "");
        let mut runner = MockRunner::new([out(0, "btrfs\n")]);
        assert!(prepare_swapfile(&mut runner, &scratch.0, &selected, |_, _| {}).is_err());
        assert_eq!(runner.calls().len(), 1);
        assert!(!scratch.0.join("swapfile").exists());
        let mut runner = MockRunner::new([
            out(0, "xfs\n"),
            out(0, "header\n/dev/mock 1024 1000 24 98% /os\n"),
        ]);
        assert!(prepare_swapfile(&mut runner, &scratch.0, &selected, |_, _| {}).is_err());
        assert!(!scratch.0.join("swapfile").exists());
        symlink(scratch.0.join("not-created"), scratch.0.join("swapfile")).unwrap();
        assert!(prepare_swapfile(&mut empty, &scratch.0, &selected, |_, _| {}).is_err());
        assert!(!scratch.0.join("not-created").exists());
    }

    #[test]
    fn mkswap_failure_leaves_fstab_untouched_and_removes_only_its_temporary_file() {
        let scratch = Scratch::with_etc("advanced_config");
        scratch.write("etc/fstab", "original\n");
        scratch.write("keep", "unrelated");
        let mut selected = InstallPlan::new(Target::Debian);
        selected.system.swap.size_mib = Some(64);
        let mut runner = SwapSafeRunner::new(SwapIoRunner(MockRunner::new([
            out(0, "xfs\n"),
            out(0, "header\n/dev/mock 4096 512 3584 13% /os\n"),
            out(0, ""),
            out(1, "failure"),
        ])));
        assert!(prepare_swapfile(&mut runner, &scratch.0, &live(selected), |_, _| {}).is_err());
        assert_eq!(scratch.read("etc/fstab"), "original\n");
        assert_eq!(scratch.read("keep"), "unrelated");
        assert!(!scratch.0.join("swapfile").exists());
        assert!(!fs::read_dir(&scratch.0).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".rustaller-swap")
        }));
    }

    #[test]
    fn dd_failure_does_not_format_or_publish_swap() {
        let scratch = Scratch::with_etc("advanced_config");
        scratch.write("etc/fstab", "original\n");
        let mut selected = InstallPlan::new(Target::Debian);
        selected.system.swap.size_mib = Some(64);
        let mut runner = MockRunner::new([
            out(0, "xfs\n"),
            out(0, "header\n/dev/mock 4096 512 3584 13% /os\n"),
            out(1, "No space left on device"),
        ]);
        assert!(prepare_swapfile(&mut runner, &scratch.0, &live(selected), |_, _| {}).is_err());
        assert_eq!(runner.calls().last().unwrap().program, "dd");
        assert_eq!(scratch.read("etc/fstab"), "original\n");
        assert!(!scratch.0.join("swapfile").exists());
        assert!(!fs::read_dir(&scratch.0).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".rustaller-swap")
        }));
    }

    #[test]
    fn alpine_swap_enables_native_openrc_boot_service() {
        let scratch = Scratch::with_etc("advanced_config");
        scratch.service("swap");
        scratch.write("etc/fstab", "UUID=root / xfs defaults 0 0\n");
        let mut selected = InstallPlan::new(Target::Alpine);
        selected.system.swap.size_mib = Some(64);
        let mut runner = SwapSafeRunner::new(SwapIoRunner(MockRunner::new([
            out(0, "xfs\n"),
            out(0, "header\n/dev/mock 4096 512 3584 13% /os\n"),
            out(0, ""),
            out(0, ""),
        ])));
        assert!(prepare_swapfile(&mut runner, &scratch.0, &live(selected), |_, _| {}).unwrap());
        assert_eq!(
            fs::read_link(scratch.0.join("etc/runlevels/boot/swap")).unwrap(),
            PathBuf::from("/etc/init.d/swap")
        );
    }

    #[test]
    fn ext4_swapfile_requires_matching_mount_and_stays_inactive_until_target_boot() {
        let scratch = Scratch::with_etc("advanced_config");
        scratch.write("etc/fstab", "UUID=root / ext4 defaults 0 1\n");
        let mut selected = InstallPlan::new(Target::Debian);
        selected.system.root_filesystem = rustaller::config::RootFilesystem::Ext4;
        selected.system.swap.size_mib = Some(64);
        let selected = live(selected);
        let mut mismatch = MockRunner::new([out(0, "xfs\n")]);
        assert!(prepare_swapfile(&mut mismatch, &scratch.0, &selected, |_, _| {}).is_err());
        assert!(!scratch.0.join("swapfile").exists());
        let mut runner = SwapSafeRunner::new(SwapIoRunner(MockRunner::new([
            out(0, "ext4\n"),
            out(0, "header\n/dev/mock 4096 512 3584 13% /os\n"),
            out(0, ""),
            out(0, ""),
        ])));
        assert!(prepare_swapfile(&mut runner, &scratch.0, &selected, |_, _| {}).unwrap());
        assert_eq!(
            fs::metadata(scratch.0.join("swapfile")).unwrap().len(),
            64 * 1024 * 1024
        );
        assert_eq!(
            scratch
                .read("etc/fstab")
                .matches("/swapfile none swap defaults 0 0")
                .count(),
            1
        );
        assert!(
            !runner
                .into_inner()
                .0
                .calls()
                .iter()
                .any(|c| c.program == "swapon" || c.program == "swapoff")
        );
    }
}
