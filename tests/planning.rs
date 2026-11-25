mod support;
mod args {
    use rustaller::host::args::{CliOutcome, parse_args};
    use rustaller::model::{BootMode, Target};

    fn parse(args: &[&str]) -> CliOutcome {
        parse_args(args.iter().copied()).expect("valid CLI arguments")
    }

    #[test]
    fn explicit_ethernet_policy_round_trips_and_rejects_ambiguous_options() {
        let mac = "--network-mac=52:54:00:12:34:56";
        let CliOutcome::Plan(plan) = parse(&[
            "debian",
            mac,
            "--dhcp4",
            "--ipv6-address=2001:db8::5/64",
            "--ipv6-gateway=fe80::1",
            "--dns=2001:db8::53",
            "--mtu=1400",
        ]) else {
            panic!("expected an install plan");
        };
        let policy = plan.system.explicit_network.as_ref().unwrap();
        assert!(policy.dhcp4);
        assert_eq!(policy.ipv6_address.as_deref(), Some("2001:db8::5/64"));
        let decoded =
            rustaller::codec::decode_plan(&rustaller::codec::encode_plan(&plan).unwrap()).unwrap();
        assert_eq!(decoded.system, plan.system);
        for args in [
            vec!["debian", "--dhcp4"],
            vec!["debian", mac, "--ipv4-gateway=192.0.2.1", "--dhcp4"],
            vec!["debian", mac, "--ipv6-address=2001:db8::5/64", "--ipv6-ra"],
            vec!["debian", mac, "--ipv6=disable", "--dhcp6"],
            vec!["debian", mac, "--dhcp4", "--dns=2001:db8::53"],
        ] {
            assert!(parse_args(args).is_err());
        }
        assert!(matches!(parse_args(["reset"]), Ok(CliOutcome::Reset)));
        assert!(parse_args(["reset", "--filesystem=ext4"]).is_err());
    }

    #[test]
    fn parses_target_and_current_option_aliases() {
        let CliOutcome::Plan(plan) = parse(&[
            "--user=admin",
            "--ssh-port",
            "2222",
            "--sleep",
            "2",
            "--force-boot-mode",
            "efi",
            "debian",
        ]) else {
            panic!("expected install plan");
        };

        assert_eq!(plan.target, Target::Debian);
        assert_eq!(plan.username.as_deref(), Some("admin"));
        assert_eq!(plan.ssh_port, Some(2222));
        assert_eq!(plan.hold, Some(2));
        assert_eq!(plan.force_boot_mode, Some(BootMode::Efi));
    }

    #[test]
    fn help_short_circuits_invalid_options_before_host_checks() {
        assert!(matches!(
            parse_args(["--nonsense", "--help"]),
            Ok(CliOutcome::Help)
        ));
        assert!(matches!(parse_args(["-h"]), Ok(CliOutcome::Help)));
    }

    #[test]
    fn separates_read_only_inspection_and_explicit_disk_confirmation_from_the_install_plan() {
        let CliOutcome::Inspect(plan) =
            parse_args(["debian", "--inspect", "--target-disk=/dev/vda"]).unwrap()
        else {
            panic!("inspection")
        };
        assert_eq!(plan.target, Target::Debian);
        let token = "ERASE /dev/vda 8589934592 SERIAL=vm WWN= PTUUID=12345678";
        let CliOutcome::ConfirmedPlan { plan, confirmation } = parse_args([
            "alpine",
            "--confirm-erase",
            token,
            "--ssh-key=/root/client.pub",
        ])
        .unwrap() else {
            panic!("confirmed plan")
        };
        assert_eq!(confirmation, token);
        assert_eq!(plan.ssh_keys, ["/root/client.pub"]);
        for args in [
            vec!["debian", "--inspect", "--confirm-erase", token],
            vec!["debian", "--inspect=true"],
            vec!["debian", "--confirm-erase", "bad\nvalue"],
            vec!["debian", "--confirm-erase", token, "--confirm-erase", token],
            vec!["reset", "--inspect"],
            vec!["netboot.xyz", "--confirm-erase", token],
        ] {
            assert!(parse_args(args).is_err());
        }
        assert!(matches!(
            parse_args(["alpine", "--hold=1", "--inspect"]),
            Ok(CliOutcome::Inspect(_))
        ));
        assert!(matches!(
            parse_args([
                "alpine",
                "--hold=1",
                "--confirm-erase",
                "RESCUE disk identity"
            ]),
            Ok(CliOutcome::ConfirmedPlan { .. })
        ));
    }

