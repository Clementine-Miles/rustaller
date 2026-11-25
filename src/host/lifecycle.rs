//! Rustaller's lock and state record live outside the source's configuration.
//! No reset path invokes kexec, changes boot files or requests a reboot.
use crate::command::{CommandOutput, CommandRunner, CommandSpec, PipelineOutput, SystemRunner};
use crate::fsutil::{atomic_replace, effective_uid};
use std::fmt;
use std::fs::{self, DirBuilder, File, OpenOptions, TryLockError};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const HOST_STATE: &str = "/var/tmp/rustaller-handoff";
const LIVE_STATE: &str = "/run/rustaller-lifecycle";
const SCHEMA: &str = "rustaller-lifecycle-1";
const NO_FOLLOW: i32 = 0o400000;
const NON_BLOCK: i32 = 0o4000;

#[derive(Debug)]
pub enum LifecycleError {
    Unsafe(&'static str),
    Io(io::Error),
}
impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsafe(reason) => f.write_str(reason),
            Self::Io(error) => write!(f, "Cannot access Rustaller lifecycle state: {error}"),
        }
    }
}
impl std::error::Error for LifecycleError {}
impl From<io::Error> for LifecycleError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Preparing,
    Prepared,
    Handoff,
    LiveReady,
    Cancelled,
    Destructive,
    Installed,
}
impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Prepared => "prepared",
            Self::Handoff => "handoff",
            Self::LiveReady => "live-ready",
            Self::Cancelled => "cancelled",
            Self::Destructive => "destructive",
            Self::Installed => "installed",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        [
            Self::Preparing,
            Self::Prepared,
            Self::Handoff,
            Self::LiveReady,
            Self::Cancelled,
            Self::Destructive,
            Self::Installed,
        ]
        .into_iter()
        .find(|phase| phase.label() == value)
    }
}

#[derive(Debug)]
struct Record {
    operation: String,
    boot: String,
    phase: Phase,
}
impl Record {
    fn new(phase: Phase) -> Result<Self, LifecycleError> {
        Ok(Self {
            operation: kernel_id("/proc/sys/kernel/random/uuid")?,
            boot: kernel_id("/proc/sys/kernel/random/boot_id")?,
            phase,
        })
    }
    fn parse(text: &str) -> Result<Self, LifecycleError> {
        let mut fields = text.split_ascii_whitespace();
        if let (Some(SCHEMA), Some(operation), Some(boot), Some(phase), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) && valid_id(operation)
            && valid_id(boot)
            && let Some(phase) = Phase::parse(phase)
        {
            return Ok(Self {
                operation: operation.into(),
                boot: boot.into(),
                phase,
            });
        }
        Err(LifecycleError::Unsafe("Invalid Rustaller lifecycle record"))
    }
    fn workspace(&self, state: &State) -> PathBuf {
        state.path.join(format!("operation-{}", self.operation))
    }
}

fn valid_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, byte)| {
            if [8, 13, 18, 23].contains(&i) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}
fn kernel_id(path: &str) -> Result<String, LifecycleError> {
    let id = bounded_text(File::open(path)?, 64)?.trim().to_owned();
    if !valid_id(&id) {
        return Err(LifecycleError::Unsafe(
            "Kernel did not provide a valid lifecycle identifier",
        ));
    }
    Ok(id)
}
fn bounded_text(file: File, maximum: u64) -> Result<String, LifecycleError> {
    let mut text = String::new();
    file.take(maximum + 1).read_to_string(&mut text)?;
    if text.len() as u64 > maximum {
        return Err(LifecycleError::Unsafe(
            "Oversized Rustaller lifecycle input",
        ));
    }
    Ok(text)
}
fn require_root() -> Result<(), LifecycleError> {
    if effective_uid() != 0 {
        return Err(LifecycleError::Unsafe(
            "Root privileges are required to change Rustaller lifecycle state",
        ));
    }
    Ok(())
}

