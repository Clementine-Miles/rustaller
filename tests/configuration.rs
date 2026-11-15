mod support;
mod postinstall {
    use rustaller::model::{InstallPlan, Target};
    use rustaller::postinstall::{KernelChoice, PostInstallConfig, PostInstallError};

    #[test]
    fn official_kernel_matrix_preserves_defaults_and_rejects_cross_distribution_choices() {
        for (target, choice, package) in [
            (Target::Alpine, KernelChoice::Auto, "linux-lts"),
            (Target::Alpine, KernelChoice::Cloud, "linux-virt"),
            (Target::Alpine, KernelChoice::Generic, "linux-lts"),
            (Target::Alpine, KernelChoice::Lts, "linux-lts"),
            (Target::Alpine, KernelChoice::Virt, "linux-virt"),
            (Target::Alpine, KernelChoice::Standard, "linux-lts"),
            (Target::Debian, KernelChoice::Standard, "linux-image-amd64"),
            (Target::Arch, KernelChoice::Cachyos, "linux-cachyos"),
            (Target::Debian, KernelChoice::Auto, "linux-image-amd64"),
            (
                Target::Debian,
                KernelChoice::Cloud,
                "linux-image-cloud-amd64",
            ),
            (Target::Debian, KernelChoice::Generic, "linux-image-amd64"),
            (Target::Arch, KernelChoice::Auto, "linux"),
            (Target::Arch, KernelChoice::Standard, "linux"),
            (Target::Arch, KernelChoice::Lts, "linux-lts"),
            (Target::Arch, KernelChoice::Hardened, "linux-hardened"),
            (Target::Arch, KernelChoice::Zen, "linux-zen"),
            (
                Target::Gentoo,
                KernelChoice::Auto,
                "sys-kernel/gentoo-kernel-bin",
            ),
            (
                Target::Gentoo,
                KernelChoice::Standard,
                "sys-kernel/gentoo-kernel-bin",
            ),
        ] {
            let config = PostInstallConfig {
                kernel: choice,
                ..Default::default()
            };
            config.validate(target, false, None).unwrap();
            assert_eq!(
                config.kernel_package(target, false, false).unwrap().package,
                package
            );
            for (other_target, other_choice) in [
                (Target::Debian, KernelChoice::Lts),
                (Target::Arch, KernelChoice::Cloud),
                (Target::Arch, KernelChoice::Generic),
                (Target::Gentoo, KernelChoice::Cloud),
                (Target::Gentoo, KernelChoice::Generic),
                (Target::Gentoo, KernelChoice::Lts),
                (Target::Alpine, KernelChoice::Hardened),
                (Target::Debian, KernelChoice::Zen),
                (Target::Gentoo, KernelChoice::Hardened),
            ] {
                assert!(
                    PostInstallConfig {
                        kernel: other_choice,
                        ..Default::default()
                    }
                    .validate(other_target, false, None)
                    .is_err()
                );
            }
        }
    }

    #[test]
    fn package_spelling_aliases_are_bound_to_their_distribution() {
        for (target, value, expected) in [
            (Target::Arch, "linux", KernelChoice::Standard),
            (Target::Arch, "linux-lts", KernelChoice::Lts),
            (Target::Arch, "linux-hardened", KernelChoice::Hardened),
            (Target::Arch, "linux-zen", KernelChoice::Zen),
            (Target::Debian, "linux-image-amd64", KernelChoice::Generic),
            (
                Target::Debian,
                "linux-image-cloud-amd64",
                KernelChoice::Cloud,
            ),
            (Target::Alpine, "linux-lts", KernelChoice::Lts),
            (Target::Alpine, "linux-virt", KernelChoice::Virt),
            (Target::Gentoo, "gentoo-kernel-bin", KernelChoice::Standard),
        ] {
            assert_eq!(
                KernelChoice::parse_for_target(value, target).unwrap(),
                expected
            );
        }
        for (target, value) in [
            (Target::Alpine, "linux"),
            (Target::Alpine, "linux-image-cloud-amd64"),
            (Target::Gentoo, "linux-image-amd64"),
            (Target::Debian, "linux-lts"),
            (Target::Arch, "gentoo-kernel-bin"),
        ] {
            assert!(KernelChoice::parse_for_target(value, target).is_err());
        }
    }

