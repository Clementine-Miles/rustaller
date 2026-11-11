use std::process::Command;

#[test]
fn worker_help_contains_the_runtime_version_marker() {
    let output = Command::new(env!("CARGO_BIN_EXE_rustaller-worker"))
        .arg("--help")
        .output()
        .expect("run worker help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(rustaller::RUNTIME_VERSION));
    assert!(stdout.contains("firstboot-fix"));
}
