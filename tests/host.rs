mod support;
mod preflight {
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
}

mod disk {
    use rustaller::command::{CommandOutput, MockRunner};
    use rustaller::host::disk::{
        DiskError, discover_root_disks, parse_partition_table_id, parse_root_device,
        parse_root_disk_names, read_partition_table_id, select_root_disk,
        validate_target_disk_path,
    };
    use std::fs;
    use std::path::Path;

    #[test]
    fn parses_root_device_and_all_underlying_disk_names_from_utilities() {
        assert_eq!(
            parse_root_device(
                "proc on /proc type proc (rw)\n/dev/mapper/vg-root on / type xfs (rw)\n"
            ),
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
            parse_partition_table_id(
                "Disk identifier (GUID): d6b17c1a-40a1-4bd2-8f00-123456789abc\n"
            )
            .unwrap(),
            "d6b17c1a-40a1-4bd2-8f00-123456789abc"
        );
        assert!(parse_partition_table_id("Disk identifier: invalid\n").is_err());
    }

    #[test]
    fn target_disk_paths_must_resolve_to_block_devices_under_dev() {
        let scratch = crate::support::Scratch::new("disk");
        let regular_file = scratch.0.join("regular");
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
}

mod hostname {
    use crate::support::Scratch;
    use rustaller::host::hostname::{default_hostname, detect_hostname, resolve_install_hostname};
    use rustaller::model::{InstallPlan, Target};

    use std::fs;
    fn directory() -> Scratch {
        let dir = Scratch::with_etc("hostname");
        fs::create_dir_all(dir.0.join("proc/sys/kernel")).unwrap();
        dir
    }

    #[test]
    fn source_hostname_is_bounded_validated_and_preferred_to_runtime_hostname() {
        let dir = directory();
        fs::write(dir.0.join("etc/hostname"), "previous-vps.example.org\n").unwrap();
        fs::write(dir.0.join("proc/sys/kernel/hostname"), "running-vps\n").unwrap();
        assert_eq!(detect_hostname(&dir.0).as_deref(), Some("previous-vps"));
        for invalid in [
            "localhost\n".into(),
            "bad name".into(),
            "safe.bad/domain".into(),
            "x".repeat(257),
        ] {
            fs::write(dir.0.join("etc/hostname"), invalid).unwrap();
            assert_eq!(detect_hostname(&dir.0).as_deref(), Some("running-vps"));
        }
    }

    #[test]
    fn missing_hostname_uses_distribution_and_explicit_values_and_rescue_are_preserved() {
        let dir = directory();
        for (target, expected) in [
            (Target::Alpine, "alpine"),
            (Target::Debian, "debian"),
            (Target::Arch, "arch"),
            (Target::Gentoo, "gentoo"),
        ] {
            assert_eq!(default_hostname(target, &dir.0), expected);
            let mut plan = InstallPlan::new(target);
            resolve_install_hostname(&mut plan, &dir.0);
            assert_eq!(plan.post_install.hostname.as_deref(), Some(expected));
            plan.post_install.hostname = Some("chosen".into());
            resolve_install_hostname(&mut plan, &dir.0);
            assert_eq!(plan.post_install.hostname.as_deref(), Some("chosen"));
        }
        let mut rescue = InstallPlan::new(Target::Alpine);
        rescue.hold = Some(1);
        resolve_install_hostname(&mut rescue, &dir.0);
        assert!(rescue.post_install.hostname.is_none());
    }
}

mod handoff {
    use rustaller::command::{CommandOutput, MockRunner};
    use rustaller::deployment::{DeploymentManifest, DiskIdentity, NetworkInterface};
    use rustaller::host::handoff::{
        ALPINE_RELEASE_FINGERPRINT, HostDeployment, HostInit, confirmation_for,
        confirmation_for_plan, detect_init, parse_addresses, pinned_primary_key, prepare,
        read_disk_identity, resolve_public_keys, valid_alpine_signature,
    };
    use rustaller::host::sources::AlpineNetboot;
    use rustaller::model::{BootMode, InstallPlan, SecretString, Target};
    use rustaller::worker::initrd_network::AddressFamily;
    use std::fs;
    use std::path::Path;