    #[test]
    fn additional_arch_kernels_round_trip_through_the_worker_protocol() {
        use rustaller::codec::{decode_plan, encode_plan};
        for kernel in [KernelChoice::Hardened, KernelChoice::Zen] {
            let mut plan = InstallPlan::new(Target::Arch);
            plan.post_install.kernel = kernel;
            assert_eq!(
                decode_plan(&encode_plan(&plan).unwrap())
                    .unwrap()
                    .post_install
                    .kernel,
                kernel
            );
        }
    }

    #[test]
    fn standard_defaults_are_predictable_on_physical_and_virtual_hosts() {
        let config = PostInstallConfig::default();
        assert_eq!(
            config
                .kernel_package(Target::Alpine, false, true)
                .unwrap()
                .alpine_flavor,
            Some("lts")
        );
        assert_eq!(
            config
                .kernel_package(Target::Alpine, true, true)
                .unwrap()
                .alpine_flavor,
            Some("lts")
        );
        assert_eq!(
            config
                .kernel_package(Target::Debian, true, true)
                .unwrap()
                .package,
            "linux-image-amd64"
        );
        let explicit = PostInstallConfig {
            kernel: KernelChoice::Generic,
            ..Default::default()
        };
        assert_eq!(
            explicit.validate(Target::Debian, true, None),
            Err(PostInstallError::ConflictingKernelOptions)
        );
    }

    #[test]
    fn configuration_data_uses_target_relative_paths_and_only_requested_settings() {
        let config = PostInstallConfig {
            kernel: KernelChoice::Lts,
            hostname: Some("my-vps-1".into()),
            timezone: Some("Etc/UTC".into()),
        };
        let prepared = config.prepare(Target::Arch, false, true).unwrap();
        assert_eq!(prepared.kernel.package, "linux-lts");
        assert_eq!(prepared.hostname_contents.as_deref(), Some("my-vps-1\n"));
        assert_eq!(prepared.timezone_contents.as_deref(), Some("Etc/UTC\n"));
        assert_eq!(
            prepared.zoneinfo_relative_path.as_deref(),
            Some("usr/share/zoneinfo/Etc/UTC")
        );
        assert_eq!(prepared.extra_packages, ["tzdata"]);
        let default = PostInstallConfig::default()
            .prepare(Target::Debian, false, true)
            .unwrap();
        assert!(default.hostname_contents.is_none());
        assert!(default.timezone_contents.is_none());
        assert!(default.zoneinfo_relative_path.is_none());
        assert!(default.extra_packages.is_empty());
        let gentoo = PostInstallConfig {
            timezone: Some("Asia/Shanghai".into()),
            ..Default::default()
        }
        .prepare(Target::Gentoo, false, true)
        .unwrap();
        assert_eq!(gentoo.extra_packages, ["sys-libs/timezone-data"]);
    }

    #[test]
    fn hostnames_and_timezone_paths_reject_injection_and_traversal() {
        for value in [
            "",
            "-vps",
            "vps-",
            "a.b",
            "white space",
            "host\nname",
            "$(id)",
            "../etc",
            "\u{4e2d}\u{6587}",
        ] {
            let config = PostInstallConfig {
                hostname: Some(value.into()),
                ..Default::default()
            };
            assert_eq!(
                config.validate(Target::Alpine, false, None),
                Err(PostInstallError::InvalidHostname),
                "accepted hostname {value:?}"
            );
        }
        let config = PostInstallConfig {
            hostname: Some("a".repeat(64)),
            ..Default::default()
        };
        assert!(config.validate(Target::Alpine, false, None).is_err());
        for value in [
            "",
            "/Etc/UTC",
            "../UTC",
            "Etc/../UTC",
            "Etc//UTC",
            "Etc/UTC/",
            "Etc/./UTC",
            "Etc/UTC\n",
            "Etc/UTC;id",
            "Etc\\UTC",
        ] {
            let config = PostInstallConfig {
                timezone: Some(value.into()),
                ..Default::default()
            };
            assert_eq!(
                config.validate(Target::Alpine, false, None),
                Err(PostInstallError::InvalidTimezone),
                "accepted timezone {value:?}"
            );
        }
        // Existence belongs to the target's installed tzdata, never the host's database.
        for value in [
            "UTC",
            "Etc/UTC",
            "Etc/GMT+8",
            "America/Argentina/Buenos_Aires",
            "Zone/NotInstalledYet",
        ] {
            let config = PostInstallConfig {
                timezone: Some(value.into()),
                ..Default::default()
            };
            config.validate(Target::Alpine, false, None).unwrap();
        }
    }

