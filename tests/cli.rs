use std::process::Command;

#[test]
fn help_is_available_without_host_preflight_or_network_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_rustaller"))
        .arg("--help")
        .output()
        .expect("run rustaller help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage: rustaller [OPTIONS] TARGET"));
    assert!(stdout.contains("Linux x86_64 hosts only"));
}

#[test]
fn noninteractive_empty_invocation_fails_closed_with_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_rustaller"))
        .output()
        .expect("run rustaller without a target");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("Usage: rustaller")
    );
}