    use crate::support::Scratch;
    fn output(text: &str) -> CommandOutput {
        CommandOutput::new(Some(0), text.as_bytes().to_vec(), vec![])
    }
    fn disk() -> DiskIdentity {
        DiskIdentity {
            device: "/dev/vda".into(),
            size_bytes: 8 * 1024_u64.pow(3),
            serial: "acceptance-disk".into(),
            wwn: String::new(),
            partition_table_id: "1234abcd".into(),
        }
    }
    fn plan() -> InstallPlan {
        let mut plan = InstallPlan::new(Target::Debian);
        plan.password = Some(SecretString::new("private-password-sentinel"));
        plan
    }
    fn deployment(plan: &InstallPlan) -> HostDeployment {
        let source=AlpineNetboot {version:"3.24.3".into(),branch:"v3.24".into(),filename:"alpine-netboot-3.24.3-x86_64.tar.gz".into(),url:"https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-netboot-3.24.3-x86_64.tar.gz".into(),sha256:"a".repeat(64),checksum_url:"https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-netboot-3.24.3-x86_64.tar.gz.sha256".into(),signature_url:"https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-netboot-3.24.3-x86_64.tar.gz.asc".into()};
        HostDeployment {
            manifest: DeploymentManifest {
                disk: disk(),
                boot_mode: BootMode::Bios,
                alpine_branch: source.branch.clone(),
                debian_codename: "trixie".into(),
                gentoo_stage3: None,
                network: vec![NetworkInterface {
                    name: "eth0".into(),
                    mac: "52:54:00:12:34:56".into(),
                    ipv4_addresses: vec!["192.0.2.4/24".into()],
                    ipv4_gateway: Some("192.0.2.1".into()),
                    ipv6_addresses: vec![],
                    ipv6_gateway: None,
                    dhcp4: true,
                    dhcp6: false,
                    accept_ra: false,
                }],
            },
            confirmation: confirmation_for_plan(plan, &disk()),
            init: HostInit::Systemd,
            alpine: source,
        }
    }

    #[test]
    fn only_native_init_kexec_adapters_are_selected() {
        assert_eq!(
            detect_init(Path::new("/usr/lib/systemd/systemd")).unwrap(),
            HostInit::Systemd
        );
        assert_eq!(
            detect_init(Path::new("/sbin/openrc-init")).unwrap(),
            HostInit::OpenRcInit
        );
        for init in [
            "/bin/busybox",
            "/sbin/init",
            "/sbin/runit",
            "/usr/bin/python",
        ] {
            assert!(detect_init(Path::new(init)).is_err());
        }
    }

    #[test]
    fn disk_inspection_uses_exact_columns_and_confirmation_binds_every_identifier() {
        let mut runner = MockRunner::new([
            output("8589934592\n"),
            output("acceptance-disk\n"),
            output("\n"),
            output("1234abcd\n"),
        ]);
        assert_eq!(read_disk_identity(&mut runner, "/dev/vda").unwrap(), disk());
        assert_eq!(
            runner
                .calls()
                .iter()
                .map(|call| call.args[1].to_str().unwrap())
                .collect::<Vec<_>>(),
            ["SIZE", "SERIAL", "WWN", "PTUUID"]
        );
        assert_eq!(
            confirmation_for(&disk()),
            "ERASE /dev/vda 8589934592 SERIAL=acceptance-disk WWN= PTUUID=1234abcd"
        );
        let mut rescue = plan();
        rescue.target = Target::Alpine;
        rescue.hold = Some(1);
        assert!(confirmation_for_plan(&rescue, &disk()).starts_with("RESCUE "));
        let mut empty = MockRunner::empty();
        assert!(read_disk_identity(&mut empty, "/dev/../sda").is_err());
        assert!(empty.calls().is_empty());
    }