    #[test]
    fn rescue_and_manual_routes_reject_target_configuration() {
        for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
            let mut plan = InstallPlan::new(target);
            plan.hold = Some(1);
            plan.post_install.hostname = Some("vps".into());
            assert!(plan.validate().is_err());
        }
        {
            let target = Target::Reset;
            assert_eq!(
                PostInstallConfig {
                    hostname: Some("vps".into()),
                    ..Default::default()
                }
                .validate(target, false, None),
                Err(PostInstallError::UnexpectedRoute)
            );
            assert!(
                PostInstallConfig::default()
                    .prepare(target, false, false)
                    .is_err()
            );
        }
    }
}

mod system_config {
    use rustaller::codec::{PlanCodecError, decode_plan, encode_plan};
    use rustaller::config::{DnsConfig, DnsMode, InstallProfile, SystemConfig, SystemConfigError};
    use rustaller::host::args::{CliOutcome, parse_args};
    use rustaller::model::{BootMode, InstallPlan, PlanError, Target};
    use rustaller::targets::packages::{
        PRESERVED_ROOTS, apply_package_policy, arch_policy, cleanup_plan,
    };
    use rustaller::targets::{debian, packages as profile};
    use rustaller::worker::live::{ValidatedLivePlan, validate_live_plan_with_uid};
    use std::fs;
    use std::net::IpAddr;

    use crate::support::Scratch;
    fn live(plan: InstallPlan) -> ValidatedLivePlan {
        validate_live_plan_with_uid(plan, "x86_64", 2048, rustaller::RUNTIME_VERSION, 0).unwrap()
    }
    fn dns() -> DnsConfig {
        DnsConfig {
            mode: DnsMode::Static,
            servers: vec![
                "1.1.1.1".parse().unwrap(),
                "2606:4700:4700::1111".parse().unwrap(),
            ],
            search_domains: vec!["example.internal".into()],
        }
    }