    #[test]
    fn rejects_missing_target_unknown_options_and_removed_credentials() {
        assert!(parse_args::<_, &str>([]).is_err());
        assert!(parse_args(["--unknown", "alpine"]).is_err());
        for flag in ["--password", "--passwd", "--root-password"] {
            assert!(parse_args(["alpine", flag, "credential"]).is_err());
            let option = format!("{flag}=PRIVATE-SENTINEL");
            let error = parse_args(["alpine", &option]).err().expect("removed flag");
            assert!(!error.to_string().contains("PRIVATE-SENTINEL"));
        }
    }

    #[test]
    fn target_configuration_uses_shared_validation_and_rejects_kernel_overrides() {
        let CliOutcome::Plan(plan) =
            parse_args(["arch", "--hostname", "my-vps", "--timezone", "Etc/UTC"]).unwrap()
        else {
            panic!("plan")
        };
        assert_eq!(plan.post_install.hostname.as_deref(), Some("my-vps"));
        assert_eq!(plan.post_install.timezone.as_deref(), Some("Etc/UTC"));
        for args in [
            vec!["debian", "--kernel", "lts"],
            vec!["arch", "--kernel", "cloud"],
            vec!["gentoo", "--kernel", "generic"],
            vec!["debian", "--kernel", "generic", "--no-cloud-kernel"],
            vec!["debian", "--kernel", "https://example/kernel"],
            vec!["alpine", "--hostname", "bad\nname"],
            vec!["alpine", "--timezone", "../../etc"],
            vec!["alpine", "--hold", "1", "--hostname", "vps"],
            vec!["reset", "--hostname", "vps"],
            vec!["netboot.xyz", "--kernel", "standard"],
            vec!["iso", "--iso", "/images/live.iso", "--timezone", "Etc/UTC"],
        ] {
            assert!(parse_args(&args).is_err(), "accepted {args:?}");
        }
        assert!(parse_args(["debian", "--no-cloud-kernel"]).is_err());
    }

    #[test]
    fn filesystem_and_non_root_key_configuration_allow_read_only_inspection() {
        use rustaller::config::RootFilesystem;
        let CliOutcome::Plan(plan) = parse_args([
            "debian",
            "--username=admin",
            "--ssh-key",
            "/root/client.pub",
            "--filesystem=ext4",
        ])
        .unwrap() else {
            panic!("plan")
        };
        assert_eq!(plan.system.root_filesystem, RootFilesystem::Ext4);
        assert_eq!(plan.ssh_keys, ["/root/client.pub"]);
        assert!(plan.validate_install_credentials().is_ok());
        assert!(matches!(
            parse_args(["debian", "--username=admin", "--inspect"]),
            Ok(CliOutcome::Inspect(_))
        ));
        for args in [
            vec!["debian", "--filesystem=btrfs"],
            vec!["debian", "--btrfs-compression=3"],
            vec!["debian", "--hold=1", "--filesystem=ext4"],
            vec!["netboot.xyz", "--filesystem=ext4"],
        ] {
            assert!(parse_args(args).is_err());
        }
    }
}

mod model {
    use rustaller::model::{
        BootMode, CURRENT_SCHEMA_VERSION, InstallPlan, Target, validate_host_arch, validate_ram,
    };

    #[test]
    fn target_ram_floors_match_installer_contract() {
        assert_eq!(Target::Alpine.minimum_ram_mb(), 512);
        assert_eq!(Target::Debian.minimum_ram_mb(), 512);
        assert_eq!(Target::Arch.minimum_ram_mb(), 1024);
        assert_eq!(Target::Gentoo.minimum_ram_mb(), 2048);
        assert_eq!(Target::Reset.minimum_ram_mb(), 0);
    }

    #[test]
    fn architecture_and_ram_gates_match_installer_contract() {
        assert!(validate_host_arch("x86_64").is_ok());
        for machine in ["aarch64", "i686", "AMD64", ""] {
            assert!(validate_host_arch(machine).is_err(), "accepted {machine:?}");
        }

        for (target, floor) in [
            (Target::Alpine, 512),
            (Target::Debian, 512),
            (Target::Arch, 1024),
            (Target::Gentoo, 2048),
        ] {
            assert!(validate_ram(target, floor - 1).is_err());
            assert!(validate_ram(target, floor).is_ok());
        }
    }