struct State {
    path: PathBuf,
    live: bool,
}
impl State {
    fn open(path: &Path, live: bool, create: bool) -> Result<Option<Self>, LifecycleError> {
        let Some(parent) = path.parent() else {
            return Err(LifecycleError::Unsafe(
                "Invalid Rustaller lifecycle directory",
            ));
        };
        let mut prefix = PathBuf::new();
        for part in parent.components() {
            prefix.push(part);
            let meta = match fs::symlink_metadata(&prefix) {
                Ok(meta) => meta,
                Err(error) if !create && error.kind() == io::ErrorKind::NotFound => {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            };
            if !meta.is_dir()
                || ![0, effective_uid()].contains(&meta.uid())
                || (meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0)
            {
                return Err(LifecycleError::Unsafe(
                    "Untrusted Rustaller lifecycle directory ancestor",
                ));
            }
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                match DirBuilder::new().mode(0o700).create(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
                fs::symlink_metadata(path)?
            }
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_dir()
            || metadata.mode() & 0o7777 != 0o700
            || metadata.uid() != 0 && (create || metadata.uid() != effective_uid())
        {
            return Err(LifecycleError::Unsafe(
                "Untrusted Rustaller lifecycle directory",
            ));
        }
        Ok(Some(Self {
            path: path.into(),
            live,
        }))
    }

    fn lock(&self, name: &str, wait: bool) -> Result<Option<File>, LifecycleError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(NO_FOLLOW)
            .open(self.path.join(name))?;
        self.validate_file(&file)?;
        if wait {
            file.lock()?;
        } else {
            match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
        Ok(Some(file))
    }

    fn validate_file(&self, file: &File) -> Result<(), LifecycleError> {
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.mode() & 0o7777 != 0o600
            || metadata.uid() != fs::symlink_metadata(&self.path)?.uid()
        {
            return Err(LifecycleError::Unsafe("Untrusted Rustaller lifecycle file"));
        }
        Ok(())
    }

    fn read_record(&self) -> Result<Option<Record>, LifecycleError> {
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(NO_FOLLOW | NON_BLOCK)
            .open(self.path.join("state"))
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        self.validate_file(&file)?;
        let record = Record::parse(&bounded_text(file, 192)?)?;
        let host_phase = matches!(
            record.phase,
            Phase::Preparing | Phase::Prepared | Phase::Handoff
        );
        let live_phase = matches!(
            record.phase,
            Phase::LiveReady | Phase::Destructive | Phase::Installed
        );
        if (self.live && host_phase) || (!self.live && live_phase) {
            return Err(LifecycleError::Unsafe(
                "Rustaller lifecycle state belongs to a different environment",
            ));
        }
        Ok(Some(record))
    }

    fn write_record(&self, record: &Record) -> Result<(), LifecycleError> {
        atomic_replace(
            &self.path.join("state"),
            format!(
                "{SCHEMA} {} {} {}\n",
                record.operation,
                record.boot,
                record.phase.label()
            )
            .as_bytes(),
            0o600,
        )?;
        Ok(())
    }

    fn discard(&self, record: &Record) -> Result<(), LifecycleError> {
        let workspace = record.workspace(self);
        match fs::symlink_metadata(&workspace) {
            Ok(meta) => {
                let parent = fs::symlink_metadata(&self.path)?;
                if !meta.is_dir()
                    || meta.uid() != parent.uid()
                    || meta.dev() != parent.dev()
                    || meta.mode() & 0o7777 != 0o700
                {
                    return Err(LifecycleError::Unsafe(
                        "Refusing to remove unexpected staging directory",
                    ));
                }
                if includes_mount(&workspace, &mountinfo()?) {
                    return Err(LifecycleError::Unsafe(
                        "Refusing to remove staging with mounted filesystems",
                    ));
                }
                fs::remove_dir_all(&workspace)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        fs::remove_file(self.path.join("state"))?;
        File::open(&self.path)?.sync_all()?;
        Ok(())
    }
}

fn mountinfo() -> Result<String, LifecycleError> {
    bounded_text(File::open("/proc/self/mountinfo")?, 1024 * 1024)
}
fn includes_mount(workspace: &Path, inventory: &str) -> bool {
    inventory.lines().any(|line| {
        line.split_ascii_whitespace()
            .nth(4)
            .is_some_and(|mount| Path::new(mount).starts_with(workspace))
    })
}
fn private_live() -> Result<bool, LifecycleError> {
    let cmdline = bounded_text(File::open("/proc/cmdline")?, 16 * 1024)?;
    if !cmdline
        .split_ascii_whitespace()
        .any(|arg| arg == "rustaller_live=1")
    {
        return Ok(false);
    }
    if !Path::new("/etc/alpine-release").is_file()
        || !mountinfo()?.lines().any(|line| {
            line.split_once(" - ").is_some_and(|(left, right)| {
                left.split_ascii_whitespace().nth(4) == Some("/")
                    && matches!(
                        right.split_ascii_whitespace().next(),
                        Some("rootfs" | "ramfs" | "tmpfs")
                    )
            })
        })
    {
        return Err(LifecycleError::Unsafe(
            "Cannot establish private Alpine Live provenance",
        ));
    }
    Ok(true)
}

pub(crate) struct Preparation {
    state: State,
    record: Record,
    _operation_lock: File,
}
impl Preparation {
    pub(crate) fn begin() -> Result<Self, LifecycleError> {
        require_root()?;
        if private_live()? {
            return Err(LifecycleError::Unsafe(
                "Original system preparation is unavailable in Alpine Live",
            ));
        }
        let state = State::open(Path::new(HOST_STATE), false, true)?.expect("create enabled");
        Self::begin_in(state)
    }
    fn begin_in(state: State) -> Result<Self, LifecycleError> {
        let operation_lock = state
            .lock("operation.lock", false)?
            .ok_or(LifecycleError::Unsafe(
                "A Rustaller operation is already active",
            ))?;
        let _guard = state.lock("state.lock", true)?;
        let record = Record::new(Phase::Preparing)?;
        if let Some(old) = state.read_record()? {
            if old.phase == Phase::Handoff && old.boot == record.boot {
                return Err(uncertain_handoff());
            }
            state.discard(&old)?;
        }
        state.write_record(&record)?;
        let preparation = Self {
            state,
            record,
            _operation_lock: operation_lock,
        };
        if let Err(error) = DirBuilder::new()
            .mode(0o700)
            .create(preparation.workspace())
        {
            // A pre-existing directory cannot be attributed to this operation.
            fs::remove_file(preparation.state.path.join("state"))?;
            return Err(error.into());
        }
        Ok(preparation)
    }
    pub(crate) fn workspace(&self) -> PathBuf {
        self.record.workspace(&self.state)
    }
    pub(crate) fn finish(&self) -> Result<(), LifecycleError> {
        let _guard = self.state.lock("state.lock", true)?;
        let mut record = self.current()?;
        if record.phase == Phase::Cancelled {
            return Err(LifecycleError::Unsafe(
                "Rustaller preparation was cancelled",
            ));
        }
        record.phase = Phase::Prepared;
        self.state.write_record(&record)
    }
    pub(crate) fn discard(&self) -> Result<(), LifecycleError> {
        let _guard = self.state.lock("state.lock", true)?;
        self.state.discard(&self.current()?)
    }
    fn current(&self) -> Result<Record, LifecycleError> {
        self.state
            .read_record()?
            .filter(|record| record.operation == self.record.operation)
            .ok_or(LifecycleError::Unsafe("Rustaller operation state changed"))
    }
}

pub(crate) struct CancelAware<'a, R>(pub(crate) &'a Preparation, pub(crate) &'a mut R);
impl<R: CommandRunner> CommandRunner for CancelAware<'_, R> {
    fn run(&mut self, command: &CommandSpec) -> io::Result<CommandOutput> {
        self.check()?;
        self.1.run(command)
    }
    fn run_pipeline(&mut self, commands: &[CommandSpec]) -> io::Result<PipelineOutput> {
        self.check()?;
        self.1.run_pipeline(commands)
    }
}
impl<R> CancelAware<'_, R> {
    fn check(&self) -> io::Result<()> {
        let _guard = self
            .0
            .state
            .lock("state.lock", true)
            .map_err(io::Error::other)?;
        if self.0.current().map_err(io::Error::other)?.phase != Phase::Preparing {
            return Err(io::Error::other("Rustaller preparation was cancelled"));
        }
        Ok(())
    }
}

pub(crate) struct Handoff {
    state: State,
    record: Record,
    _operation_lock: File,
}
impl Handoff {
    pub(crate) fn begin(workspace: &Path) -> Result<Self, LifecycleError> {
        require_root()?;
        let state = State::open(Path::new(HOST_STATE), false, false)?.ok_or(
            LifecycleError::Unsafe("The preparation has already been removed"),
        )?;
        let operation_lock = state
            .lock("operation.lock", false)?
            .ok_or(LifecycleError::Unsafe(
                "A Rustaller operation is already active",
            ))?;
        let _guard = state.lock("state.lock", true)?;
        let mut record = state
            .read_record()?
            .filter(|record| {
                record.phase == Phase::Prepared && record.workspace(&state) == workspace
            })
            .ok_or(LifecycleError::Unsafe(
                "The preparation was cancelled or replaced",
            ))?;
        if record.boot != kernel_id("/proc/sys/kernel/random/boot_id")? {
            return Err(LifecycleError::Unsafe(
                "Run a fresh preflight after rebooting the source",
            ));
        }
        record.phase = Phase::Handoff;
        state.write_record(&record)?;
        Ok(Self {
            state,
            record,
            _operation_lock: operation_lock,
        })
    }
    pub(crate) fn disarmed(mut self) -> Result<(), LifecycleError> {
        let _guard = self.state.lock("state.lock", true)?;
        self.record.phase = Phase::Prepared;
        self.state.write_record(&self.record)
    }
}
fn uncertain_handoff() -> LifecycleError {
    LifecycleError::Unsafe(
        "Rustaller handoff was initiated during this boot. Ownership of a possibly loaded image cannot be proved. Check the source init shutdown and kexec state from its console before any reboot",
    )
}

pub fn status() -> Result<&'static str, LifecycleError> {
    let live = private_live()?;
    let Some(state) = State::open(
        Path::new(if live { LIVE_STATE } else { HOST_STATE }),
        live,
        false,
    )?
    else {
        return Ok(if live {
            "Alpine Live before disk erasure"
        } else {
            "No pending Rustaller operation"
        });
    };
    Ok(match state.read_record()?.map(|record| record.phase) {
        None => "No pending Rustaller operation",
        Some(Phase::Preparing) => "Preparing authenticated installation assets",
        Some(Phase::Prepared) => "Prepared installation can be cancelled with rustaller reset",
        Some(Phase::Handoff) => {
            "Handoff initiated. Check the console before attempting cancellation."
        }
        Some(Phase::LiveReady) => "Alpine Live before disk erasure",
        Some(Phase::Cancelled) => "Rustaller operation cancelled",
        Some(Phase::Destructive) => "Disk erasure started. Original-system recovery unavailable.",
        Some(Phase::Installed) => {
            "Target installation verified. Original-system recovery unavailable."
        }
    })
}

pub fn reset() -> Result<&'static str, LifecycleError> {
    if private_live()? {
        return crate::worker::install::cancel_live(&mut SystemRunner);
    }
    let Some(state) = State::open(Path::new(HOST_STATE), false, false)? else {
        return Ok("No pending Rustaller operation");
    };
    require_root()?;
    reset_original(&state, &kernel_id("/proc/sys/kernel/random/boot_id")?)
}
fn reset_original(state: &State, boot: &str) -> Result<&'static str, LifecycleError> {
    let _guard = state.lock("state.lock", true)?;
    let Some(mut record) = state.read_record()? else {
        return Ok("No pending Rustaller operation");
    };
    if record.phase == Phase::Handoff && record.boot == boot {
        return Err(uncertain_handoff());
    }
    let operation = state.lock("operation.lock", false)?;
    if operation.is_none() && matches!(record.phase, Phase::Preparing | Phase::Cancelled) {
        record.phase = Phase::Cancelled;
        state.write_record(&record)?;
        return Ok(
            "Preparation cancellation requested. Staging is cleaned once the current preparation command exits.",
        );
    }
    state.discard(&record)?;
    Ok("Pending Rustaller preparation cancelled. The source system is still running.")
}