    #[test]
    fn dns_requires_explicit_unicast_upstreams_and_bounded_unique_search_domains() {
        dns().validate().unwrap();
        for address in [
            "0.0.0.0",
            "127.0.0.1",
            "255.255.255.255",
            "224.0.0.1",
            "::",
            "::1",
            "ff02::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            let value = DnsConfig {
                servers: vec![address.parse().unwrap()],
                ..dns()
            };
            assert_eq!(
                value.validate(),
                Err(SystemConfigError::InvalidDnsServer),
                "{address}"
            );
        }
        let mut value = dns();
        value.servers = vec!["10.0.0.53".parse().unwrap(), "fd00::53".parse().unwrap()];
        value.validate().unwrap();
        value.servers = vec![
            "1.1.1.1".parse().unwrap(),
            "::ffff:1.1.1.1".parse().unwrap(),
        ];
        assert!(value.validate().is_err());
        value = dns();
        value.mode = DnsMode::Auto;
        assert_eq!(
            value.validate(),
            Err(SystemConfigError::AutomaticDnsWithValues)
        );
        value = dns();
        value.servers.clear();
        assert!(value.validate().is_err());
        value.servers = vec!["1.1.1.1".parse::<IpAddr>().unwrap(); 4];
        assert!(value.validate().is_err());
        for domain in [
            "",
            ".",
            "bad..domain",
            "../etc",
            "bad\nname",
            "-bad.example",
            "bad-.example",
            "example.com;id",
        ] {
            value = dns();
            value.search_domains = vec![domain.into()];
            assert!(value.validate().is_err());
        }
        value = dns();
        value.search_domains = vec!["Example.com".into(), "example.COM".into()];
        assert!(value.validate().is_err());
        value.search_domains = vec!["internal".into()];
        value.validate().unwrap();
        let auto = DnsConfig::default();
        assert!(auto.resolv_conf().unwrap().is_none());
        assert!(auto.networkd_settings().unwrap().is_none());
    }

    #[test]
    fn minimal_cli_options_roundtrip_and_old_plan_schemas_fail_closed() {
        let CliOutcome::Plan(plan) = parse_args([
            "debian",
            "--minimal",
            "--with-tools",
            "--keep-docs",
            "--keep-package-cache",
            "--dns",
            "1.1.1.1,2606:4700:4700::1111",
            "--dns-search",
            "example.internal",
            "--kernel",
            "cloud",
        ])
        .unwrap() else {
            panic!("plan")
        };
        assert_eq!(plan.system.profile, InstallProfile::Minimal);
        assert!(plan.system.with_tools);
        assert!(plan.system.keep_docs);
        assert!(plan.system.keep_package_cache);
        assert_eq!(plan.system.dns, dns());
        let decoded = decode_plan(&encode_plan(&plan).unwrap()).unwrap();
        assert_eq!(decoded.system, plan.system);
        assert_eq!(
            debian::apt_package_list_for_plan(&plan, BootMode::Bios, &[]).unwrap()[5],
            "linux-image-cloud-amd64"
        );
        for schema in [
            1_u16,
            2_u16,
            3_u16,
            4_u16,
            rustaller::model::CURRENT_SCHEMA_VERSION + 1,
        ] {
            let mut bytes = b"RINSTALL\0".to_vec();
            bytes.extend_from_slice(&schema.to_le_bytes());
            assert!(
                matches!(decode_plan(&bytes),Err(PlanCodecError::InvalidPlan(PlanError::UnsupportedSchema(value))) if value==schema)
            );
        }
        for args in [
            vec!["debian", "--dns-mode", "auto", "--dns", "1.1.1.1"],
            vec!["debian", "--dns-mode", "static"],
            vec!["debian", "--dns", "1.1.1.1:53"],
            vec!["debian", "--dns-search", "example.com"],
            vec!["debian", "--profile", "tiny"],
        ] {
            assert!(parse_args(args).is_err());
        }
    }

    #[test]
    fn defaults_preserve_existing_behavior_and_profiles_never_apply_to_rescue_or_media() {
        assert!(SystemConfig::default().is_default());
        for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
            let original = InstallPlan::new(target);
            original.validate().unwrap();
            assert!(!original.system.strip_docs());
            assert!(!original.system.clear_package_cache());
            let mut selected = InstallPlan::new(target);
            selected.system.profile = InstallProfile::Minimal;
            let packages = profile::package_plan(&selected, BootMode::Bios, true, &[]).unwrap();
            assert!(packages.strip_docs);
            assert!(packages.clear_package_cache);
            assert!(
                packages
                    .bootstrap_packages
                    .iter()
                    .chain(&packages.later_packages)
                    .any(|package| package.contains("openssh"))
            );
            selected.hold = Some(1);
            assert!(selected.validate().is_err());
        }
        {
            let target = Target::Reset;
            let system = SystemConfig {
                profile: InstallProfile::Minimal,
                ..Default::default()
            };
            assert!(system.validate(target, None).is_err());
            let system = SystemConfig {
                dns: dns(),
                ..Default::default()
            };
            assert!(system.validate(target, None).is_err());
        }
    }

    #[test]
    fn native_minimal_package_policies_preserve_boot_crypto_databases_and_licenses() {
        for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
            let scratch = Scratch::with_etc("system_config");
            scratch.write("etc/pacman.conf","[options]\nArchitecture = auto\n[core]\nServer = https://mirror.example/$repo/os/$arch\n");
            scratch.write("etc/portage/make.conf", "COMMON_FLAGS=\"-O2 -pipe\"\n");
            let mut selected = InstallPlan::new(target);
            selected.system.profile = InstallProfile::Minimal;
            let selected = live(selected);
            let cleanup = apply_package_policy(&scratch.0, &selected).unwrap();
            apply_package_policy(&scratch.0, &selected).unwrap();
            assert!(cleanup.documentation_roots.contains(&"usr/share/doc"));
            assert!(!cleanup.package_cache_roots.is_empty());
            for protected in PRESERVED_ROOTS {
                assert!(!cleanup.documentation_roots.contains(protected));
                assert!(!cleanup.package_cache_roots.contains(protected));
            }
            match target {
                Target::Debian => assert!(
                    fs::read_to_string(scratch.0.join("etc/dpkg/dpkg.cfg.d/99-rustaller-minimal"))
                        .unwrap()
                        .contains("path-include=/usr/share/doc/*/copyright")
                ),
                Target::Arch => {
                    let value = fs::read_to_string(scratch.0.join("etc/pacman.conf")).unwrap();
                    assert!(value.contains("SigLevel = Required DatabaseOptional"));
                    assert!(value.find("NoExtract").unwrap() < value.find("[core]").unwrap());
                    assert_eq!(
                        value.matches("# BEGIN rustaller minimal packages").count(),
                        1
                    );
                }
                Target::Gentoo => assert!(
                    fs::read_to_string(scratch.0.join("etc/portage/make.conf"))
                        .unwrap()
                        .contains("nodoc noman noinfo")
                ),
                _ => {}
            }
            // A cleanup plan never deletes existing data by itself.
            scratch.write("usr/share/doc/example/README", "still present\n");
            assert!(scratch.0.join("usr/share/doc/example/README").exists());
        }
        let mut selected = InstallPlan::new(Target::Debian);
        selected.system.profile = InstallProfile::Minimal;
        selected.system.keep_docs = true;
        selected.system.keep_package_cache = true;
        let cleanup = cleanup_plan(&live(selected)).unwrap();
        assert!(cleanup.documentation_roots.is_empty());
        assert!(cleanup.package_cache_roots.is_empty());
    }

    #[test]
    fn arch_minimal_policy_rejects_repository_signature_bypasses_and_replaces_global_legacy_policy()
    {
        assert!(arch_policy("[options]\n[core]\nSigLevel = Never\n", true).is_err());
        assert!(arch_policy("[core]\n", true).is_err());
        let value=arch_policy("[options]\nSigLevel = Never\nArchitecture = auto\n[core]\nServer = https://example.org/$repo/os/$arch\n",true).unwrap();
        assert!(!value.contains("SigLevel = Never"));
        assert!(value.contains("SigLevel = Required DatabaseOptional"));
        assert_eq!(arch_policy(&value, true).unwrap(), value);
    }

    #[test]
    fn mapped_ipv4_dns_is_rendered_as_ipv4_even_when_ipv6_is_disabled() {
        let mut selected = InstallPlan::new(Target::Debian);
        selected.system.network.ipv6 = rustaller::config::Ipv6Policy::Disable;
        selected.system.dns = DnsConfig {
            mode: DnsMode::Static,
            servers: vec!["::ffff:1.1.1.1".parse().unwrap()],
            search_domains: Vec::new(),
        };
        selected.validate().unwrap();
        let dns = &selected.system.dns;
        assert!(
            dns.resolv_conf()
                .unwrap()
                .unwrap()
                .contains("nameserver 1.1.1.1\n")
        );
        assert!(
            !dns.networkd_settings()
                .unwrap()
                .unwrap()
                .contains("::ffff:")
        );
    }
}

