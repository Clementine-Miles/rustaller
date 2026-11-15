mod support;
mod live_preflight {
    use rustaller::codec::encode_plan;
    use rustaller::command::{CommandOutput, MockRunner};
    use rustaller::model::{InstallPlan, Target};
    use rustaller::worker::live::{
        LivePreflightError, load_and_validate_plan_with_uid, validate_live_plan_with_uid,
    };
    use std::fs;

    use crate::support::Scratch;

    fn write_plan(path: &std::path::Path, target: Target) {
        let encoded = encode_plan(&InstallPlan::new(target)).unwrap();
        fs::write(path, encoded).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn live_ram_gate_runs_for_each_automatic_target_before_workflow_creation() {
        for (target, floor) in [
            (Target::Alpine, 512),
            (Target::Debian, 512),
            (Target::Arch, 1024),
            (Target::Gentoo, 2048),
        ] {
            assert!(
                validate_live_plan_with_uid(
                    InstallPlan::new(target),
                    "x86_64",
                    floor,
                    rustaller::RUNTIME_VERSION,
                    0,
                )
                .is_ok()
            );
            assert!(matches!(
                validate_live_plan_with_uid(
                    InstallPlan::new(target),
                    "x86_64",
                    floor - 1,
                    rustaller::RUNTIME_VERSION,
                    0
                ),
                Err(LivePreflightError::InsufficientRam(_))
            ));
        }
    }

    #[test]
    fn live_worker_reads_private_plan_and_checks_uname_then_ram_without_disk_commands() {
        let scratch = Scratch::new("live_preflight");
        let plan_path = scratch.0.join("install.plan");
        write_plan(&plan_path, Target::Arch);
        let mut runner = MockRunner::new([
        CommandOutput::new(Some(0), b"x86_64\n".to_vec(), Vec::new()),
        CommandOutput::new(
            Some(0),
            b" total used free shared buff/cache available\nMem: 2048 100 1000 0 948 1800\nSwap: 0 0 0\n".to_vec(),
            Vec::new(),
        ),
    ]);
        let validated =
            load_and_validate_plan_with_uid(&mut runner, &plan_path, rustaller::RUNTIME_VERSION, 0)
                .unwrap();
        assert_eq!(validated.plan().target, Target::Arch);
        assert!(!plan_path.exists(), "one-shot plan is consumed on read");
        assert_eq!(runner.calls().len(), 2);
        assert_eq!(runner.calls()[0].program, "uname");
        assert_eq!(runner.calls()[0].args, ["-m"]);
        assert_eq!(runner.calls()[1].program, "free");
        assert_eq!(runner.calls()[1].args, ["-m"]);
    }

    #[test]
    fn live_worker_rechecks_ram_and_stops_before_any_disk_command() {
        let scratch = Scratch::new("live_preflight");
        let plan_path = scratch.0.join("install.plan");
        write_plan(&plan_path, Target::Gentoo);
        let mut runner = MockRunner::new([
        CommandOutput::new(Some(0), b"x86_64\n".to_vec(), Vec::new()),
        CommandOutput::new(
            Some(0),
            b" total used free shared buff/cache available\nMem: 2047 100 1000 0 947 1800\nSwap: 0 0 0\n".to_vec(),
            Vec::new(),
        ),
    ]);
        assert!(matches!(
            load_and_validate_plan_with_uid(&mut runner, &plan_path, rustaller::RUNTIME_VERSION, 0),
            Err(LivePreflightError::InsufficientRam(_))
        ));
        assert_eq!(runner.calls().len(), 2);
        assert_eq!(runner.calls()[0].program, "uname");
        assert_eq!(runner.calls()[1].program, "free");
        assert!(!plan_path.exists());
    }

    #[test]
    fn live_worker_refuses_non_root_before_reading_plan_or_running_utilities() {
        let mut runner = MockRunner::empty();
        assert!(matches!(
            load_and_validate_plan_with_uid(
                &mut runner,
                std::path::Path::new("/must-not-be-read"),
                rustaller::RUNTIME_VERSION,
                1000
            ),
            Err(LivePreflightError::RootRequired)
        ));
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn live_preflight_rejects_non_root_before_any_workflow_can_start() {
        assert!(matches!(
            validate_live_plan_with_uid(
                InstallPlan::new(Target::Alpine),
                "x86_64",
                512,
                rustaller::RUNTIME_VERSION,
                1000
            ),
            Err(LivePreflightError::RootRequired)
        ));
    }

    #[test]
    fn live_preflight_rejects_wrong_architecture_target_and_runtime_version() {
        assert!(matches!(
            validate_live_plan_with_uid(
                InstallPlan::new(Target::Alpine),
                "aarch64",
                2048,
                rustaller::RUNTIME_VERSION,
                0
            ),
            Err(LivePreflightError::UnsupportedArchitecture(_))
        ));
        assert!(matches!(
            validate_live_plan_with_uid(
                InstallPlan::new(Target::Reset),
                "x86_64",
                0,
                rustaller::RUNTIME_VERSION,
                0
            ),
            Err(LivePreflightError::NonAutomaticTarget(Target::Reset))
        ));
        assert!(matches!(
            validate_live_plan_with_uid(
                InstallPlan::new(Target::Alpine),
                "x86_64",
                512,
                "old-version",
                0
            ),
            Err(LivePreflightError::RuntimeVersionMismatch { .. })
        ));
    }
}

mod live_install {
    use rustaller::deployment::{
        DeploymentManifest, DiskIdentity, NetworkInterface, decode_manifest, encode_manifest,
    };
    use rustaller::host::sources::{GENTOO_ARCHIVE, GentooStage3};
    use rustaller::model::{BootMode, InstallPlan, Target};
    use rustaller::targets::install::{
        gentoo_sha512, valid_gentoo_signature, validate_install_options,
    };
    use rustaller::worker::install::{
        parse_unmounted_descendants, reject_target_swaps, same_physical_disk, validate_live_root,
    };
    use rustaller::worker::live::validate_live_plan_with_uid;

    fn manifest() -> DeploymentManifest {
        DeploymentManifest {
            disk: DiskIdentity {
                device: "/dev/vda".into(),
                size_bytes: 8 * 1024 * 1024 * 1024,
                serial: "rustaller-test-disk".into(),
                wwn: "".into(),
                partition_table_id: "aabbccdd".into(),
            },
            boot_mode: BootMode::Bios,
            alpine_branch: "v3.24".into(),
            debian_codename: "trixie".into(),
            gentoo_stage3: None,
            network: vec![NetworkInterface {
                name: "ens3".into(),
                mac: "52:54:00:11:22:33".into(),
                ipv4_addresses: vec!["192.0.2.7/32".into()],
                ipv4_gateway: Some("192.0.2.1".into()),
                ipv6_addresses: vec!["2001:db8::7/128".into(), "2001:db8::8/128".into()],
                ipv6_gateway: Some("fe80::1".into()),
                dhcp4: false,
                dhcp6: false,
                accept_ra: false,
            }],
        }
    }
    fn stage() -> GentooStage3 {
        let timestamp = "20261025T170057Z";
        let filename = format!("stage3-amd64-nomultilib-openrc-{timestamp}.tar.xz");
        let url = format!("{GENTOO_ARCHIVE}/{timestamp}/{filename}");
        GentooStage3 {
            timestamp: timestamp.into(),
            filename,
            size: 280774272,
            digests_url: format!("{url}.DIGESTS"),
            signature_url: format!("{url}.asc"),
            url,
        }
    }
    fn plan(target: Target) -> InstallPlan {
        let mut plan = InstallPlan::new(target);
        plan.username = Some("root".into());
        plan.ssh_keys = vec!["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA test".into()];
        plan
    }

    #[test]
    fn deployment_roundtrips_bounded_identity_network_and_gentoo_source_without_fake_pgp_armor() {
        for boot in [BootMode::Bios, BootMode::Efi] {
            let mut value = manifest();
            value.boot_mode = boot;
            value.gentoo_stage3 = Some(stage());
            assert_eq!(
                decode_manifest(&encode_manifest(&value).unwrap()).unwrap(),
                value
            );
        }
        let mut value = manifest();
        value.gentoo_stage3 = Some(stage());
        value.gentoo_stage3.as_mut().unwrap().url = "https://attacker.invalid/stage3.tar.xz".into();
        assert!(encode_manifest(&value).is_err());
        let mut bytes = encode_manifest(&manifest()).unwrap();
        bytes.push(0);
        assert!(decode_manifest(&bytes).is_err());
        for bytes in [b"".as_slice(), b"RDEPLOY\0\x02\0".as_slice()] {
            assert!(decode_manifest(bytes).is_err());
        }
    }

    #[test]
    fn physical_identity_survives_device_renaming_and_requires_all_recorded_identifiers() {
        let expected = manifest().disk;
        let mut found = expected.clone();
        found.device = "/dev/sda".into();
        assert!(same_physical_disk(&expected, &found));
        found.size_bytes += 512;
        assert!(!same_physical_disk(&expected, &found));
        found = expected.clone();
        found.serial = "different".into();
        assert!(!same_physical_disk(&expected, &found));
        found = expected.clone();
        found.partition_table_id = "deadbeef".into();
        assert!(!same_physical_disk(&expected, &found));
        let mut unbound = expected.clone();
        unbound.serial.clear();
        unbound.partition_table_id.clear();
        assert!(!same_physical_disk(&unbound, &expected));
    }

    #[test]
    fn running_root_and_target_inventory_guards_reject_host_mounts_swap_lvm_and_ambiguity() {
        for root in ["rootfs", "tmpfs", "ramfs"] {
            validate_live_root(root).unwrap();
        }
        for root in ["ext4", "overlay", "btrfs", "xfs", ""] {
            assert!(validate_live_root(root).is_err());
        }
        let descendants = parse_unmounted_descendants(
            "/dev/vda disk\n/dev/vda1 part\n/dev/vda2 part\n",
            "/dev/vda",
        )
        .unwrap();
        reject_target_swaps(&descendants, "Filename Type Size Used Priority\n").unwrap();
        assert!(
            reject_target_swaps(
                &descendants,
                "Filename Type Size Used Priority\n/dev/vda2 partition 2048 0 -2\n"
            )
            .is_err()
        );
        assert!(reject_target_swaps(&descendants, "").is_err());
        for text in [
            "/dev/vda disk /\n",
            "/dev/vda disk\n/dev/vg/root lvm\n",
            "/dev/vda disk\n/dev/mapper/root crypt\n",
            "/dev/vda1 part\n",
            "/dev/vda disk\n/dev/vda disk\n",
        ] {
            assert!(parse_unmounted_descendants(text, "/dev/vda").is_err());
        }
    }

    #[test]
    fn all_four_targets_validate_before_mutation_and_reject_unresolved_public_key_sources() {
        for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
            let mut metadata = manifest();
            if target == Target::Gentoo {
                metadata.gentoo_stage3 = Some(stage());
            }
            let selected = validate_live_plan_with_uid(
                plan(target),
                "x86_64",
                4096,
                rustaller::RUNTIME_VERSION,
                0,
            )
            .unwrap();
            validate_install_options(&selected, &metadata).unwrap();
        }
        let mut invalid = plan(Target::Debian);
        invalid.ssh_keys = vec!["/root/client.pub".into()];
        let selected =
            validate_live_plan_with_uid(invalid, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0)
                .unwrap();
        assert!(validate_install_options(&selected, &manifest()).is_err());
        let mut invalid = plan(Target::Alpine);
        invalid.username = Some("option;injection".into());
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn gentoo_source_init_mismatch_fails_before_installation_authorization() {
        let mut metadata = manifest();
        metadata.gentoo_stage3 = Some(stage());
        let mut p = plan(Target::Gentoo);
        p.system.gentoo_init = Some(rustaller::config::GentooInit::Systemd);
        let p =
            validate_live_plan_with_uid(p, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0).unwrap();
        assert!(validate_install_options(&p, &metadata).is_err());
        let stage = metadata.gentoo_stage3.as_mut().unwrap();
        stage.filename = stage.filename.replace("nomultilib-openrc", "systemd");
        assert!(metadata.validate().is_err());
    }

    #[test]
    fn gentoo_accepts_verified_subkey_primary_binding_and_rejects_expired_bad_or_multiple_signatures()
     {
        let good = "[GNUPG:] GOODSIG 2C44695DB9F6043D Gentoo\n[GNUPG:] VALIDSIG 534E4209AB49EEE1C19D96162C44695DB9F6043D 2026-10-26 1792974065 0 4 0 1 8 01 13EBBDBEDE7A12775DFDB1BABB572E0E2D182910\n";
        assert!(valid_gentoo_signature(good));
        assert!(!valid_gentoo_signature(&good.replace(
            "13EBBDBEDE7A12775DFDB1BABB572E0E2D182910",
            "D99EAC7379A850BCE47DA5F29E6438C817072058"
        )));
        assert!(!valid_gentoo_signature(&format!("{good}{good}")));
        assert!(!valid_gentoo_signature(&format!(
            "{good}[GNUPG:] EXPKEYSIG 2C44695DB9F6043D Gentoo\n"
        )));
        let selected = stage();
        let digest = "a".repeat(128);
        let body = format!("# SHA512 HASH\n{digest}  {}\n", selected.filename);
        assert_eq!(gentoo_sha512(&selected, &body).unwrap(), digest);
        assert!(gentoo_sha512(&selected, &format!("{body}{body}")).is_err());
        assert!(
            gentoo_sha512(&selected, &body.replace(&selected.filename, "other.tar.xz")).is_err()
        );
    }
}