    #[test]
    fn automatic_install_uses_embedded_assets_and_validates_schema() {
        let mut plan = InstallPlan::new(Target::Debian);
        assert!(plan.validate().is_ok());

        assert!(plan.validate().is_ok());
        assert_eq!(plan.schema_version, CURRENT_SCHEMA_VERSION);

        plan.schema_version += 1;
        assert!(plan.validate().is_err());
    }

    #[test]
    fn account_names_reject_argument_and_line_injection_before_handoff() {
        let mut plan = InstallPlan::new(Target::Debian);
        for name in [
            "-root",
            "0root",
            "root\nadmin",
            "root name",
            "root/other",
            "none",
            "",
        ] {
            plan.username = Some(name.into());
            assert!(plan.validate().is_err());
        }
        for name in ["root", "_service", "admin-1"] {
            plan.username = Some(name.into());
            assert!(plan.validate().is_ok());
        }
    }

    #[test]
    fn netboot_and_reset_reject_automatic_install_fields() {
        let mut reset = InstallPlan::new(Target::Reset);
        reset.hold = Some(1);
        assert!(reset.validate().is_ok());
    }

    #[test]
    fn boot_mode_has_only_bios_and_efi_variants() {
        assert_eq!(BootMode::Bios.as_str(), "bios");
        assert_eq!(BootMode::Efi.as_str(), "efi");
    }

    #[test]
    fn root_and_non_root_installation_and_rescue_require_keys_at_dispatch() {
        use rustaller::model::PlanError;
        for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
            for (account, hold) in [("root", None), ("admin", None), ("admin", Some(1))] {
                let mut plan = InstallPlan::new(target);
                plan.username = Some(account.into());
                plan.hold = hold;
                assert!(plan.validate().is_ok());
                assert_eq!(
                    plan.validate_install_credentials(),
                    Err(PlanError::MissingSshKey)
                );
                plan.ssh_keys.push(crate::support::PUBLIC_KEY.into());
                assert!(plan.validate_install_credentials().is_ok());
                assert!(!format!("{plan:?}").contains(crate::support::PUBLIC_KEY));
            }
        }
        assert!(
            InstallPlan::new(Target::Reset)
                .validate_install_credentials()
                .is_ok()
        );
    }

    #[test]
    fn filesystem_choices_validate_routes_and_keep_xfs_as_the_default() {
        use rustaller::config::RootFilesystem;
        assert_eq!(RootFilesystem::default(), RootFilesystem::Xfs);
        assert_eq!(
            "ext4".parse::<RootFilesystem>().unwrap(),
            RootFilesystem::Ext4
        );
        for invalid in ["", "zfs", "btrfs", "ext4;reboot", "EXT4"] {
            assert!(invalid.parse::<RootFilesystem>().is_err());
        }
        let mut plan = InstallPlan::new(Target::Debian);
        plan.system.root_filesystem = RootFilesystem::Ext4;
        assert!(plan.validate().is_ok());
        plan.hold = Some(1);
        assert!(plan.validate().is_err());
        plan.hold = None;
        plan.target = Target::Reset;
        assert!(plan.validate().is_err());
    }
}

mod plan_codec {
    use rustaller::codec::{PlanCodecError, decode_plan, encode_plan};
    use rustaller::model::{BootMode, CURRENT_SCHEMA_VERSION, InstallPlan, PlanError, Target};

    #[test]
    fn plan_round_trip_preserves_all_modes_and_private_fields() {
        let mut plan = InstallPlan::new(Target::Debian);
        plan.username = Some("admin".into());
        plan.ssh_keys.push(crate::support::PUBLIC_KEY.into());
        plan.system.root_filesystem = rustaller::config::RootFilesystem::Ext4;
        plan.ssh_port = Some(2200);
        plan.frpc_config = Some("/root/private/frpc.toml".into());
        plan.target_disk = Some("/dev/vda".into());
        plan.post_install.hostname = Some("my-vps".into());
        plan.post_install.timezone = Some("Etc/UTC".into());
        plan.hold = Some(2);
        plan.force_boot_mode = Some(BootMode::Efi);
        plan.validate().expect("valid automatic plan");

        let decoded = decode_plan(&encode_plan(&plan).expect("encode valid plan"))
            .expect("decode valid plan");
        assert_eq!(decoded.schema_version, plan.schema_version);
        assert_eq!(decoded.target, plan.target);
        assert_eq!(decoded.username, plan.username);
        assert_eq!(decoded.ssh_port, plan.ssh_port);
        assert_eq!(
            decoded.system.root_filesystem,
            rustaller::config::RootFilesystem::Ext4
        );
        assert!(!format!("{decoded:?}").contains(crate::support::PUBLIC_KEY));
        assert_eq!(decoded.frpc_config, plan.frpc_config);
        assert_eq!(decoded.target_disk, plan.target_disk);
        assert_eq!(decoded.post_install, plan.post_install);
        assert_eq!(decoded.hold, plan.hold);
        assert_eq!(decoded.force_boot_mode, plan.force_boot_mode);
        assert_eq!(decoded.ssh_keys, plan.ssh_keys);
    }

