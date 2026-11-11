use reinstall_rs::command::{CommandOutput, MockRunner};
use reinstall_rs::host::disk::{
    DiskError, discover_root_disks, parse_partition_table_id, parse_root_device,
    parse_root_disk_names, read_partition_table_id, select_root_disk, validate_target_disk_path,
};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn parses_root_device_and_all_underlying_disk_names_from_utilities() {
    assert_eq!(
        parse_root_device("proc on /proc type proc (rw)\n/dev/mapper/vg-root on / type xfs (rw)\n"),
        Some("/dev/mapper/vg-root")
    );
    assert_eq!(
        parse_root_disk_names(concat!(
            "vg-root 253:0 0 80G 0 lvm /\n",
            "nvme0n1 259:0 0 80G 0 disk\n",
            "sda 8:0 0 1T 0 disk\n",
            "loop0 7:0 0 2G 0 loop\n",
            "nvme0n1 259:0 0 80G 0 disk\n"
        ))
        .unwrap(),
        ["nvme0n1", "sda"]
    );
}

#[test]
fn root_disk_selection_requires_explicit_choice_when_root_spans_disks() {
    let names = vec!["nvme0n1".into(), "sda".into()];
    assert!(matches!(
        select_root_disk(&names, None),
        Err(DiskError::AmbiguousRootDisks(_))
    ));
    assert_eq!(select_root_disk(&names, Some("sda")).unwrap(), "sda");
    assert_eq!(select_root_disk(&["vda".into()], None).unwrap(), "vda");
    assert!(select_root_disk(&names, Some("nvme0n1p1")).is_err());
}

#[test]
fn parses_legacy_mbr_and_gpt_partition_table_identifiers() {
    assert_eq!(
        parse_partition_table_id("Disk identifier: 0x36778223\n").unwrap(),
        "36778223"
    );
    assert_eq!(
        parse_partition_table_id("Disk identifier (GUID): d6b17c1a-40a1-4bd2-8f00-123456789abc\n")
            .unwrap(),
        "d6b17c1a-40a1-4bd2-8f00-123456789abc"
    );
    assert!(parse_partition_table_id("Disk identifier: invalid\n").is_err());
}

#[test]
fn target_disk_paths_must_resolve_to_block_devices_under_dev() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let regular_file = std::env::temp_dir().join(format!(
        "reinstall-disk-test-{}-{nonce}",
        std::process::id()
    ));
    fs::write(&regular_file, b"not a disk").unwrap();
    assert!(validate_target_disk_path(&regular_file).is_err());
    assert!(validate_target_disk_path(Path::new("/etc/passwd")).is_err());
    assert!(validate_target_disk_path(Path::new("/dev/../etc/passwd")).is_err());
    fs::remove_file(regular_file).unwrap();
}

#[test]
fn reads_the_partition_table_id_with_the_existing_fdisk_invocation() {
    let mut runner = MockRunner::new([CommandOutput::new(
        Some(0),
        b"Disk identifier: 0x36778223\n".to_vec(),
        Vec::new(),
    )]);
    assert_eq!(
        read_partition_table_id(&mut runner, "/dev/nvme0n1").unwrap(),
        "36778223"
    );
    assert_eq!(runner.calls()[0].program, "fdisk");
    assert_eq!(runner.calls()[0].args, ["-l", "/dev/nvme0n1"]);
}

#[test]
fn root_disk_discovery_uses_existing_mount_and_lsblk_utility_order() {
    let mut runner = MockRunner::new([
        CommandOutput::new(
            Some(0),
            b"/dev/mapper/vg-root on / type xfs (rw)\n".to_vec(),
            Vec::new(),
        ),
        CommandOutput::new(
            Some(0),
            b"vg-root 253:0 0 80G 0 lvm /\nnvme0n1 259:0 0 80G 0 disk\n".to_vec(),
            Vec::new(),
        ),
    ]);
    assert_eq!(discover_root_disks(&mut runner).unwrap(), ["nvme0n1"]);
    assert_eq!(runner.calls().len(), 2);
    assert_eq!(runner.calls()[0].program, "mount");
    assert_eq!(runner.calls()[1].program, "lsblk");
    assert_eq!(
        runner.calls()[1].args,
        ["-rn", "--inverse", "/dev/mapper/vg-root"]
    );
}