pub(crate) struct LiveBoundary(State);
impl LiveBoundary {
    pub(crate) fn open() -> Result<Self, LifecycleError> {
        require_root()?;
        if !private_live()? {
            return Err(LifecycleError::Unsafe(
                "This action requires private Alpine Live",
            ));
        }
        Ok(Self(
            State::open(Path::new(LIVE_STATE), true, true)?.expect("create enabled"),
        ))
    }
    pub(crate) fn begin(&self) -> Result<bool, LifecycleError> {
        let _guard = self.0.lock("state.lock", true)?;
        match self.0.read_record()? {
            None => self.0.write_record(&Record::new(Phase::LiveReady)?)?,
            Some(record) if record.phase == Phase::Cancelled => return Ok(false),
            _ => {
                return Err(LifecycleError::Unsafe(
                    "Alpine Live already has an installation in progress",
                ));
            }
        }
        Ok(true)
    }
    pub(crate) fn start_erasure(&self) -> Result<bool, LifecycleError> {
        let _guard = self.0.lock("state.lock", true)?;
        let mut record = self.0.read_record()?.ok_or(LifecycleError::Unsafe(
            "Missing Alpine Live lifecycle state",
        ))?;
        match record.phase {
            Phase::LiveReady => record.phase = Phase::Destructive,
            Phase::Cancelled => return Ok(false),
            _ => {
                return Err(LifecycleError::Unsafe(
                    "Alpine Live disk erasure was already entered",
                ));
            }
        }
        self.0.write_record(&record)?;
        Ok(true)
    }
    pub(crate) fn installed(&self) -> Result<(), LifecycleError> {
        let _guard = self.0.lock("state.lock", true)?;
        let mut record = self
            .0
            .read_record()?
            .filter(|record| record.phase == Phase::Destructive)
            .ok_or(LifecycleError::Unsafe(
                "Unexpected Alpine Live installation state",
            ))?;
        record.phase = Phase::Installed;
        self.0.write_record(&record)
    }
    pub(crate) fn cancel(&self) -> Result<(), LifecycleError> {
        let _guard = self.0.lock("state.lock", true)?;
        let mut record = match self.0.read_record()? {
            Some(record) => record,
            None => Record::new(Phase::LiveReady)?,
        };
        match record.phase {
            Phase::LiveReady => {
                record.phase = Phase::Cancelled;
                self.0.write_record(&record)
            }
            Phase::Cancelled => Ok(()),
            _ => Err(LifecycleError::Unsafe(
                "Disk erasure already started. Lossless reset is unavailable. Use the Alpine Live rescue environment.",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct Fixture(PathBuf, State);
    impl Fixture {
        fn new(live: bool) -> Self {
            let path = std::env::temp_dir().join(format!(
                "rustaller-lifecycle-test-{}",
                kernel_id("/proc/sys/kernel/random/uuid").unwrap(),
            ));
            DirBuilder::new().mode(0o700).create(&path).unwrap();
            let state = State::open(&path, live, false).unwrap().unwrap();
            Self(path, state)
        }
        fn prepare(&self) -> Preparation {
            Preparation::begin_in(State::open(&self.0, false, false).unwrap().unwrap()).unwrap()
        }
        fn reset(&self) -> Result<&'static str, LifecycleError> {
            reset_original(
                &self.1,
                &kernel_id("/proc/sys/kernel/random/boot_id").unwrap(),
            )
        }
        fn live(&self) -> LiveBoundary {
            LiveBoundary(State::open(&self.0, true, false).unwrap().unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn prepared_reset_removes_only_its_recorded_workspace_and_is_idempotent() {
        let fixture = Fixture::new(false);
        let prepared = fixture.prepare();
        let workspace = prepared.workspace();
        let unrelated = fixture.0.join("unrelated");
        fs::write(&unrelated, b"keep").unwrap();
        symlink(&unrelated, workspace.join("link-to-outside")).unwrap();
        prepared.finish().unwrap();
        drop(prepared);
        assert!(fixture.reset().unwrap().contains("cancelled"));
        assert!(!workspace.exists());
        assert_eq!(fs::read(unrelated).unwrap(), b"keep");
        assert_eq!(fixture.reset().unwrap(), "No pending Rustaller operation");
    }

    #[test]
    fn active_preparation_cancellation_uses_the_operation_lock() {
        let fixture = Fixture::new(false);
        let preparation = fixture.prepare();
        let other = State::open(&fixture.0, false, false).unwrap().unwrap();
        assert!(Preparation::begin_in(other).is_err());
        assert!(fixture.reset().unwrap().contains("requested"));
        assert!(preparation.workspace().exists());
        let mut runner = crate::command::MockRunner::empty();
        let mut runner = CancelAware(&preparation, &mut runner);
        assert!(runner.run(&CommandSpec::new("test", ["unused"])).is_err());
        assert!(preparation.finish().is_err());
        preparation.discard().unwrap();
        drop(preparation);
        assert_eq!(fixture.reset().unwrap(), "No pending Rustaller operation");
    }

    #[test]
    fn recorded_handoff_is_left_alone_until_a_new_kernel_boot() {
        let fixture = Fixture::new(false);
        let preparation = fixture.prepare();
        let workspace = preparation.workspace();
        let mut record = preparation.current().unwrap();
        drop(preparation);
        record.phase = Phase::Handoff;
        fixture.1.write_record(&record).unwrap();
        assert!(fixture.reset().is_err());
        assert!(workspace.is_dir());
        assert!(reset_original(&fixture.1, "not the source boot").is_ok());
        assert!(!workspace.exists());
    }

    #[test]
    fn malformed_or_untrusted_records_and_workspace_mounts_fail_closed() {
        let fixture = Fixture::new(false);
        let preparation = fixture.prepare();
        let workspace = preparation.workspace();
        let record = preparation.current().unwrap();
        drop(preparation);
        let escaped = format!("{SCHEMA} ../../outside {} prepared\n", record.boot);
        atomic_replace(&fixture.0.join("state"), escaped.as_bytes(), 0o600).unwrap();
        assert!(fixture.reset().is_err());
        fixture.1.write_record(&record).unwrap();
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(fixture.reset().is_err());
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(includes_mount(
            &workspace,
            &format!(
                "25 21 0:3 / {} a - tmpfs tmpfs rw",
                workspace.join("inside").display(),
            )
        ));
        assert!(!includes_mount(
            &workspace,
            &format!(
                "25 21 0:3 / {}-elsewhere a - tmpfs tmpfs rw",
                workspace.display(),
            )
        ));
        fixture.reset().unwrap();
    }

    #[test]
    fn live_cancellation_and_erasure_are_mutually_exclusive() {
        let cancelled = Fixture::new(true);
        let live = cancelled.live();
        live.cancel().unwrap();
        live.cancel().unwrap();
        assert!(!live.begin().unwrap());
        assert!(!live.start_erasure().unwrap());
        assert!(live.installed().is_err());

        let irreversible = Fixture::new(true);
        let live = irreversible.live();
        assert!(live.begin().unwrap());
        assert!(live.start_erasure().unwrap());
        assert!(live.cancel().is_err());
        live.installed().unwrap();
        assert!(live.cancel().is_err());
    }
}