mod configure {
    use rustaller::model::{InstallPlan, Target};
    use rustaller::postinstall::PostInstallConfig;
    use rustaller::shared::configure::{ConfigurationError, apply_basic_configuration};
    use rustaller::worker::live::{ValidatedLivePlan, validate_live_plan_with_uid};
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::Path;

    use crate::support::Scratch;

    fn utc_zonefile() -> Vec<u8> {
        // A minimal TZif v1 UTC record: one ttinfo and a four-byte UTC name.
        let mut bytes = vec![0; 44];
        bytes[..4].copy_from_slice(b"TZif");
        bytes[36..40].copy_from_slice(&1_u32.to_be_bytes());
        bytes[40..44].copy_from_slice(&4_u32.to_be_bytes());
        bytes.extend_from_slice(&[0; 6]);
        bytes.extend_from_slice(b"UTC\0");
        bytes
    }

    #[test]
    fn installed_iana_zonefiles_with_real_transition_tables_are_accepted() {
        for (name, bytes) in [
            (
                "Etc/UTC",
                include_bytes!("fixtures/zoneinfo/UTC").as_slice(),
            ),
            (
                "America/New_York",
                include_bytes!("fixtures/zoneinfo/New_York").as_slice(),
            ),
        ] {
            let scratch = Scratch::with_etc("configure");
            scratch.zonefile(name, bytes);
            let configuration = PostInstallConfig {
                timezone: Some(name.into()),
                ..Default::default()
            };
            let plan = live_plan(Target::Debian, configuration, None);
            apply_basic_configuration(&scratch.0, &plan, true).unwrap();
            assert_eq!(fs::read(scratch.0.join("etc/localtime")).unwrap(), bytes);
        }
    }

