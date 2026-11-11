use rustaller::command::{
    CommandError, CommandInput, CommandOutput, CommandRunner, CommandSpec, MockRunner, OutputMode,
    SwapSafeRunner, SystemRunner, run_checked,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn streamed_output_inherits_the_parent_while_capture_stays_private() {
    const PROBE: &str = "RUSTALLER_STREAM_TEST_CHILD";
    if std::env::var_os(PROBE).is_some() {
        let spec = CommandSpec::new(
            "sh",
            ["-c", "printf stream-sentinel; printf capture-sentinel >&2"],
        )
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Capture);
        let output = SystemRunner.run(&spec).unwrap();
        assert_eq!(output.status, Some(0));
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"capture-sentinel");
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "streamed_output_inherits_the_parent_while_capture_stays_private",
            "--nocapture",
        ])
        .env(PROBE, "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("stream-sentinel"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("capture-sentinel"));
    assert!(output.stderr.is_empty());
}

#[test]
fn captured_pipeline_drains_output_while_writing_more_than_pipe_capacity() {
    let bytes = vec![b'x'; 2 * 1024 * 1024];
    let first = CommandSpec::new("cat", [] as [&str; 0])
        .stdin(CommandInput::Bytes(bytes.clone()))
        .stderr_mode(OutputMode::Discard);
    let last = CommandSpec::new("cat", [] as [&str; 0])
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Discard);
    let output = SystemRunner.run_pipeline(&[first, last]).unwrap();
    assert_eq!(output.statuses, [Some(0), Some(0)]);
    assert_eq!(output.stdout, bytes);
}

#[test]
fn mock_runner_records_program_args_and_phase_in_order() {
    let mut runner = MockRunner::new([
        CommandOutput::new(Some(0), b"first\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(7), Vec::new(), b"failed\n".to_vec()),
    ]);
    let first = CommandSpec::new("apk", ["add", "alpine-base"]).phase("alpine-rootfs");
    let second =
        CommandSpec::new("debootstrap", ["--variant=minbase", "stable"]).phase("debian-rootfs");

    let first_output = runner.run(&first).expect("first mocked command");
    let second_output = runner.run(&second).expect("second mocked command");

    assert_eq!(first_output.status, Some(0));
    assert_eq!(first_output.stdout, b"first\n");
    assert_eq!(second_output.status, Some(7));
    assert_eq!(second_output.stderr, b"failed\n");
    assert_eq!(runner.calls(), &[first, second]);
}

#[test]
fn command_debug_never_prints_arguments_or_environment_values() {
    let command = CommandSpec::new("curl", ["--header", "Authorization: Bearer secret-token"])
        .phase("fetch")
        .env("ACCESS_TOKEN", "another-secret");
    let debug = format!("{command:?}");
    assert!(!debug.contains("secret-token"));
    assert!(!debug.contains("another-secret"));
    assert!(debug.contains("curl"));
    assert!(debug.contains("ACCESS_TOKEN"));
}

#[test]
fn system_runner_captures_child_output_stdin_and_exit_status() {
    let (_scratch, binary) = build_mock_command();
    let command = CommandSpec::new(&binary, ["argument with spaces"])
        .phase("runner-test")
        .env("MOCK_EXIT_CODE", "7")
        .stdin(CommandInput::Bytes(b"stdin-data".to_vec()))
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Capture);

    let output = SystemRunner.run(&command).expect("spawn mock executable");
    assert_eq!(output.status, Some(7));
    assert_eq!(output.stdout, b"out:argument with spaces:stdin-data\n");
    assert_eq!(output.stderr, b"diagnostic\n");
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn build_mock_command() -> (Scratch, PathBuf) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let scratch =
        std::env::temp_dir().join(format!("rustaller-mock-{}-{nonce}", std::process::id()));
    fs::create_dir(&scratch).expect("create unique mock directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/mock-command.rs");
    let binary = scratch.join("mock-command");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = Command::new(rustc)
        .arg("--edition=2024")
        .arg(source)
        .arg("-o")
        .arg(&binary)
        .status()
        .expect("run rustc for Rust mock executable");
    assert!(
        status.success(),
        "mock-command compilation failed: {status}"
    );
    (Scratch(scratch), binary)
}

#[test]
fn system_runner_connects_pipeline_without_a_shell() {
    let (_scratch, binary) = build_mock_command();
    let first = CommandSpec::new(&binary, ["first"])
        .phase("pipe")
        .stdin(CommandInput::Bytes(b"input".to_vec()))
        .stderr_mode(OutputMode::Capture);
    let second = CommandSpec::new(&binary, ["second"])
        .phase("pipe")
        .env("MOCK_EXIT_CODE", "7")
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Capture);

    let output = SystemRunner
        .run_pipeline(&[first, second])
        .expect("run Rust-managed process pipeline");
    assert_eq!(output.statuses, [Some(0), Some(7)]);
    assert_eq!(output.stdout, b"out:second:out:first:input\n\n");
    assert_eq!(output.stderr, b"diagnostic\ndiagnostic\n");
}

#[test]
fn swap_safe_runner_rejects_mutations_in_commands_and_pipelines_before_execution() {
    let mut runner = SwapSafeRunner::new(MockRunner::new([CommandOutput::new(
        Some(0),
        Vec::new(),
        Vec::new(),
    )]));
    let swapon = CommandSpec::new("/sbin/swapon", ["--all"]).phase("unsafe");
    let nested = CommandSpec::new("chroot", ["/os", "mkswap", "/dev/mock"]).phase("unsafe-nested");
    let harmless = CommandSpec::new("printf", ["safe"]).phase("safe");

    assert!(runner.run(&swapon).is_err());
    assert!(runner.run_pipeline(&[harmless, nested]).is_err());
    assert!(runner.into_inner().calls().is_empty());
}

#[test]
fn mock_runner_reports_unconfigured_invocation() {
    let mut runner = MockRunner::empty();
    let command = CommandSpec::new("mkfs.ext4", ["/dev/mock-disk"]).phase("format");
    assert!(runner.run(&command).is_err());
}

#[test]
fn checked_execution_requires_zero_and_preserves_signal_and_failure_statuses() {
    for status in [Some(0), Some(7), Some(141), None] {
        let mut runner = MockRunner::new([CommandOutput::new(status, Vec::new(), Vec::new())]);
        let command = CommandSpec::new("installer", ["secret-argument"]).phase("checked");
        let result = run_checked(&mut runner, &command);
        assert_eq!(runner.calls(), std::slice::from_ref(&command));
        if status == Some(0) {
            assert!(result.is_ok());
        } else {
            let error = result.unwrap_err();
            assert!(!format!("{error}").contains("secret-argument"));
            match error {
                CommandError::Failed {
                    program,
                    status: actual,
                } => {
                    assert_eq!(program, "installer");
                    assert_eq!(actual, status);
                }
                other => panic!("unexpected command error: {other}"),
            }
        }
    }
}

#[test]
fn checked_execution_keeps_spawn_error_distinct_from_a_child_exit() {
    let command = CommandSpec::new("missing-installer", ["secret-argument"]);
    let error = run_checked(&mut MockRunner::empty(), &command).unwrap_err();
    assert!(!format!("{error}").contains("secret-argument"));
    assert!(std::error::Error::source(&error).is_some());
    match error {
        CommandError::Start { program, error } => {
            assert_eq!(program, "missing-installer");
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("unexpected command error: {other}"),
    }
}
