use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use std::fmt;
use std::io;

#[derive(Debug)]
pub enum LogStreamError {
    InvalidPath,
    Command(io::Error),
    PipelineFailed,
}

impl fmt::Display for LogStreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath => f.write_str("Invalid websocket log path"),
            Self::Command(error) => {
                write!(f, "Cannot start websocket log filter pipeline: {error}")
            }
            Self::PipelineFailed => f.write_str("Websocket log filter pipeline failed"),
        }
    }
}

impl std::error::Error for LogStreamError {}

pub fn run_log_stream<R: CommandRunner>(
    runner: &mut R,
    expected_path: &str,
    request_path: &str,
) -> Result<(), LogStreamError> {
    if !valid_web_path(expected_path) {
        return Err(LogStreamError::InvalidPath);
    }
    if request_path != expected_path {
        return Ok(());
    }

    let tail = CommandSpec::new("tail", ["-fn+0", "/rustaller.log"])
        .phase("web-log-stream")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream);
    let tr = CommandSpec::new("tr", ["\r", "\n"])
        .phase("web-log-stream")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream);
    let grep = CommandSpec::new("grep", ["-Fiv", "-e", "password", "-e", "token"])
        .phase("web-log-stream")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream);
    let output = runner
        .run_pipeline(&[tail, tr, grep])
        .map_err(LogStreamError::Command)?;
    // grep exits 1 when every line was filtered, which is a valid empty stream.
    // Other failures (including signalled processes) must reach the worker CLI.
    if !matches!(output.statuses.as_slice(), [Some(0), Some(0), Some(0 | 1)]) {
        return Err(LogStreamError::PipelineFailed);
    }
    Ok(())
}

pub fn valid_web_path(path: &str) -> bool {
    path.len() == 9
        && path.starts_with('/')
        && path[1..].bytes().all(|byte| byte.is_ascii_alphanumeric())
}
