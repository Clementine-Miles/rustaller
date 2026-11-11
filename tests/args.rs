use rustaller::host::args::{CliOutcome, parse_args};
use rustaller::model::{BootMode, Target};
use rustaller::postinstall::KernelChoice;

fn parse(args: &[&str]) -> CliOutcome {
    parse_args(args.iter().copied(), None).expect("valid CLI arguments")
}

#[test]
fn parses_target_and_current_option_aliases() {
    let CliOutcome::Plan(plan) = parse(&[
        "--user=admin",
        "--passwd",
        "secret",
        "--ssh-port",
        "2222",
        "--http-port=8080",
        "--no-cloud-kernel",
        "--sleep",
        "2",
        "--force-boot-mode",
        "efi",
        "--confhome",
        "https://assets.example/reinstall",
        "debian",
    ]) else {
        panic!("expected install plan");
    };

    assert_eq!(plan.target, Target::Debian);
    assert_eq!(plan.username.as_deref(), Some("admin"));
    assert_eq!(
        plan.password.as_ref().map(|value| value.expose()),
        Some("secret")
    );
    assert_eq!(plan.ssh_port, Some(2222));
    assert_eq!(plan.web_port, Some(8080));
    assert!(plan.no_cloud_kernel);
    assert_eq!(plan.hold, Some(2));
    assert_eq!(plan.force_boot_mode, Some(BootMode::Efi));
}

#[test]
fn help_short_circuits_invalid_options_before_host_checks() {
    assert!(matches!(
        parse_args(["--nonsense", "--help"], None),
        Ok(CliOutcome::Help)
    ));
    assert!(matches!(parse_args(["-h"], None), Ok(CliOutcome::Help)));
}

#[test]
fn uses_optional_environment_resource_root_and_embeds_default_rust_assets() {
    let CliOutcome::Plan(plan) = parse_args(["alpine"], Some("https://assets.example/reinstall"))
        .expect("environment URL accepted")
    else {
        panic!("expected plan");
    };
    assert_eq!(
        plan.confhome.as_deref(),
        Some("https://assets.example/reinstall")
    );
    assert!(matches!(
        parse_args(["alpine"], None),
        Ok(CliOutcome::Plan(_))
    ));
}

#[test]
fn separates_read_only_inspection_and_explicit_disk_confirmation_from_the_install_plan() {
    let CliOutcome::Inspect(plan) =
        parse_args(["debian", "--inspect", "--target-disk=/dev/vda"], None).unwrap()
    else {
        panic!("inspection")
    };
    assert_eq!(plan.target, Target::Debian);
    let token = "ERASE /dev/vda 8589934592 SERIAL=vm WWN= PTUUID=12345678";
    let CliOutcome::ConfirmedPlan { plan, confirmation } = parse_args(
        [
            "alpine",
            "--confirm-erase",
            token,
            "--ssh-key=/root/client.pub",
        ],
        None,
    )
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
        assert!(parse_args(args, None).is_err());
    }
    assert!(matches!(
        parse_args(["alpine", "--hold=1", "--inspect"], None),
        Ok(CliOutcome::Inspect(_))
    ));
    assert!(matches!(
        parse_args(
            [
                "alpine",
                "--hold=1",
                "--confirm-handoff",
                "RESCUE disk identity"
            ],
            None
        ),
        Ok(CliOutcome::ConfirmedPlan { .. })
    ));
    // An option-looking credential is its value, never an execution flag.
    let CliOutcome::Plan(plan) = parse_args(["debian", "--password", "--inspect"], None).unwrap()
    else {
        panic!("literal credential")
    };
    assert_eq!(plan.password.as_ref().unwrap().expose(), "--inspect");
}

#[test]
fn local_iso_options_are_kept_separate_from_automatic_targets() {
    let CliOutcome::Plan(plan) = parse(&[
        "iso",
        "--iso",
        "/images/live.iso",
        "--iso-kernel",
        "/live/vmlinuz",
        "--iso-initrd",
        "/live/initrd.img",
        "--iso-cmdline",
        "boot=live findiso=%ISO_PATH%",
    ]) else {
        panic!("expected plan");
    };
    assert_eq!(plan.target, Target::LocalIso);
    assert_eq!(plan.iso.as_ref().unwrap().path, "/images/live.iso");
    assert!(
        parse_args(
            ["iso", "--iso", "/images/live.iso", "--ssh-port", "22"],
            None
        )
        .is_err()
    );
}

#[test]
fn rejects_missing_target_unknown_options_and_conflicting_credentials() {
    assert!(parse_args::<_, &str>([], None).is_err());
    assert!(
        parse_args(
            ["--unknown", "alpine"],
            Some("https://assets.example/reinstall")
        )
        .is_err()
    );
    assert!(
        parse_args(
            [
                "alpine",
                "--confhome",
                "https://assets.example/reinstall",
                "--password",
                "x",
                "--ssh-key",
                "ssh-ed25519 AAAA"
            ],
            None
        )
        .is_err()
    );
}

#[test]
fn target_configuration_uses_shared_validation_and_official_kernel_choices() {
    let env = Some("https://assets.example/reinstall");
    let CliOutcome::Plan(plan) = parse_args(
        [
            "arch",
            "--kernel=lts",
            "--hostname",
            "my-vps",
            "--timezone",
            "Etc/UTC",
        ],
        env,
    )
    .unwrap() else {
        panic!("plan")
    };
    assert_eq!(plan.post_install.kernel, KernelChoice::Lts);
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
        assert!(parse_args(&args, env).is_err(), "accepted {args:?}");
    }
    assert!(parse_args(["debian", "--kernel", "auto", "--no-cloud-kernel"], env).is_ok());
}
