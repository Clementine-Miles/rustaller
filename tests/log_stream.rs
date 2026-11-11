use reinstall_rs::command::{CommandOutput, MockRunner};
use reinstall_rs::worker::args::{WorkerAction, parse_worker_args};
use reinstall_rs::worker::log_stream::run_log_stream;

#[test]
fn worker_parses_only_the_exact_generated_web_log_path_form() {
    assert_eq!(
        parse_worker_args(["log-stream", "/A1B2C3D4"]).unwrap(),
        Some(WorkerAction::LogStream {
            path: "/A1B2C3D4".into()
        })
    );
    for path in ["relative", "/short", "/has-dash", "/A1B2C3D4/extra"] {
        assert!(parse_worker_args(["log-stream", path]).is_err());
    }
}

#[test]
fn log_worker_runs_the_legacy_tail_tr_grep_pipeline_only_for_the_requested_path() {
    let mut runner = MockRunner::new([
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
    ]);
    run_log_stream(&mut runner, "/A1B2C3D4", "/A1B2C3D4").unwrap();
    assert_eq!(runner.calls().len(), 3);
    assert_eq!(runner.calls()[0].program, "tail");
    assert_eq!(runner.calls()[0].args, ["-fn+0", "/reinstall.log"]);
    assert_eq!(runner.calls()[1].program, "tr");
    assert_eq!(runner.calls()[1].args, ["\r", "\n"]);
    assert_eq!(runner.calls()[2].program, "grep");
    assert_eq!(
        runner.calls()[2].args,
        ["-Fiv", "-e", "password", "-e", "token"]
    );
    assert!(
        runner
            .calls()
            .iter()
            .all(|call| call.stdout_mode == reinstall_rs::command::OutputMode::Stream)
    );

    let mut unused = MockRunner::empty();
    run_log_stream(&mut unused, "/A1B2C3D4", "/wrongpath").unwrap();
    assert!(unused.calls().is_empty());
}
