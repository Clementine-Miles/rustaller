use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use crate::model::{InstallPlan, PlanError, RamError, Target, validate_host_arch, validate_ram};
use crate::plan_file::{PlanFileError, PrivatePlanFile};
use std::fmt;
use std::io;
use std::path::Path;

pub struct ValidatedLivePlan {
    plan: InstallPlan,
}

impl ValidatedLivePlan {
    pub fn plan(&self) -> &InstallPlan {
        &self.plan
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LivePreflightError {
    InvalidPlan(PlanError),
    NonAutomaticTarget(Target),
    RootRequired,
    UnsupportedArchitecture(String),
    InsufficientRam(RamError),
    RuntimeVersionMismatch {
        expected: String,
        found: String,
    },
    PlanFile(String),
    CommandStart {
        program: &'static str,
        error: String,
    },
    CommandFailed {
        program: &'static str,
        status: Option<i32>,
    },
    InvalidMemoryOutput,
}

impl fmt::Display for LivePreflightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPlan(error) => write!(f, "Invalid install plan: {error}"),
            Self::NonAutomaticTarget(target) => {
                write!(f, "Alpine Live worker cannot install target {target}")
            }
            Self::RootRequired => f.write_str("Alpine Live installer must run as root"),
            Self::UnsupportedArchitecture(architecture) => write!(
                f,
                "Only Linux x86_64 targets are supported (reported: {architecture})"
            ),
            Self::InsufficientRam(error) => error.fmt(f),
            Self::RuntimeVersionMismatch { expected, found } => write!(
                f,
                "Installer resource version mismatch (expected {expected}, found {found})"
            ),
            Self::PlanFile(error) => write!(f, "Cannot read private install plan: {error}"),
            Self::CommandStart { program, error } => {
                write!(f, "Cannot start Live preflight command {program}: {error}")
            }
            Self::CommandFailed { program, status } => {
                write!(
                    f,
                    "Live preflight command {program} failed with status {status:?}"
                )
            }
            Self::InvalidMemoryOutput => {
                f.write_str("Cannot parse total memory from free -m output")
            }
        }
    }
}

impl std::error::Error for LivePreflightError {}

impl From<PlanFileError> for LivePreflightError {
    fn from(error: PlanFileError) -> Self {
        Self::PlanFile(error.to_string())
    }
}

pub fn load_and_validate_plan<R: CommandRunner>(
    runner: &mut R,
    plan_path: &Path,
    runtime_version: &str,
) -> Result<ValidatedLivePlan, LivePreflightError> {
    load_and_validate_plan_with_uid(runner, plan_path, runtime_version, effective_uid())
}

pub fn load_and_validate_plan_with_uid<R: CommandRunner>(
    runner: &mut R,
    plan_path: &Path,
    runtime_version: &str,
    effective_uid: u32,
) -> Result<ValidatedLivePlan, LivePreflightError> {
    if effective_uid != 0 {
        return Err(LivePreflightError::RootRequired);
    }
    let private_plan = PrivatePlanFile::open(plan_path)?;
    let plan = private_plan.read()?;
    plan.validate().map_err(LivePreflightError::InvalidPlan)?;
    if !plan.target.is_automatic() {
        return Err(LivePreflightError::NonAutomaticTarget(plan.target));
    }

    let architecture = run_text_command(runner, "uname", ["-m"], "live-preflight-arch")?;
    let architecture = architecture.trim();
    validate_host_arch(architecture)
        .map_err(|_| LivePreflightError::UnsupportedArchitecture(architecture.to_owned()))?;

    let memory = run_text_command(runner, "free", ["-m"], "live-preflight-memory")?;
    let memory_mb = parse_free_memory_mb(&memory)?;
    validate_live_plan_with_uid(
        plan,
        architecture,
        memory_mb,
        runtime_version,
        effective_uid,
    )
}

pub fn parse_free_memory_mb(output: &str) -> Result<u64, LivePreflightError> {
    output
        .lines()
        .nth(1)
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|total| total.parse().ok())
        .ok_or(LivePreflightError::InvalidMemoryOutput)
}

fn run_text_command<R, I, S>(
    runner: &mut R,
    program: &'static str,
    args: I,
    phase: &'static str,
) -> Result<String, LivePreflightError>
where
    R: CommandRunner,
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let command = CommandSpec::new(program, args)
        .phase(phase)
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Stream);
    let output = runner
        .run(&command)
        .map_err(|error| LivePreflightError::CommandStart {
            program,
            error: io::Error::to_string(&error),
        })?;
    if output.status != Some(0) {
        return Err(LivePreflightError::CommandFailed {
            program,
            status: output.status,
        });
    }
    String::from_utf8(output.stdout).map_err(|_| LivePreflightError::InvalidMemoryOutput)
}

pub fn validate_live_plan(
    plan: InstallPlan,
    architecture: &str,
    memory_mb: u64,
    runtime_version: &str,
) -> Result<ValidatedLivePlan, LivePreflightError> {
    validate_live_plan_with_uid(
        plan,
        architecture,
        memory_mb,
        runtime_version,
        effective_uid(),
    )
}

pub fn validate_live_plan_with_uid(
    plan: InstallPlan,
    architecture: &str,
    memory_mb: u64,
    runtime_version: &str,
    effective_uid: u32,
) -> Result<ValidatedLivePlan, LivePreflightError> {
    plan.validate().map_err(LivePreflightError::InvalidPlan)?;
    if effective_uid != 0 {
        return Err(LivePreflightError::RootRequired);
    }
    if !plan.target.is_automatic() {
        return Err(LivePreflightError::NonAutomaticTarget(plan.target));
    }
    validate_host_arch(architecture)
        .map_err(|_| LivePreflightError::UnsupportedArchitecture(architecture.to_owned()))?;
    validate_ram(plan.target, memory_mb).map_err(LivePreflightError::InsufficientRam)?;
    if runtime_version != crate::RUNTIME_VERSION {
        return Err(LivePreflightError::RuntimeVersionMismatch {
            expected: crate::RUNTIME_VERSION.to_owned(),
            found: runtime_version.to_owned(),
        });
    }
    Ok(ValidatedLivePlan { plan })
}

unsafe extern "C" {
    fn geteuid() -> u32;
}

fn effective_uid() -> u32 {
    unsafe { geteuid() }
}