    #[test]
    fn confirmation_firmware_and_source_identity_fail_before_workspace_or_downloads() {
        let scratch = Scratch::new("handoff");
        let plan = plan();
        let inventory = deployment(&plan);
        let workspace = scratch.0.join("new-workspace");
        let mut runner = MockRunner::empty();
        assert!(
            prepare(
                &mut runner,
                &plan,
                &inventory,
                "yes",
                &workspace,
                b"not-a-worker"
            )
            .is_err()
        );
        assert!(!workspace.exists());
        assert!(runner.calls().is_empty());
        let mut wrong_source = deployment(&plan);
        wrong_source.alpine.filename = "../outside".into();
        assert!(
            prepare(
                &mut runner,
                &plan,
                &wrong_source,
                &wrong_source.confirmation,
                &workspace,
                b"not-a-worker"
            )
            .is_err()
        );
        assert!(!workspace.exists());
        assert!(runner.calls().is_empty());
        let mut forced = plan;
        forced.force_boot_mode = Some(BootMode::Efi);
        assert!(
            prepare(
                &mut runner,
                &forced,
                &inventory,
                &inventory.confirmation,
                &workspace,
                b"not-a-worker"
            )
            .is_err()
        );
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn changed_disk_identity_stops_before_any_download_or_staging() {
        let scratch = Scratch::new("handoff");
        let plan = plan();
        let inventory = deployment(&plan);
        let workspace = scratch.0.join("new-workspace");
        let mut runner = MockRunner::new([
            output("8589934592\n"),
            output("different-disk\n"),
            output("\n"),
            output("1234abcd\n"),
        ]);
        assert!(
            prepare(
                &mut runner,
                &plan,
                &inventory,
                &inventory.confirmation,
                &workspace,
                b"not-a-worker"
            )
            .is_err()
        );
        assert_eq!(runner.calls().len(), 4);
        assert!(!workspace.exists());
    }

    #[test]
    fn release_key_parsing_rejects_unpinned_or_ambiguous_primary_keys_and_bad_status() {
        let colons = format!(
            "pub:-:4096:1:key:0:0::::::\nfpr:::::::::{ALPINE_RELEASE_FINGERPRINT}:\nsub:-:4096:1:subkey:0:0::::::\nfpr:::::::::{}:\n",
            "b".repeat(40)
        );
        assert!(pinned_primary_key(&colons));
        assert!(!pinned_primary_key(&format!("{colons}{colons}")));
        assert!(!pinned_primary_key(
            &colons.replace(ALPINE_RELEASE_FINGERPRINT, &"c".repeat(40))
        ));
        let valid = format!(
            "[GNUPG:] GOODSIG key signer\n[GNUPG:] VALIDSIG {} 2026-10-26 0 0 4 0 1 8 00 {ALPINE_RELEASE_FINGERPRINT}\n",
            "b".repeat(40)
        );
        assert!(valid_alpine_signature(&valid));
        for status in [
            "BADSIG",
            "ERRSIG",
            "EXPSIG",
            "EXPKEYSIG",
            "REVKEYSIG",
            "NO_PUBKEY",
        ] {
            assert!(!valid_alpine_signature(&format!(
                "{valid}[GNUPG:] {status} key\n"
            )));
        }
        assert!(!valid_alpine_signature(&format!("{valid}{valid}")));
        assert!(!valid_alpine_signature(
            &valid.replace(ALPINE_RELEASE_FINGERPRINT, &"c".repeat(40))
        ));
    }

    #[test]
    fn key_resolution_reads_sources_on_host_and_validates_every_unique_key_without_echoing() {
        let scratch = Scratch::new("handoff");
        let file = scratch.0.join("client.pub");
        fs::write(
            &file,
            "# ignored\nssh-ed25519 AAAA first\nssh-rsa BBBB second\n",
        )
        .unwrap();
        let mut runner = MockRunner::new([output("fingerprint1"), output("fingerprint2")]);
        let result = resolve_public_keys(
            &mut runner,
            &scratch.0,
            &[
                file.to_str().unwrap().into(),
                "ssh-ed25519 AAAA first".into(),
            ],
        )
        .unwrap();
        assert_eq!(result, ["ssh-ed25519 AAAA first", "ssh-rsa BBBB second"]);
        assert_eq!(runner.calls().len(), 2);
        assert!(
            runner
                .calls()
                .iter()
                .all(|call| call.program == "ssh-keygen")
        );
        let debug = format!("{:?}", runner.calls());
        assert!(!debug.contains("AAAA"));
        assert!(!debug.contains("BBBB"));
        let other = Scratch::new("handoff");
        let private = other.0.join("accidental-private");
        fs::write(
            &private,
            "-----BEGIN OPENSSH PRIVATE KEY-----\nprivate-sentinel\n",
        )
        .unwrap();
        let mut empty = MockRunner::empty();
        let error = resolve_public_keys(&mut empty, &other.0, &[private.to_str().unwrap().into()])
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("private-sentinel"));
        assert!(empty.calls().is_empty());
    }

