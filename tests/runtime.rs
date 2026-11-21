mod support;
mod command {
    use rustaller::command::{
        CommandError, CommandInput, CommandOutput, CommandRunner, CommandSpec, MockRunner,
        OutputMode, SwapSafeRunner, SystemRunner, run_checked,
    };

    use std::path::{Path, PathBuf};
    use std::process::Command;

    #[test]
    fn streamed_output_inherits_the_parent_while_capture_stays_private() {
        const PROBE: &str = "RUSTALLER_STREAM_TEST_CHILD";
        if std::env::var_os(PROBE).is_some() {
            let (_scratch, binary) = build_mock_command();
            let spec = CommandSpec::new(&binary, ["stream-sentinel"])
                .stdout_mode(OutputMode::Stream)
                .stderr_mode(OutputMode::Capture);
            let output = SystemRunner.run(&spec).unwrap();
            assert_eq!(output.status, Some(0));
            assert!(output.stdout.is_empty());
            assert_eq!(output.stderr, b"diagnostic\n");
            return;
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "command::streamed_output_inherits_the_parent_while_capture_stays_private",
                "--nocapture",
            ])
            .env(PROBE, "1")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("stream-sentinel"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("diagnostic"));
        assert!(output.stderr.is_empty());
    }

    #[test]
    fn inspection_deadline_also_closes_descendant_output_pipes() {
        let start = std::time::Instant::now();
        // Controlled local fixture: the descendant inherits stdout. Killing
        // only the shell would leave the bounded-output reader waiting on it.
        let error = SystemRunner
            .run(&CommandSpec::new("sh", ["-c", "sleep 30 & wait"]).inspection_limits())
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
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
    fn optional_inspection_bounds_output_and_wait_time() {
        let start = std::time::Instant::now();
        let error = SystemRunner
            .run(&CommandSpec::new("sleep", ["30"]).inspection_limits())
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        let error = SystemRunner
            .run(&CommandSpec::new("head", ["-c", "1048576", "/dev/zero"]).inspection_limits())
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        let output = SystemRunner
            .run(&CommandSpec::new("cat", [] as [&str; 0]).inspection_limits())
            .unwrap();
        assert_eq!(output.status, Some(0));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
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

    use crate::support::Scratch;

    fn build_mock_command() -> (Scratch, PathBuf) {
        let scratch = Scratch::new("mock");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/mock-command.rs");
        let binary = scratch.0.join("mock-command");
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
        (scratch, binary)
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
        let nested =
            CommandSpec::new("chroot", ["/os", "mkswap", "/dev/mock"]).phase("unsafe-nested");
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
}

mod cli {
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
}

mod worker_args {
    use rustaller::worker::args::{WorkerAction, parse_worker_args};

    #[test]
    fn worker_accepts_only_known_actions_and_validates_initrd_arguments() {
        assert_eq!(parse_worker_args(std::iter::empty::<&str>()).unwrap(), None);
        assert_eq!(parse_worker_args(["--help"]).unwrap(), None);
        assert_eq!(
            parse_worker_args(["live-modules"]).unwrap(),
            Some(WorkerAction::LiveModules)
        );
        assert!(parse_worker_args(["live-modules", "/arbitrary/image"]).is_err());
        assert_eq!(
            parse_worker_args(["firstboot-fix"]).unwrap(),
            Some(WorkerAction::FirstbootFix)
        );
        assert_eq!(
            parse_worker_args(["cleanup-openrc"]).unwrap(),
            Some(WorkerAction::CleanupOpenRc)
        );
        assert!(parse_worker_args(["cleanup-systemd"]).is_err());
        let network = parse_worker_args([
            "initrd-network",
            "00:11:22:33:44:55",
            "198.51.100.8/24",
            "198.51.100.1",
            "2001:db8::8/64",
            "2001:db8::1",
            "",
        ])
        .unwrap();
        let Some(WorkerAction::InitrdNetwork(config)) = network else {
            panic!("expected validated initrd network action");
        };
        assert_eq!(config.mac_addr(), "00:11:22:33:44:55");
        assert!(parse_worker_args(["initrd-network", "not-a-mac", "", "", "", "", ""]).is_err());
        assert!(parse_worker_args(["firstboot-fix", "extra"]).is_err());
        assert_eq!(
            parse_worker_args(["cachyos-firstboot"]).unwrap(),
            Some(WorkerAction::CachyosFirstboot)
        );
        assert!(parse_worker_args(["cachyos-firstboot", "--anything"]).is_err());
        assert!(parse_worker_args(["unknown"]).is_err());
        assert_eq!(
            parse_worker_args([
                "live-install",
                "/configs/install.plan",
                "/configs/deployment.manifest"
            ])
            .unwrap(),
            Some(WorkerAction::LiveInstall {
                plan: "/configs/install.plan".into(),
                manifest: "/configs/deployment.manifest".into()
            })
        );
        for args in [
            vec!["live-install", "relative", "/configs/m"],
            vec!["live-install", "/configs/p", "/configs/p"],
            vec!["live-install", "/configs/../p", "/configs/m"],
            vec!["live-install", "/configs/p", "/configs/m", "extra"],
        ] {
            assert!(parse_worker_args(args).is_err());
        }
    }
}

mod worker_cli {
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
}

mod embedded {
    #[test]
    fn debug_host_embeds_explicit_x86_64_worker_fixture_with_current_runtime_marker() {
        let worker = rustaller::worker_bytes();
        assert_eq!(&worker[..4], b"\x7fELF");
        assert_eq!(worker[4], 2);
        assert_eq!(worker[5], 1);
        assert_eq!(&worker[18..20], &[0x3e, 0]);
        assert!(
            worker
                .windows(rustaller::RUNTIME_VERSION.len())
                .any(|window| window == rustaller::RUNTIME_VERSION.as_bytes())
        );
    }
}
