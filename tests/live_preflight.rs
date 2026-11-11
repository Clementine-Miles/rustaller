use reinstall_rs::codec::encode_plan;
use reinstall_rs::command::{CommandOutput, MockRunner};
use reinstall_rs::model::{InstallPlan, Target};
use reinstall_rs::worker::live::{
    LivePreflightError, load_and_validate_plan_with_uid, validate_live_plan_with_uid,
};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "reinstall-live-preflight-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_plan(path: &std::path::Path, target: Target) {
    let encoded = encode_plan(&plan(target)).unwrap();
    fs::write(path, encoded).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn plan(target: Target) -> InstallPlan {
    let mut plan = InstallPlan::new(target);
    if target.is_automatic() {
        plan.confhome = Some("https://assets.example/reinstall".into());
    }
    plan
}

#[test]
fn live_ram_gate_runs_for_each_automatic_target_before_workflow_creation() {
    for (target, floor) in [
        (Target::Alpine, 512),
        (Target::Debian, 512),
        (Target::Arch, 1024),
        (Target::Gentoo, 2048),
    ] {
        assert!(
            validate_live_plan_with_uid(
                plan(target),
                "x86_64",
                floor,
                reinstall_rs::RUNTIME_VERSION,
                0,
            )
            .is_ok()
        );
        assert!(matches!(
            validate_live_plan_with_uid(
                plan(target),
                "x86_64",
                floor - 1,
                reinstall_rs::RUNTIME_VERSION,
                0
            ),
            Err(LivePreflightError::InsufficientRam(_))
        ));
    }
}

#[test]
fn live_worker_reads_private_plan_and_checks_uname_then_ram_without_disk_commands() {
    let scratch = Scratch::new();
    let plan_path = scratch.0.join("install.plan");
    write_plan(&plan_path, Target::Arch);
    let mut runner = MockRunner::new([
        CommandOutput::new(Some(0), b"x86_64\n".to_vec(), Vec::new()),
        CommandOutput::new(
            Some(0),
            b" total used free shared buff/cache available\nMem: 2048 100 1000 0 948 1800\nSwap: 0 0 0\n".to_vec(),
            Vec::new(),
        ),
    ]);
    let validated =
        load_and_validate_plan_with_uid(&mut runner, &plan_path, reinstall_rs::RUNTIME_VERSION, 0)
            .unwrap();
    assert_eq!(validated.plan().target, Target::Arch);
    assert!(!plan_path.exists(), "one-shot plan is consumed on read");
    assert_eq!(runner.calls().len(), 2);
    assert_eq!(runner.calls()[0].program, "uname");
    assert_eq!(runner.calls()[0].args, ["-m"]);
    assert_eq!(runner.calls()[1].program, "free");
    assert_eq!(runner.calls()[1].args, ["-m"]);
}

#[test]
fn live_worker_rechecks_ram_and_stops_before_any_disk_command() {
    let scratch = Scratch::new();
    let plan_path = scratch.0.join("install.plan");
    write_plan(&plan_path, Target::Gentoo);
    let mut runner = MockRunner::new([
        CommandOutput::new(Some(0), b"x86_64\n".to_vec(), Vec::new()),
        CommandOutput::new(
            Some(0),
            b" total used free shared buff/cache available\nMem: 2047 100 1000 0 947 1800\nSwap: 0 0 0\n".to_vec(),
            Vec::new(),
        ),
    ]);
    assert!(matches!(
        load_and_validate_plan_with_uid(&mut runner, &plan_path, reinstall_rs::RUNTIME_VERSION, 0),
        Err(LivePreflightError::InsufficientRam(_))
    ));
    assert_eq!(runner.calls().len(), 2);
    assert_eq!(runner.calls()[0].program, "uname");
    assert_eq!(runner.calls()[1].program, "free");
    assert!(!plan_path.exists());
}

#[test]
fn live_worker_refuses_non_root_before_reading_plan_or_running_utilities() {
    let mut runner = MockRunner::empty();
    assert!(matches!(
        load_and_validate_plan_with_uid(
            &mut runner,
            std::path::Path::new("/must-not-be-read"),
            reinstall_rs::RUNTIME_VERSION,
            1000
        ),
        Err(LivePreflightError::RootRequired)
    ));
    assert!(runner.calls().is_empty());
}

#[test]
fn live_preflight_rejects_non_root_before_any_workflow_can_start() {
    assert!(matches!(
        validate_live_plan_with_uid(
            plan(Target::Alpine),
            "x86_64",
            512,
            reinstall_rs::RUNTIME_VERSION,
            1000
        ),
        Err(LivePreflightError::RootRequired)
    ));
}

#[test]
fn live_preflight_rejects_wrong_architecture_target_and_runtime_version() {
    assert!(matches!(
        validate_live_plan_with_uid(
            plan(Target::Alpine),
            "aarch64",
            2048,
            reinstall_rs::RUNTIME_VERSION,
            0
        ),
        Err(LivePreflightError::UnsupportedArchitecture(_))
    ));
    assert!(matches!(
        validate_live_plan_with_uid(
            plan(Target::Reset),
            "x86_64",
            0,
            reinstall_rs::RUNTIME_VERSION,
            0
        ),
        Err(LivePreflightError::NonAutomaticTarget(Target::Reset))
    ));
    assert!(matches!(
        validate_live_plan_with_uid(plan(Target::Alpine), "x86_64", 512, "old-version", 0),
        Err(LivePreflightError::RuntimeVersionMismatch { .. })
    ));
}