    #[test]
    fn network_address_parser_preserves_multiple_cidrs_but_rejects_missing_family_fields() {
        let text = "2: eth0 inet 192.0.2.4/24 brd 192.0.2.255 scope global dynamic eth0\n2: eth0 inet 192.0.2.5/24 scope global eth0\n";
        assert_eq!(
            parse_addresses(text, AddressFamily::Ipv4).unwrap(),
            ["192.0.2.4/24", "192.0.2.5/24"]
        );
        assert!(parse_addresses("2: eth0 missing-address\n", AddressFamily::Ipv4).is_err());
        assert!(parse_addresses("2: eth0 inet\n", AddressFamily::Ipv4).is_err());
    }

    #[test]
    fn inherited_default_routes_reject_ambiguity_multipath_and_wrong_families() {
        use rustaller::host::handoff::parse_default_gateway;
        assert_eq!(
            parse_default_gateway("", AddressFamily::Ipv4).unwrap(),
            None
        );
        assert_eq!(
            parse_default_gateway(
                "default via 192.0.2.1 dev eth0 proto dhcp metric 100\n",
                AddressFamily::Ipv4
            )
            .unwrap()
            .as_deref(),
            Some("192.0.2.1")
        );
        assert_eq!(
            parse_default_gateway(
                "default via fe80::1 dev eth0 proto ra\n",
                AddressFamily::Ipv6
            )
            .unwrap()
            .as_deref(),
            Some("fe80::1")
        );
        for route in [
            "default via 192.0.2.1 dev eth0\ndefault via 192.0.2.2 dev eth0\n",
            "default nexthop via 192.0.2.1 dev eth0 weight 1",
            "default nhid 5",
            "default dev eth0",
            "default via 127.1 dev eth0",
            "default via fe80::1 dev eth0",
        ] {
            assert!(
                parse_default_gateway(route, AddressFamily::Ipv4).is_err(),
                "{route}"
            );
        }
    }

    #[test]
    fn inherited_address_capture_checks_dad_and_ignores_temporary_slaac_addresses() {
        for flag in ["tentative", "dadfailed"] {
            assert!(
                parse_addresses(
                    &format!("2: eth0 inet6 2001:db8::1/64 scope global {flag}\n"),
                    AddressFamily::Ipv6
                )
                .is_err()
            );
        }
        assert!(
            parse_addresses(
                "2: eth0 inet6 2001:db8::1/64 scope global temporary dynamic\n",
                AddressFamily::Ipv6
            )
            .unwrap()
            .is_empty()
        );
        assert!(
            parse_addresses(
                "2: eth0 inet 01.2.3.4/24 scope global\n",
                AddressFamily::Ipv4
            )
            .is_err()
        );
    }
}

mod keys {
    use rustaller::codec::encode_plan;
    use rustaller::command::{
        CommandInput, CommandOutput, CommandRunner, CommandSpec, PipelineOutput,
    };

    use rustaller::host::ssh_keygen::{KeyType, KeygenOptions, generate};
    use rustaller::model::{InstallPlan, Target};