    #[test]
    fn ssh_key_plan_round_trip_preserves_multiple_public_keys() {
        let mut plan = InstallPlan::new(Target::Arch);
        plan.ssh_keys = vec![
            crate::support::PUBLIC_KEY.into(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM2WUfRF3HJsHnADhPcb7X5IcdKGSvmDenGXHA0AkbUV second".into(),
        ];
        plan.validate().expect("valid SSH key plan");

        let decoded = decode_plan(&encode_plan(&plan).expect("encode SSH key plan"))
            .expect("decode SSH key plan");
        assert_eq!(decoded.ssh_keys, plan.ssh_keys);
    }

    #[test]
    fn decoder_rejects_bad_magic_truncation_trailing_data_and_invalid_plan() {
        assert!(matches!(
            decode_plan(b"not a plan"),
            Err(PlanCodecError::BadMagic)
        ));
        assert!(matches!(
            decode_plan(b"RINSTALL\0"),
            Err(PlanCodecError::Truncated)
        ));

        let mut encoded = encode_plan(&InstallPlan::new(Target::Reset)).expect("encode reset plan");
        encoded.push(0);
        assert!(matches!(
            decode_plan(&encoded),
            Err(PlanCodecError::TrailingData)
        ));

        let mut invalid = InstallPlan::new(Target::Alpine);
        invalid.ssh_port = Some(0);
        assert!(matches!(
            encode_plan(&invalid),
            Err(PlanCodecError::InvalidPlan(PlanError::InvalidPort("SSH")))
        ));
    }

    #[test]
    fn incompatible_schemas_fail_before_body_decoding() {
        for schema in (0..CURRENT_SCHEMA_VERSION).chain([CURRENT_SCHEMA_VERSION + 1]) {
            let mut bytes = b"RINSTALL\0".to_vec();
            bytes.extend_from_slice(&schema.to_le_bytes());
            assert_eq!(
                decode_plan(&bytes).unwrap_err(),
                PlanCodecError::InvalidPlan(PlanError::UnsupportedSchema(schema))
            );
        }
        let mut plan = InstallPlan::new(Target::Debian);
        plan.schema_version = 11;
        assert_eq!(
            encode_plan(&plan).unwrap_err(),
            PlanCodecError::InvalidPlan(PlanError::UnsupportedSchema(11))
        );
        assert!(
            decode_plan(b"RINSTALL\0\x0b\0")
                .unwrap_err()
                .to_string()
                .contains("Regenerate")
        );
    }

    #[test]
    fn only_resolved_public_keys_can_be_serialized() {
        let mut plan = InstallPlan::new(Target::Arch);
        for source in [
            "/root/id_ed25519.pub",
            "https://example.com/public-key",
            "ssh-ed25519 AAAA\nuser",
        ] {
            plan.ssh_keys = vec![source.into()];
            assert_eq!(
                encode_plan(&plan).unwrap_err(),
                PlanCodecError::UnresolvedSshKey
            );
        }
    }
}

mod plan_file {
    use rustaller::codec::encode_plan;
    use rustaller::model::{InstallPlan, Target};
    use rustaller::plan_file::PrivatePlanFile;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use crate::support::Scratch;

    #[test]
    fn private_plan_file_is_atomic_private_readable_and_removed_on_drop() {
        let scratch = Scratch::new("plan_file");
        let path = scratch.0.join("install.plan");
        let mut plan = InstallPlan::new(Target::Alpine);
        plan.ssh_keys.push(crate::support::PUBLIC_KEY.into());

        {
            let file = PrivatePlanFile::create(&path, &plan).expect("create private plan");
            assert_eq!(file.path(), path);
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let decoded = file.read().expect("read plan");
            assert_eq!(decoded.target, Target::Alpine);
            assert_eq!(decoded.ssh_keys, [crate::support::PUBLIC_KEY]);
        }
        assert!(
            !path.exists(),
            "plan should be removed when its guard drops"
        );
    }

