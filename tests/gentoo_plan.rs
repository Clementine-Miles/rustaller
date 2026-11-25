use rustaller::codec::{decode_plan, encode_plan};
use rustaller::host::args::{CliOutcome, parse_args};
use rustaller::model::{CURRENT_SCHEMA_VERSION, Target};

#[test]
fn gentoo_init_is_fixed_to_openrc_with_no_cli_override() {
    let CliOutcome::Plan(plan) = parse_args(["gentoo"]).unwrap() else {
        panic!("expected Gentoo plan");
    };
    let decoded = decode_plan(&encode_plan(&plan).unwrap()).unwrap();
    assert_eq!(decoded.target, Target::Gentoo);
    assert_eq!(decoded.target.init_system(), Some("OpenRC"));
    assert_eq!(decoded.schema_version, CURRENT_SCHEMA_VERSION);
    for target in ["gentoo", "alpine", "debian", "arch", "reset"] {
        for option in ["--gentoo-init=openrc", "--gentoo-init=systemd"] {
            assert!(parse_args([target, option]).is_err());
        }
    }
}
