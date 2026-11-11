use reinstall_rs::codec::{PlanCodecError, decode_plan, encode_plan};
use reinstall_rs::model::{
    BootMode, CURRENT_SCHEMA_VERSION, InstallPlan, IsoOptions, PlanError, SecretString, Target,
};
use reinstall_rs::postinstall::KernelChoice;

#[test]
fn plan_round_trip_preserves_all_modes_and_private_fields() {
    let mut plan = InstallPlan::new(Target::Debian);
    plan.username = Some("admin".into());
    plan.password = Some(SecretString::new("p@ssword"));
    plan.ssh_port = Some(2200);
    plan.web_port = Some(8080);
    plan.frpc_config = Some("/root/private/frpc.toml".into());
    plan.target_disk = Some("/dev/vda".into());
    plan.no_cloud_kernel = true;
    plan.post_install.hostname = Some("my-vps".into());
    plan.post_install.timezone = Some("Etc/UTC".into());
    plan.hold = Some(2);
    plan.force_boot_mode = Some(BootMode::Efi);
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.validate().expect("valid automatic plan");

    let decoded =
        decode_plan(&encode_plan(&plan).expect("encode valid plan")).expect("decode valid plan");
    assert_eq!(decoded.schema_version, plan.schema_version);
    assert_eq!(decoded.target, plan.target);
    assert_eq!(decoded.username, plan.username);
    assert_eq!(
        decoded.password.as_ref().map(SecretString::expose),
        Some("p@ssword")
    );
    assert_eq!(decoded.ssh_port, plan.ssh_port);
    assert_eq!(decoded.web_port, plan.web_port);
    assert_eq!(decoded.frpc_config, plan.frpc_config);
    assert_eq!(decoded.target_disk, plan.target_disk);
    assert_eq!(decoded.no_cloud_kernel, plan.no_cloud_kernel);
    assert_eq!(decoded.post_install, plan.post_install);
    assert_eq!(decoded.hold, plan.hold);
    assert_eq!(decoded.force_boot_mode, plan.force_boot_mode);
    assert_eq!(decoded.confhome, plan.confhome);
    assert_eq!(decoded.ssh_keys, plan.ssh_keys);
}

#[test]
fn ssh_key_plan_round_trip_preserves_multiple_public_keys() {
    let mut plan = InstallPlan::new(Target::Arch);
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.ssh_keys = vec![
        "ssh-ed25519 AAAATEST first@host".into(),
        "ssh-rsa AAAATEST second@host".into(),
    ];
    plan.validate().expect("valid SSH key plan");

    let decoded = decode_plan(&encode_plan(&plan).expect("encode SSH key plan"))
        .expect("decode SSH key plan");
    assert_eq!(decoded.ssh_keys, plan.ssh_keys);
}

#[test]
fn local_iso_plan_round_trip_preserves_boot_options() {
    let mut plan = InstallPlan::new(Target::LocalIso);
    plan.iso = Some(IsoOptions {
        path: "/images/rescue.iso".into(),
        config: None,
        kernel: Some("/live/vmlinuz".into()),
        initrd: Some("/live/initrd.img".into()),
        cmdline: Some("boot=live findiso=%ISO_PATH%".into()),
    });
    plan.force_boot_mode = None;
    plan.validate().expect("valid local ISO plan");

    let decoded =
        decode_plan(&encode_plan(&plan).expect("encode ISO plan")).expect("decode ISO plan");
    assert_eq!(decoded.target, Target::LocalIso);
    assert_eq!(decoded.iso, plan.iso);
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
    invalid.confhome = Some("http://insecure.example".into());
    assert!(matches!(
        decode_plan(&encode_plan(&invalid).expect("encode invalid plan")),
        Err(PlanCodecError::InvalidPlan(_))
    ));
}

#[test]
fn kernel_choices_round_trip_and_unsupported_schemas_fail_before_body_decoding() {
    for (target, kernel) in [
        (Target::Alpine, KernelChoice::Cloud),
        (Target::Debian, KernelChoice::Generic),
        (Target::Arch, KernelChoice::Lts),
        (Target::Gentoo, KernelChoice::Standard),
    ] {
        let mut plan = InstallPlan::new(target);
        plan.confhome = Some("https://assets.example/reinstall".into());
        plan.post_install.kernel = kernel;
        assert_eq!(
            decode_plan(&encode_plan(&plan).unwrap())
                .unwrap()
                .post_install,
            plan.post_install
        );
    }
    for schema in [1, 2, 3, 4, CURRENT_SCHEMA_VERSION + 1] {
        let mut bytes = b"RINSTALL\0".to_vec();
        bytes.extend_from_slice(&schema.to_le_bytes());
        assert!(
            matches!(decode_plan(&bytes), Err(PlanCodecError::InvalidPlan(PlanError::UnsupportedSchema(value))) if value == schema)
        );
    }
    let mut bytes = encode_plan(&InstallPlan::new(Target::Reset)).unwrap();
    let kernel_index = bytes.len() - 3;
    bytes[kernel_index] = 255;
    assert!(matches!(
        decode_plan(&bytes),
        Err(PlanCodecError::InvalidTag)
    ));
}
