use rustaller::command::{CommandOutput, MockRunner};
use rustaller::host::sources::{
    StableRelease, fetch_stable_release, parse_alpine_release_metadata,
    parse_debian_release_metadata,
};
use rustaller::model::Target;

#[test]
fn parses_official_stable_metadata_fixtures() {
    let alpine = include_str!("fixtures/alpine-latest-releases.yaml");
    assert_eq!(parse_alpine_release_metadata(alpine).unwrap(), "3.24");

    let debian = include_str!("fixtures/debian-stable-Release");
    assert_eq!(
        parse_debian_release_metadata(debian).unwrap(),
        StableRelease::Debian {
            version: "13".into(),
            codename: "trixie".into(),
        }
    );
}

#[test]
fn rejects_missing_ambiguous_or_malformed_stable_metadata() {
    assert!(parse_alpine_release_metadata("version: nope\n").is_err());
    assert!(parse_alpine_release_metadata("version: 3.24.1\nversion: 3.25.1\n").is_err());
    assert!(parse_debian_release_metadata("Version: 13.7\n").is_err());
    assert!(parse_debian_release_metadata("Version: x\nCodename: Trixie\n").is_err());
}

#[test]
fn stable_fetch_uses_existing_curl_command_and_arguments() {
    let body = include_str!("fixtures/debian-stable-Release")
        .as_bytes()
        .to_vec();
    let mut runner = MockRunner::new([CommandOutput::new(Some(0), body, Vec::new())]);
    let release = fetch_stable_release(&mut runner, Target::Debian, "x86_64").unwrap();

    assert_eq!(
        release,
        StableRelease::Debian {
            version: "13".into(),
            codename: "trixie".into(),
        }
    );
    let calls = runner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "curl");
    assert_eq!(
        calls[0].args,
        [
            "--connect-timeout",
            "10",
            "-f",
            "-L",
            "https://deb.debian.org/debian/dists/stable/Release",
        ]
    );
}

#[test]
fn metadata_fetch_fails_closed_on_http_error() {
    let mut runner = MockRunner::new([CommandOutput::new(Some(22), Vec::new(), b"404".to_vec())]);
    assert!(fetch_stable_release(&mut runner, Target::Alpine, "x86_64").is_err());
}

#[test]
fn non_stable_targets_do_not_have_release_metadata_routes() {
    let mut runner = MockRunner::empty();
    assert!(fetch_stable_release(&mut runner, Target::Arch, "x86_64").is_err());
    assert!(runner.calls().is_empty());
}
