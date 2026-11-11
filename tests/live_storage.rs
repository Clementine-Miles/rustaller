use reinstall_rs::command::{CommandOutput, MockRunner};
use reinstall_rs::model::{BootMode, InstallPlan, Target};
use reinstall_rs::shared::storage::{DiskLayout, create_partitions};
use reinstall_rs::worker::live::validate_live_plan_with_uid;

fn layout(target: Target, mode: BootMode, bytes: u64, disk: &str) -> DiskLayout {
    let mut plan = InstallPlan::new(target);
    plan.confhome = Some("https://assets.example/reinstall".into());
    let validated =
        validate_live_plan_with_uid(plan, "x86_64", 4096, reinstall_rs::RUNTIME_VERSION, 0)
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
    use reinstall_rs::shared::storage::validate_disk_geometry;
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
        reinstall_rs::RUNTIME_VERSION,
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
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.target_disk = Some("/dev/vda".into());
    let validated =
        validate_live_plan_with_uid(plan, "x86_64", 1024, reinstall_rs::RUNTIME_VERSION, 0)
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
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.hold = Some(1);
    let plan =
        validate_live_plan_with_uid(plan, "x86_64", 512, reinstall_rs::RUNTIME_VERSION, 0).unwrap();
    assert!(DiskLayout::new(&plan, "vda", BootMode::Bios, 10_000).is_err());
}

fn successes(count: usize) -> Vec<CommandOutput> {
    (0..count)
        .map(|_| CommandOutput::new(Some(0), vec![], vec![]))
        .collect()
}

#[test]
fn partition_refresh_ignores_partprobe_but_stops_on_partx_failure() {
    use reinstall_rs::shared::storage::{PartitionEnvironment, refresh_partition_table};
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
    use reinstall_rs::shared::storage::refresh_partition_table;
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
    use reinstall_rs::shared::storage::refresh_partition_table;
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
    use reinstall_rs::command::{CommandRunner, CommandSpec, PipelineOutput};
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
