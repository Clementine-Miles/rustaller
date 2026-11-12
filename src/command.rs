use std::collections::{BTreeMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread::{self, JoinHandle};

/// Common native Live defaults. Callers retain distro-specific environment,
/// errors and retries.
pub(crate) fn live_command<I, S>(program: &str, args: I, phase: &str) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    CommandSpec::new(program, args)
        .phase(phase)
        .cwd("/")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream)
}

/// Target adapters share tool lookup and offline service policy; each adapter
/// still selects its own locale and package-manager environment.
pub(crate) fn target_command<I, S>(program: &str, args: I, phase: &str) -> CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    live_command(program, args, phase)
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .env("SYSTEMD_OFFLINE", "1")
}

#[derive(Clone, Eq, PartialEq)]
pub enum CommandInput {
    Inherit,
    Null,
    Bytes(Vec<u8>),
}

impl std::fmt::Debug for CommandInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inherit => f.write_str("Inherit"),
            Self::Null => f.write_str("Null"),
            Self::Bytes(bytes) => write!(f, "Bytes([REDACTED; {} bytes])", bytes.len()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputMode {
    /// Inherit the parent's descriptor, including its terminal properties.
    Stream,
    Capture,
    Discard,
}

#[derive(Clone, Eq, PartialEq)]
pub struct CommandSpec {
    pub phase: String,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<OsString, OsString>,
    pub stdin: CommandInput,
    pub stdout_mode: OutputMode,
    pub stderr_mode: OutputMode,
    // Only the target swapfile allocator can grant this exact mkswap invocation.
    target_swapfile: Option<PathBuf>,
}

impl CommandSpec {
    pub fn new<I, S>(program: impl AsRef<OsStr>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self {
            phase: String::new(),
            program: program.as_ref().to_owned(),
            args: args
                .into_iter()
                .map(|arg| arg.as_ref().to_owned())
                .collect(),
            cwd: None,
            env: BTreeMap::new(),
            stdin: CommandInput::Inherit,
            stdout_mode: OutputMode::Stream,
            stderr_mode: OutputMode::Stream,
            target_swapfile: None,
        }
    }

    pub fn phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = phase.into();
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env
            .insert(key.as_ref().to_owned(), value.as_ref().to_owned());
        self
    }

    pub fn stdin(mut self, input: CommandInput) -> Self {
        self.stdin = input;
        self
    }

    pub fn stdout_mode(mut self, mode: OutputMode) -> Self {
        self.stdout_mode = mode;
        self
    }

    pub fn stderr_mode(mut self, mode: OutputMode) -> Self {
        self.stderr_mode = mode;
        self
    }

    pub(crate) fn format_target_swapfile(path: &std::path::Path) -> Self {
        let mut spec = Self::new("mkswap", [OsStr::new("--"), path.as_os_str()])
            .phase("target-swapfile-format")
            .cwd("/")
            .stdin(CommandInput::Null);
        spec.target_swapfile = Some(path.to_owned());
        spec
    }
}

impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env_keys: Vec<&OsStr> = self.env.keys().map(OsString::as_os_str).collect();
        f.debug_struct("CommandSpec")
            .field("phase", &self.phase)
            .field("program", &self.program)
            .field("args", &"[REDACTED]")
            .field("cwd", &self.cwd)
            .field("env_keys", &env_keys)
            .field("stdin", &self.stdin)
            .field("stdout_mode", &self.stdout_mode)
            .field("stderr_mode", &self.stderr_mode)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    pub fn new(status: Option<i32>, stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
        Self {
            status,
            stdout,
            stderr,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PipelineOutput {
    pub statuses: Vec<Option<i32>>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait CommandRunner {
    fn run(&mut self, command: &CommandSpec) -> io::Result<CommandOutput>;
    fn run_pipeline(&mut self, commands: &[CommandSpec]) -> io::Result<PipelineOutput>;
}

#[derive(Debug)]
pub enum CommandError {
    Start {
        program: OsString,
        error: io::Error,
    },
    Failed {
        program: OsString,
        status: Option<i32>,
    },
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Start { program, error } => {
                write!(
                    f,
                    "Cannot start command {}: {error}",
                    program.to_string_lossy()
                )
            }
            Self::Failed { program, status } => write!(
                f,
                "Command {} failed with status {status:?}",
                program.to_string_lossy()
            ),
        }
    }
}

impl std::error::Error for CommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Start { error, .. } => Some(error),
            Self::Failed { .. } => None,
        }
    }
}

/// Require exit zero without changing the command's arguments or I/O policy.
pub fn run_checked<R: CommandRunner>(
    runner: &mut R,
    command: &CommandSpec,
) -> Result<(), CommandError> {
    checked_output(runner, command).map(|_| ())
}

/// Capture stdout after a successful command. Stderr, stdin, environment and
/// working directory retain the caller's policy; decoding and bounds belong to
/// the domain that consumes the bytes.
pub(crate) fn capture_checked<R: CommandRunner>(
    runner: &mut R,
    command: &CommandSpec,
) -> Result<Vec<u8>, CommandError> {
    checked_output(runner, &command.clone().stdout_mode(OutputMode::Capture))
        .map(|output| output.stdout)
}