    use std::fs;
    use std::io;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    use crate::support::Scratch;
    fn out(status: i32, text: &str) -> CommandOutput {
        CommandOutput::new(Some(status), text.as_bytes().to_vec(), vec![])
    }
    struct FakeKeygen {
        calls: Vec<CommandSpec>,
        status: i32,
    }
    impl CommandRunner for FakeKeygen {
        fn run(&mut self, spec: &CommandSpec) -> io::Result<CommandOutput> {
            self.calls.push(spec.clone());
            assert_eq!(spec.program, "ssh-keygen");
            let output = PathBuf::from(spec.args.last().unwrap());
            fs::write(&output, "PRIVATE-TEST-SENTINEL")?;
            let mut public = output.into_os_string();
            public.push(".pub");
            fs::write(
                PathBuf::from(public),
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA test\n",
            )?;
            Ok(out(self.status, ""))
        }
        fn run_pipeline(&mut self, _: &[CommandSpec]) -> io::Result<PipelineOutput> {
            panic!("no pipeline")
        }
    }
    fn key_options(scratch: &Scratch) -> KeygenOptions {
        KeygenOptions {
            key_type: KeyType::Ed25519,
            rounds: 512,
            cipher: "aes256-ctr".into(),
            rsa_bits: 4096,
            comment: "20261026 $(not-a-shell)".into(),
            output: scratch.0.join("client-key"),
        }
    }

    #[test]
    fn ssh_keygen_calls_system_tool_with_separate_arguments_and_never_serializes_private_key_or_passphrase()
     {
        let scratch = Scratch::with_etc("advanced_config");
        let options = key_options(&scratch);
        let mut runner = FakeKeygen {
            calls: vec![],
            status: 0,
        };
        let key = generate(&mut runner, &options).unwrap();
        let args = runner.calls[0]
            .args
            .iter()
            .map(|a| a.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            &args[..9],
            [
                "-o",
                "-a",
                "512",
                "-Z",
                "aes256-ctr",
                "-t",
                "ed25519",
                "-C",
                "20261026 $(not-a-shell)"
            ]
        );
        assert!(!args.contains(&"-N"));
        assert_eq!(runner.calls[0].stdin, CommandInput::Inherit);
        assert_eq!(
            fs::metadata(&key.private_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&key.public_path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let mut selected = InstallPlan::new(Target::Debian);
        selected.ssh_keys.push(key.public_key);
        let encoded = encode_plan(&selected).unwrap();
        assert!(
            !encoded
                .windows(b"PRIVATE-TEST-SENTINEL".len())
                .any(|s| s == b"PRIVATE-TEST-SENTINEL")
        );
        assert!(
            !encoded
                .windows(b"client-key".len())
                .any(|s| s == b"client-key")
        );
        assert!(!fs::read_dir(&scratch.0).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".rustaller-keygen")
        }));
        assert!(generate(&mut runner, &options).is_err());
        assert_eq!(runner.calls.len(), 1);
        assert_eq!(scratch.read("client-key"), "PRIVATE-TEST-SENTINEL");
    }

    #[test]
    fn ssh_keygen_failure_and_invalid_options_do_not_publish_or_overwrite_keys() {
        let scratch = Scratch::with_etc("advanced_config");
        let mut runner = FakeKeygen {
            calls: vec![],
            status: 1,
        };
        let mut options = key_options(&scratch);
        assert!(generate(&mut runner, &options).is_err());
        assert!(!options.output.exists());
        assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 1);
        options.rounds = 0;
        assert!(generate(&mut runner, &options).is_err());
        options.rounds = 512;
        options.cipher = "arbitrary -N secret".into();
        assert!(generate(&mut runner, &options).is_err());
        options.cipher = "aes256-ctr".into();
        options.output = "relative".into();
        assert!(generate(&mut runner, &options).is_err());
        options = key_options(&scratch);
        symlink(scratch.0.join("not-created"), &options.output).unwrap();
        assert!(generate(&mut runner, &options).is_err());
        assert!(!scratch.0.join("not-created").exists());
        assert_eq!(runner.calls.len(), 1);
    }
}

mod kexec {

    use rustaller::host::preflight::{KexecError, validate_one_reboot_support};