    fn utc_zonefile_v2(version: u8) -> Vec<u8> {
        // UTC has no transitions, so its 32-bit and 64-bit data blocks are equal.
        let mut block = utc_zonefile();
        block[4] = version;
        let mut bytes = block.clone();
        bytes.extend_from_slice(&block);
        bytes.extend_from_slice(b"\nUTC0\n");
        bytes
    }

    fn live_plan(
        target: Target,
        configuration: PostInstallConfig,
        hold: Option<u8>,
    ) -> ValidatedLivePlan {
        let mut plan = InstallPlan::new(target);
        plan.post_install = configuration;
        plan.hold = hold;
        validate_live_plan_with_uid(plan, "x86_64", 2048, rustaller::RUNTIME_VERSION, 0).unwrap()
    }

    fn requested_configuration() -> PostInstallConfig {
        PostInstallConfig {
            hostname: Some("my-vps".into()),
            timezone: Some("UTC".into()),
            ..Default::default()
        }
    }

    #[test]
    fn requested_settings_replace_localtime_symlink_idempotently_without_following_it() {
        for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
            let scratch = Scratch::with_etc("configure");
            let bytes = utc_zonefile();
            scratch.zonefile("Etc/UTC", &bytes);
            symlink("Etc/UTC", scratch.0.join("usr/share/zoneinfo/UTC")).unwrap();
            let outside = scratch.0.join("outside-localtime");
            fs::write(&outside, b"do not overwrite").unwrap();
            fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
            symlink(&outside, scratch.0.join("etc/localtime")).unwrap();
            fs::write(scratch.0.join("etc/hosts"), b"preserve hosts\n").unwrap();
            let plan = live_plan(target, requested_configuration(), None);

            for _ in 0..2 {
                apply_basic_configuration(&scratch.0, &plan, true).unwrap();
                assert_eq!(
                    fs::read(scratch.0.join("etc/hostname")).unwrap(),
                    b"my-vps\n"
                );
                assert_eq!(fs::read(scratch.0.join("etc/timezone")).unwrap(), b"UTC\n");
                assert_eq!(fs::read(scratch.0.join("etc/localtime")).unwrap(), bytes);
                for name in ["hostname", "timezone", "localtime"] {
                    let metadata = fs::symlink_metadata(scratch.0.join("etc").join(name)).unwrap();
                    assert!(metadata.is_file());
                    assert_eq!(metadata.permissions().mode() & 0o777, 0o644);
                }
                assert_eq!(fs::read(&outside).unwrap(), b"do not overwrite");
                assert_eq!(
                    fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
                    0o600
                );
                assert_eq!(
                    fs::read(scratch.0.join("etc/hosts")).unwrap(),
                    b"preserve hosts\n"
                );
                assert_eq!(fs::read_dir(scratch.0.join("etc")).unwrap().count(), 4);
            }
        }
    }

    #[test]
    fn missing_malformed_and_oversized_zonefiles_prevent_hostname_publication() {
        for bytes in [
            None,
            Some(b"not a TZif file".to_vec()),
            Some(vec![b'x'; 1024 * 1024 + 1]),
        ] {
            let scratch = Scratch::with_etc("configure");
            fs::write(scratch.0.join("etc/hostname"), b"existing\n").unwrap();
            if let Some(bytes) = bytes {
                scratch.zonefile("UTC", &bytes);
            }
            let plan = live_plan(Target::Debian, requested_configuration(), None);
            assert!(apply_basic_configuration(&scratch.0, &plan, false).is_err());
            assert_eq!(
                fs::read(scratch.0.join("etc/hostname")).unwrap(),
                b"existing\n"
            );
            assert!(!scratch.0.join("etc/timezone").exists());
            assert!(!scratch.0.join("etc/localtime").exists());
        }
    }

    #[test]
    fn tzif_counts_versions_and_data_blocks_are_validated_before_any_write() {
        let valid = utc_zonefile();
        let mut bad_version = valid.clone();
        bad_version[4] = b'9';
        let mut zero_types = valid.clone();
        zero_types[36..40].copy_from_slice(&0_u32.to_be_bytes());
        let mut excessive_types = valid.clone();
        excessive_types[36..40].copy_from_slice(&257_u32.to_be_bytes());
        let mut zero_characters = valid.clone();
        zero_characters[40..44].copy_from_slice(&0_u32.to_be_bytes());
        let mut bad_standard_count = valid.clone();
        bad_standard_count[24..28].copy_from_slice(&2_u32.to_be_bytes());
        let mut bad_utc_count = valid.clone();
        bad_utc_count[20..24].copy_from_slice(&2_u32.to_be_bytes());
        let mut excessive_transitions = valid.clone();
        excessive_transitions[32..36].copy_from_slice(&u32::MAX.to_be_bytes());
        let mut excessive_leaps = valid.clone();
        excessive_leaps[28..32].copy_from_slice(&u32::MAX.to_be_bytes());
        let mut bad_dst = valid.clone();
        bad_dst[48] = 2;
        let mut bad_abbreviation = valid.clone();
        bad_abbreviation[49] = 4;
        let mut unterminated_abbreviation = valid.clone();
        *unterminated_abbreviation.last_mut().unwrap() = b'X';
        let mut bad_transition_type = valid.clone();
        bad_transition_type[32..36].copy_from_slice(&1_u32.to_be_bytes());
        let _ = bad_transition_type.splice(44..44, [0, 0, 0, 0, 1]);
        let mut missing_second_header = valid.clone();
        missing_second_header[4] = b'2';
        let mut mismatching_second_version = utc_zonefile_v2(b'2');
        mismatching_second_version[valid.len() + 4] = b'3';
        let v2 = utc_zonefile_v2(b'2');
        for bytes in [
            valid[..44].to_vec(),
            bad_version,
            zero_types,
            excessive_types,
            zero_characters,
            bad_standard_count,
            bad_utc_count,
            excessive_transitions,
            excessive_leaps,
            bad_dst,
            bad_abbreviation,
            unterminated_abbreviation,
            bad_transition_type,
            missing_second_header,
            mismatching_second_version,
            v2[..valid.len() * 2 - 1].to_vec(),
            v2[..valid.len() * 2].to_vec(),
        ] {
            let scratch = Scratch::with_etc("configure");
            scratch.zonefile("UTC", &bytes);
            let plan = live_plan(Target::Debian, requested_configuration(), None);
            assert!(matches!(
                apply_basic_configuration(&scratch.0, &plan, false),
                Err(ConfigurationError::InvalidZonefile)
            ));
            assert_eq!(fs::read_dir(scratch.0.join("etc")).unwrap().count(), 0);
        }
    }

    #[test]
    fn supported_tzif_versions_with_complete_data_can_be_applied() {
        for bytes in [
            utc_zonefile(),
            utc_zonefile_v2(b'2'),
            utc_zonefile_v2(b'3'),
            utc_zonefile_v2(b'4'),
        ] {
            let scratch = Scratch::with_etc("configure");
            scratch.zonefile("UTC", &bytes);
            let plan = live_plan(Target::Debian, requested_configuration(), None);
            apply_basic_configuration(&scratch.0, &plan, false).unwrap();
            assert_eq!(fs::read(scratch.0.join("etc/localtime")).unwrap(), bytes);
        }
    }

    #[test]
    fn escaped_timezone_symlink_is_rejected_before_any_configuration_write() {
        let scratch = Scratch::with_etc("configure");
        fs::create_dir_all(scratch.0.join("usr/share/zoneinfo")).unwrap();
        let outside = scratch.0.join("outside-zone");
        fs::write(&outside, utc_zonefile()).unwrap();
        symlink(&outside, scratch.0.join("usr/share/zoneinfo/UTC")).unwrap();
        let plan = live_plan(Target::Debian, requested_configuration(), None);
        assert!(matches!(
            apply_basic_configuration(&scratch.0, &plan, false),
            Err(ConfigurationError::ZonefileOutsideTarget)
        ));
        assert_eq!(fs::read_dir(scratch.0.join("etc")).unwrap().count(), 0);
    }

    #[test]
    fn root_and_etc_symlinks_and_root_ancestors_are_rejected() {
        let scratch = Scratch::with_etc("configure");
        let plan = live_plan(
            Target::Debian,
            PostInstallConfig {
                hostname: Some("my-vps".into()),
                ..Default::default()
            },
            None,
        );
        let real_root = scratch.0.join("real-root");
        fs::create_dir_all(real_root.join("etc")).unwrap();
        let linked_root = scratch.0.join("linked-root");
        symlink(&real_root, &linked_root).unwrap();
        assert!(matches!(
            apply_basic_configuration(&linked_root, &plan, false),
            Err(ConfigurationError::UnsafePath)
        ));
        fs::create_dir_all(real_root.join("child/etc")).unwrap();
        assert!(matches!(
            apply_basic_configuration(&linked_root.join("child"), &plan, false),
            Err(ConfigurationError::UnsafePath)
        ));
        fs::remove_dir(scratch.0.join("etc")).unwrap();
        symlink(real_root.join("etc"), scratch.0.join("etc")).unwrap();
        assert!(matches!(
            apply_basic_configuration(&scratch.0, &plan, false),
            Err(ConfigurationError::UnsafePath)
        ));
        assert!(!real_root.join("etc/hostname").exists());
    }

    #[test]
    fn invalid_destination_is_detected_before_hostname_write() {
        let scratch = Scratch::with_etc("configure");
        scratch.zonefile("UTC", &utc_zonefile());
        fs::create_dir(scratch.0.join("etc/localtime")).unwrap();
        let plan = live_plan(Target::Debian, requested_configuration(), None);
        assert!(matches!(
            apply_basic_configuration(&scratch.0, &plan, false),
            Err(ConfigurationError::UnsafePath)
        ));
        assert!(!scratch.0.join("etc/hostname").exists());
        assert!(!scratch.0.join("etc/timezone").exists());
    }

    #[test]
    fn host_root_relative_paths_and_rescue_are_rejected_and_defaults_write_nothing() {
        let plan = live_plan(Target::Alpine, PostInstallConfig::default(), None);
        for root in [
            Path::new("/"),
            Path::new("relative/root"),
            Path::new("/tmp/../etc"),
        ] {
            assert!(matches!(
                apply_basic_configuration(root, &plan, true),
                Err(ConfigurationError::InvalidRoot)
            ));
        }
        let scratch = Scratch::with_etc("configure");
        let rescue = live_plan(Target::Alpine, PostInstallConfig::default(), Some(1));
        assert!(matches!(
            apply_basic_configuration(&scratch.0, &rescue, true),
            Err(ConfigurationError::RescuePlan)
        ));
        apply_basic_configuration(&scratch.0, &plan, true).unwrap();
        assert_eq!(fs::read_dir(scratch.0.join("etc")).unwrap().count(), 0);
    }
}