fn checked_output<R: CommandRunner>(
    runner: &mut R,
    command: &CommandSpec,
) -> Result<CommandOutput, CommandError> {
    let output = runner.run(command).map_err(|error| CommandError::Start {
        program: command.program.clone(),
        error,
    })?;
    if output.status == Some(0) {
        Ok(output)
    } else {
        Err(CommandError::Failed {
            program: command.program.clone(),
            status: output.status,
        })
    }
}

/// Preserve the Shell retry wrapper's spawn-error statuses.
pub(crate) fn shell_attempt<R: CommandRunner>(
    runner: &mut R,
    command: &CommandSpec,
) -> CommandOutput {
    runner.run(command).unwrap_or_else(|error| {
        let status = if error.kind() == io::ErrorKind::PermissionDenied {
            126
        } else {
            127
        };
        CommandOutput::new(Some(status), Vec::new(), Vec::new())
    })
}

/// Guard source/Live swap state; only the allocator's exact inactive-file
/// mkswap invocation has a private permit. swapon/swapoff remain forbidden.
pub struct SwapSafeRunner<R> {
    inner: R,
}

impl<R> SwapSafeRunner<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: CommandRunner> CommandRunner for SwapSafeRunner<R> {
    fn run(&mut self, command: &CommandSpec) -> io::Result<CommandOutput> {
        reject_swap_mutation(command)?;
        self.inner.run(command)
    }

    fn run_pipeline(&mut self, commands: &[CommandSpec]) -> io::Result<PipelineOutput> {
        for command in commands {
            reject_swap_mutation(command)?;
        }
        self.inner.run_pipeline(commands)
    }
}

fn reject_swap_mutation(command: &CommandSpec) -> io::Result<()> {
    if let Some(path) = &command.target_swapfile
        && command.program == "mkswap"
        && command.args == [OsString::from("--"), path.as_os_str().to_owned()]
        && command.phase == "target-swapfile-format"
    {
        return Ok(());
    }
    let forbidden = ["swapon", "swapoff", "mkswap"];
    let candidates = std::iter::once(command.program.as_os_str())
        .chain(command.args.iter().map(OsString::as_os_str));
    if candidates.into_iter().any(|candidate| {
        let candidate = candidate.to_string_lossy();
        let basename = candidate.rsplit('/').next().unwrap_or(&candidate);
        forbidden.contains(&basename)
    }) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "swap mutation is forbidden in installer phase {}",
                command.phase
            ),
        ));
    }
    Ok(())
}

#[derive(Default)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&mut self, spec: &CommandSpec) -> io::Result<CommandOutput> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args).envs(&spec.env);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        command.stdin(match &spec.stdin {
            CommandInput::Inherit => Stdio::inherit(),
            CommandInput::Null => Stdio::null(),
            CommandInput::Bytes(_) => Stdio::piped(),
        });
        command.stdout(to_stdio(spec.stdout_mode));
        command.stderr(to_stdio(spec.stderr_mode));

        let mut child = command.spawn()?;
        let stdin_writer = match &spec.stdin {
            CommandInput::Bytes(bytes) => child.stdin.take().map(|mut stdin| {
                let bytes = bytes.clone();
                thread::spawn(move || stdin.write_all(&bytes))
            }),
            CommandInput::Inherit | CommandInput::Null => None,
        };
        let stdout_reader = take_reader(&mut child, spec.stdout_mode, Stream::Stdout)?;
        let stderr_reader = take_reader(&mut child, spec.stderr_mode, Stream::Stderr)?;
        let status = child.wait()?.code();

        let stdout = finish_reader(stdout_reader)?;
        let stderr = finish_reader(stderr_reader)?;
        finish_writer(stdin_writer)?;
        Ok(CommandOutput::new(status, stdout, stderr))
    }

    fn run_pipeline(&mut self, specs: &[CommandSpec]) -> io::Result<PipelineOutput> {
        if specs.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot run an empty process pipeline",
            ));
        }

        let mut children: Vec<Child> = Vec::with_capacity(specs.len());
        let mut previous_stdout: Option<ChildStdout> = None;
        let mut first_stdin: Option<ChildStdin> = None;
        let mut stderr_readers = Vec::with_capacity(specs.len());
        let mut final_stdout_reader = None;

        for (index, spec) in specs.iter().enumerate() {
            let is_first = index == 0;
            let is_last = index + 1 == specs.len();
            let mut command = Command::new(&spec.program);
            command.args(&spec.args).envs(&spec.env);
            if let Some(cwd) = &spec.cwd {
                command.current_dir(cwd);
            }
            if is_first {
                command.stdin(match &spec.stdin {
                    CommandInput::Inherit => Stdio::inherit(),
                    CommandInput::Null => Stdio::null(),
                    CommandInput::Bytes(_) => Stdio::piped(),
                });
            } else {
                let input = previous_stdout
                    .take()
                    .ok_or_else(|| io::Error::other("pipeline stdout was not connected"))?;
                command.stdin(Stdio::from(input));
            }
            command.stdout(if is_last {
                to_stdio(spec.stdout_mode)
            } else {
                Stdio::piped()
            });
            command.stderr(to_stdio(spec.stderr_mode));

            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    for child in &mut children {
                        let _ = child.kill();
                    }
                    for mut child in children {
                        let _ = child.wait();
                    }
                    return Err(error);
                }
            };

            if is_first && matches!(&spec.stdin, CommandInput::Bytes(_)) {
                first_stdin = child.stdin.take();
            }
            if is_last {
                final_stdout_reader = take_reader(&mut child, spec.stdout_mode, Stream::Stdout)?;
            } else {
                previous_stdout = child.stdout.take();
            }
            stderr_readers.push(take_reader(&mut child, spec.stderr_mode, Stream::Stderr)?);
            children.push(child);
        }

        let stdin_writer = match (&specs[0].stdin, first_stdin) {
            (CommandInput::Bytes(bytes), Some(mut stdin)) => {
                let bytes = bytes.clone();
                Some(thread::spawn(move || stdin.write_all(&bytes)))
            }
            (CommandInput::Bytes(_), None) => {
                return Err(io::Error::other("pipeline stdin pipe was not created"));
            }
            (CommandInput::Inherit | CommandInput::Null, _) => None,
        };

        let mut statuses = Vec::with_capacity(children.len());
        for child in &mut children {
            statuses.push(child.wait()?.code());
        }
        let stdout = finish_reader(final_stdout_reader)?;
        let mut stderr = Vec::new();
        for reader in stderr_readers {
            stderr.extend(finish_reader(reader)?);
        }
        finish_writer(stdin_writer)?;
        Ok(PipelineOutput {
            statuses,
            stdout,
            stderr,
        })
    }
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