    #[test]
    fn single_reboot_contract_fails_closed_without_kexec_and_never_downgrades_to_grub_restart() {
        let config = "CONFIG_KEXEC_FILE=y\nCONFIG_KEXEC_CORE=y\n";
        validate_one_reboot_support(config, "0", Some("[none] integrity confidentiality"), true)
            .unwrap();
        for (disabled, lockdown, tool, expected) in [
            ("1", None, true, KexecError::AdministrativelyDisabled),
            ("", None, true, KexecError::UnknownPolicy),
            (
                "0",
                Some("none [integrity] confidentiality"),
                true,
                KexecError::Lockdown,
            ),
            ("0", Some("unknown"), true, KexecError::UnknownPolicy),
            ("0", None, false, KexecError::ToolMissing),
        ] {
            assert_eq!(
                validate_one_reboot_support(config, disabled, lockdown, tool),
                Err(expected)
            );
        }
        assert_eq!(
            validate_one_reboot_support("CONFIG_KEXEC=y\n", "0", None, true),
            Err(KexecError::KernelUnsupported)
        );
    }
}

mod inspection_preview {
    use crate::support::Scratch;
    use rustaller::{
        command::{CommandOutput, MockRunner},
        deployment::{DeploymentManifest, DiskIdentity},
        model::{BootMode, InstallPlan, Target},
    };
    #[test]
    fn inspection_keeps_detected_facts_separate_from_computed_target_and_does_not_mutate() {
        let d = Scratch::new("host-preview");
        d.write("etc/os-release", "PRETTY_NAME=\"Source Linux\"\n");
        d.write("proc/sys/kernel/osrelease", "6.12-source\n");
        d.write(
            "proc/cpuinfo",
            "processor: 0\nmodel name: Virtual CPU\nprocessor: 1\n",
        );
        d.write(
            "proc/meminfo",
            "MemTotal: 2048000 kB\nMemAvailable: 1024000 kB\n",
        );
        d.write("etc/resolv.conf", "nameserver 192.0.2.53\n");
        let manifest = DeploymentManifest {
            disk: DiskIdentity {
                device: "/dev/vda".into(),
                size_bytes: 3 * 1024_u64.pow(4),
                serial: "DISK-A".into(),
                wwn: String::new(),
                partition_table_id: "OLD".into(),
            },
            boot_mode: BootMode::Bios,
            alpine_branch: "v3.24".into(),
            debian_codename: "".into(),
            gentoo_stage3: None,
            network: vec![],
        };
        let mut plan = InstallPlan::new(Target::Debian);
        plan.system.root_filesystem = rustaller::config::RootFilesystem::Btrfs;
        plan.password = Some(rustaller::model::SecretString::new("PRIVATE-SENTINEL"));
        let mut runner = MockRunner::new(
            [
                "kvm",
                "512 dos",
                "/dev/vda 3T disk\n/dev/vda1 3T part ext4",
                "port 2222\npasswordauthentication no\nprivate-field hidden",
            ]
            .map(|v| CommandOutput::new(Some(0), v.as_bytes().to_vec(), vec![])),
        );
        let report = rustaller::host::inspect::report(&mut runner, &d.0, &plan, &manifest);
        assert!(
            report.current.contains("Source Linux")
                && report.current.contains("vCPU: 2")
                && report.current.contains("6.12-source")
        );
        assert!(report.current.contains("Current bootloader: unknown"));
        assert!(
            report.target.contains("count: 3")
                && report.target.contains("Limine")
                && report.target.contains("@; zstd level 3")
        );
        assert!(report.warning.contains("ERASE ALL PARTITIONS ON /dev/vda"));
        assert!(
            !report.text().contains("PRIVATE-SENTINEL") && !report.text().contains("private-field")
        );
        assert_eq!(d.read("etc/resolv.conf"), "nameserver 192.0.2.53\n");
        assert_eq!(
            runner
                .calls()
                .iter()
                .map(|c| c.program.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["systemd-detect-virt", "lsblk", "lsblk", "sshd"]
        );
    }
}