    #[test]
    fn plan_file_can_be_explicitly_persisted_for_initrd_handoff() {
        let scratch = Scratch::new("plan_file");
        let path = scratch.0.join("install.plan");
        let plan = InstallPlan::new(Target::Alpine);
        let mut private_file = PrivatePlanFile::create(&path, &plan).expect("create plan");

        assert_eq!(private_file.persist(), path);
        drop(private_file);
        assert!(path.is_file());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            PrivatePlanFile::open(&path).unwrap().read().unwrap().target,
            Target::Alpine
        );
        assert!(!path.exists(), "opening persisted plan still consumes it");
    }

    #[test]
    fn opening_insecure_plan_does_not_delete_it_and_valid_open_consumes_it_on_drop() {
        let scratch = Scratch::new("plan_file");
        let path = scratch.0.join("handoff.plan");
        let plan = InstallPlan::new(Target::Reset);
        let encoded = encode_plan(&plan).unwrap();
        fs::write(&path, encoded).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(PrivatePlanFile::open(&path).is_err());
        assert!(
            path.exists(),
            "rejecting the file must not delete its owner’s data"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        {
            let file = PrivatePlanFile::open(&path).expect("open private plan");
            assert_eq!(file.read().unwrap().target, Target::Reset);
        }
        assert!(!path.exists(), "opened one-shot plan is consumed on drop");
    }

    #[test]
    fn refuses_to_replace_existing_plan_or_create_invalid_plan() {
        let scratch = Scratch::new("plan_file");
        let path = scratch.0.join("install.plan");
        fs::write(&path, b"preserve existing data").unwrap();
        let plan = InstallPlan::new(Target::Alpine);

        assert!(PrivatePlanFile::create(&path, &plan).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"preserve existing data");

        let invalid_path = scratch.0.join("invalid.plan");
        let mut invalid = InstallPlan::new(Target::Alpine);
        invalid.schema_version += 1;
        assert!(PrivatePlanFile::create(&invalid_path, &invalid).is_err());
        assert!(!invalid_path.exists());
    }
}

mod advanced_cli {
    use rustaller::codec::{decode_plan, encode_plan};

    use rustaller::host::args::{CliOutcome, parse_args};

    use rustaller::model::{InstallPlan, Target};

    #[test]
    fn advanced_cli_roundtrips_for_four_targets_and_rejects_incompatible_or_out_of_range_values() {
        for target in ["alpine", "debian", "arch", "gentoo"] {
            let CliOutcome::Plan(selected) = parse_args([
                target,
                "--tcp-congestion=bbr",
                "--qdisc=fq",
                "--mtu=1450",
                "--tcp-buffer-mib=16",
                "--somaxconn=8192",
                "--syn-backlog=8192",
                "--netdev-backlog=16384",
                "--tcp-mtu-probing=1",
                "--tcp-fastopen=3",
                "--tcp-syncookies=1",
                "--ipv6=enable",
                "--swapfile-mib=2048",
                "--swappiness=10",
            ])
            .unwrap() else {
                panic!("plan")
            };
            let decoded = decode_plan(&encode_plan(&selected).unwrap()).unwrap();
            assert_eq!(decoded.system, selected.system);
            assert_eq!(decoded.system.swap.size_mib, Some(2048));
            assert_eq!(decoded.system.network.mtu, Some(1450));
        }
        for options in [
            vec!["--mtu=1279"],
            vec!["--mtu=9001"],
            vec!["--tcp-buffer-mib=65"],
            vec!["--somaxconn=127"],
            vec!["--tcp-congestion=bbr"],
            vec!["--tcp-fastopen=4"],
            vec!["--tcp-mtu-probing=3"],
            vec!["--tcp-syncookies=2"],
            vec!["--swapfile-mib=0"],
            vec!["--swapfile-mib=65537"],
            vec!["--swappiness=10"],
            vec!["--swapfile-mib=1024", "--swappiness=201"],
            vec!["--ipv6=disable", "--dns=2606:4700:4700::1111"],
        ] {
            let mut args = vec!["debian"];
            args.extend(options);
            assert!(parse_args(args).is_err());
        }
        assert!(parse_args(["alpine", "--address-preference=ipv4"]).is_err());
        for target in ["iso", "reset", "netboot.xyz"] {
            assert!(parse_args([target, "--swapfile-mib=2048"]).is_err());
        }
        assert!(parse_args(["alpine", "--hold=1", "--ipv6=enable"]).is_err());
        assert!(InstallPlan::new(Target::Debian).system.is_default());
    }
}
