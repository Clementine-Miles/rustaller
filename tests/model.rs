use rustaller::model::{
    BootMode, CURRENT_SCHEMA_VERSION, InstallPlan, IsoOptions, SecretString, Target,
    validate_host_arch, validate_ram,
};

#[test]
fn target_ram_floors_match_installer_contract() {
    assert_eq!(Target::Alpine.minimum_ram_mb(), 512);
    assert_eq!(Target::Debian.minimum_ram_mb(), 512);
    assert_eq!(Target::Arch.minimum_ram_mb(), 1024);
    assert_eq!(Target::Gentoo.minimum_ram_mb(), 2048);
    assert_eq!(Target::LocalIso.minimum_ram_mb(), 0);
    assert_eq!(Target::NetbootXyz.minimum_ram_mb(), 0);
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
fn automatic_install_uses_embedded_assets_and_validates_optional_resource_base_and_schema() {
    let mut plan = InstallPlan::new(Target::Debian);
    assert!(plan.validate().is_ok());

    plan.confhome = Some("https://assets.example/reinstall/v1".into());
    assert!(plan.validate().is_ok());
    assert_eq!(plan.schema_version, CURRENT_SCHEMA_VERSION);

    plan.schema_version += 1;
    assert!(plan.validate().is_err());
}

#[test]
fn automatic_asset_roots_reject_untrimmed_upstream_and_unsafe_url_syntax() {
    let mut plan = InstallPlan::new(Target::Alpine);
    for confhome in [
        "https://",
        "http://assets.example/reinstall",
        "https://assets.example/reinstall?token=secret",
        "https://assets.example/reinstall#fragment",
        "https://assets.example/reinstall;command",
        "https://raw.githubusercontent.com/bin456789/reinstall/main",
        "https://cnb.cool/bin456789/reinstall/main",
    ] {
        plan.confhome = Some(confhome.to_owned());
        assert!(plan.validate().is_err(), "accepted {confhome}");
    }
    plan.confhome = Some("https://assets.example/reinstall/v1/".into());
    assert!(plan.validate().is_ok());
}

#[test]
fn automatic_credentials_are_mutually_exclusive_and_password_debug_is_redacted() {
    let mut plan = InstallPlan::new(Target::Alpine);
    plan.confhome = Some("https://assets.example/reinstall/v1".into());
    plan.password = Some(SecretString::new("never-print-this"));
    assert!(!format!("{plan:?}").contains("never-print-this"));
    assert!(plan.validate().is_ok());

    plan.ssh_keys
        .push("ssh-ed25519 AAAATEST user@example".into());
    assert!(plan.validate().is_err());
}

#[test]
fn account_and_password_inputs_reject_argument_and_line_injection_before_handoff() {
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
    for password in [
        "",
        "secret\nroot:changed",
        "secret\0value",
        "secret\rvalue",
        "secret\u{1b}value",
    ] {
        plan.password = Some(SecretString::new(password));
        assert!(plan.validate().is_err());
    }
    plan.password = Some(SecretString::new("  exact password  "));
    assert!(plan.validate().is_ok());
}

#[test]
fn local_iso_requires_iso_and_rejects_automatic_only_options() {
    let mut plan = InstallPlan::new(Target::LocalIso);
    assert!(plan.validate().is_err());

    plan.iso = Some(IsoOptions {
        path: "/images/live.iso".into(),
        config: Some("/boot/grub/loopback.cfg".into()),
        kernel: None,
        initrd: None,
        cmdline: None,
    });
    assert!(plan.validate().is_ok());

    plan.ssh_port = Some(2222);
    assert!(plan.validate().is_err());
}

#[test]
fn netboot_and_reset_reject_automatic_install_fields() {
    let mut netboot = InstallPlan::new(Target::NetbootXyz);
    netboot.target_disk = Some("/dev/vda".into());
    assert!(netboot.validate().is_err());

    let mut reset = InstallPlan::new(Target::Reset);
    reset.hold = Some(1);
    assert!(reset.validate().is_ok());
}

#[test]
fn boot_mode_has_only_bios_and_efi_variants() {
    assert_eq!(BootMode::Bios.as_str(), "bios");
    assert_eq!(BootMode::Efi.as_str(), "efi");
}
