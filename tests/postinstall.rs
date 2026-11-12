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
        "中文",
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
        plan.confhome = Some("https://assets.example/reinstall".into());
        plan.hold = Some(1);
        plan.post_install.hostname = Some("vps".into());
        assert!(plan.validate().is_err());
    }
    for target in [Target::LocalIso, Target::NetbootXyz, Target::Reset] {
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
