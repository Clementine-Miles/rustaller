use rustaller::codec::{PlanCodecError, decode_plan, encode_plan};
use rustaller::config::{GentooInit, SystemConfigError};
use rustaller::host::args::{CliOutcome, parse_args};
use rustaller::model::{CURRENT_SCHEMA_VERSION, InstallPlan, PlanError, Target};

fn plan(args: &[&str]) -> Box<InstallPlan> {
    match parse_args(args).unwrap() {
        CliOutcome::Plan(plan) | CliOutcome::Inspect(plan) => plan,
        _ => panic!("expected plan"),
    }
}

#[test]
fn gentoo_new_plan_defaults_to_openrc_and_roundtrips_each_explicit_variant() {
    assert_eq!(
        InstallPlan::new(Target::Gentoo).system.gentoo_init,
        Some(GentooInit::Openrc)
    );
    assert_eq!(
        plan(&["gentoo"]).system.gentoo_init,
        Some(GentooInit::Openrc)
    );
    for init in [GentooInit::Openrc, GentooInit::Systemd] {
        let option = format!("--gentoo-init={}", init.as_str());
        let original = plan(&["gentoo", &option]);
        let decoded = decode_plan(&encode_plan(&original).unwrap()).unwrap();
        assert_eq!(decoded.system.gentoo_init, Some(init));
        assert_eq!(decoded.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            init.stage3_variant(),
            format!("amd64-nomultilib-{}", init.as_str())
        );
    }
}

#[test]
fn gentoo_init_is_target_specific_and_a_missing_serialized_selection_fails_closed() {
    for target in ["alpine", "debian", "arch", "reset"] {
        for option in ["--gentoo-init=openrc", "--gentoo-init=systemd"] {
            assert!(parse_args([target, option]).is_err());
        }
    }
    for invalid in ["multilib", "musl", "amd64", "", "OpenRC"] {
        assert!(parse_args(["gentoo", &format!("--gentoo-init={invalid}")]).is_err());
    }
    let mut missing = InstallPlan::new(Target::Gentoo);
    missing.system.gentoo_init = None;
    assert_eq!(
        decode_plan(&encode_plan(&missing).unwrap()).unwrap_err(),
        PlanCodecError::InvalidPlan(PlanError::SystemConfig(
            SystemConfigError::MissingGentooInit
        ))
    );
}

#[test]
fn prior_systemd_only_schema_is_never_reinterpreted_as_openrc() {
    let mut bytes = encode_plan(&InstallPlan::new(Target::Gentoo)).unwrap();
    bytes[9..11].copy_from_slice(&8_u16.to_le_bytes());
    assert_eq!(
        decode_plan(&bytes).unwrap_err(),
        PlanCodecError::InvalidPlan(PlanError::UnsupportedSchema(8))
    );
}