fn to_stdio(mode: OutputMode) -> Stdio {
    match mode {
        // Live output needs no Rust forwarding thread or intermediate pipe.
        // Inheriting also preserves native terminal detection and backpressure.
        OutputMode::Stream => Stdio::inherit(),
        OutputMode::Capture => Stdio::piped(),
        OutputMode::Discard => Stdio::null(),
    }
}

fn take_reader(
    child: &mut Child,
    mode: OutputMode,
    stream: Stream,
) -> io::Result<Option<JoinHandle<io::Result<Vec<u8>>>>> {
    if mode != OutputMode::Capture {
        return Ok(None);
    }
    Ok(Some(match stream {
        Stream::Stdout => capture_reader(
            child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("child stdout pipe was not created"))?,
        ),
        Stream::Stderr => capture_reader(
            child
                .stderr
                .take()
                .ok_or_else(|| io::Error::other("child stderr pipe was not created"))?,
        ),
    }))
}

fn capture_reader(mut reader: impl Read + Send + 'static) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut captured = Vec::new();
        reader.read_to_end(&mut captured)?;
        Ok(captured)
    })
}

fn finish_reader(reader: Option<JoinHandle<io::Result<Vec<u8>>>>) -> io::Result<Vec<u8>> {
    match reader {
        Some(reader) => reader
            .join()
            .map_err(|_| io::Error::other("child output reader panicked"))?,
        None => Ok(Vec::new()),
    }
}

fn finish_writer(writer: Option<JoinHandle<io::Result<()>>>) -> io::Result<()> {
    let Some(writer) = writer else {
        return Ok(());
    };
    match writer
        .join()
        .map_err(|_| io::Error::other("child stdin writer panicked"))?
    {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error),
    }
}

pub struct MockRunner {
    calls: Vec<CommandSpec>,
    responses: VecDeque<CommandOutput>,
}

impl MockRunner {
    pub fn new(outputs: impl IntoIterator<Item = CommandOutput>) -> Self {
        Self {
            calls: Vec::new(),
            responses: outputs.into_iter().collect(),
        }
    }

    pub fn empty() -> Self {
        Self::new([])
    }

    pub fn calls(&self) -> &[CommandSpec] {
        &self.calls
    }
}

impl CommandRunner for MockRunner {
    fn run(&mut self, command: &CommandSpec) -> io::Result<CommandOutput> {
        self.calls.push(command.clone());
        self.responses.pop_front().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "no mock response configured for {}",
                    command.program.to_string_lossy()
                ),
            )
        })
    }

    fn run_pipeline(&mut self, commands: &[CommandSpec]) -> io::Result<PipelineOutput> {
        let mut outputs = Vec::with_capacity(commands.len());
        for command in commands {
            self.calls.push(command.clone());
            let output = self.responses.pop_front().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "no mock response configured for {}",
                        command.program.to_string_lossy()
                    ),
                )
            })?;
            outputs.push(output);
        }
        let stdout = outputs
            .last()
            .map_or_else(Vec::new, |output| output.stdout.clone());
        let mut stderr = Vec::new();
        for output in &outputs {
            stderr.extend_from_slice(&output.stderr);
        }
        Ok(PipelineOutput {
            statuses: outputs.iter().map(|output| output.status).collect(),
            stdout,
            stderr,
        })
    }
}
