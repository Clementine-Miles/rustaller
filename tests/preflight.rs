use rustaller::command::{CommandOutput, MockRunner};
use rustaller::host::preflight::{
    HostFacts, PreflightError, detect_ram_mb, effective_uid, parse_dmidecode_memory,
    parse_lshw_memory, parse_lsmem_memory, parse_memtotal_fallback, root_mount_is_tmpfs,
    validate_host,
};
use rustaller::model::Target;

#[test]
fn host_preflight_checks_architecture_root_container_and_ram() {
    let base = HostFacts {
        os: "linux".into(),
        architecture: "x86_64".into(),
        effective_uid: 0,
        root_fs_type: "ext4".into(),
        in_container: false,
        memory_mb: Some(2048),
    };
    assert!(validate_host(&base, Target::Gentoo).is_ok());

    let mut facts = base.clone();
    facts.architecture = "aarch64".into();
    assert_eq!(
        validate_host(&facts, Target::Alpine),
        Err(PreflightError::UnsupportedArchitecture)
    );

    let mut facts = base.clone();
    facts.effective_uid = 1000;
    assert_eq!(
        validate_host(&facts, Target::Alpine),
        Err(PreflightError::RootRequired)
    );

    let mut facts = base.clone();
    facts.root_fs_type = "tmpfs".into();
    assert_eq!(
        validate_host(&facts, Target::Alpine),
        Err(PreflightError::LiveRoot)
    );

    let mut facts = base.clone();
    facts.in_container = true;
    assert_eq!(
        validate_host(&facts, Target::Alpine),
        Err(PreflightError::ContainerUnsupported)
    );

    let mut facts = base;
    facts.memory_mb = Some(2047);
    assert!(matches!(
        validate_host(&facts, Target::Gentoo),
        Err(PreflightError::InsufficientRam { .. })
    ));
}

#[test]
fn memory_probe_parsers_match_existing_fallback_order() {
    assert_eq!(
        parse_lsmem_memory("Total online memory: 2147483648\n"),
        Some(2048)
    );
    assert_eq!(
        parse_lsmem_memory("Total online memory: not-a-number\n"),
        None
    );
    assert_eq!(
        parse_dmidecode_memory("Size: 4096 MB\nSize: 2 GB\nSize: No Module Installed\n"),
        Some(6144)
    );
    assert_eq!(parse_lshw_memory("System Memory 8GiB\n"), Some(8192));
    assert_eq!(
        parse_memtotal_fallback("MemTotal:       524288 kB\n"),
        Some(580)
    );
}

#[test]
fn ram_probe_calls_fallback_tools_in_the_existing_order() {
    let mut runner = MockRunner::new([
        CommandOutput::new(
            Some(0),
            b"Total online memory: unknown\n".to_vec(),
            Vec::new(),
        ),
        CommandOutput::new(Some(0), b"Size: 2 GB\n".to_vec(), Vec::new()),
    ]);
    assert_eq!(detect_ram_mb(&mut runner, ""), Some(2048));
    assert_eq!(
        runner
            .calls()
            .iter()
            .map(|call| call.program.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["lsmem", "dmidecode"]
    );
}

#[test]
fn proc_parsers_use_effective_uid_and_root_mount_entry() {
    let status = "Name:\ttest\nUid:\t1000\t0\t1000\t1000\n";
    assert_eq!(effective_uid(status), Some(0));
    assert_eq!(effective_uid("Name:\ttest\n"), None);
    assert!(root_mount_is_tmpfs(
        "tmpfs / tmpfs rw,nosuid 0 0\n/dev/vda1 /boot ext4 rw 0 0\n"
    ));
    assert!(!root_mount_is_tmpfs("/dev/vda1 / ext4 rw 0 0\n"));
}
