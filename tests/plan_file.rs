use rustaller::codec::encode_plan;
use rustaller::model::{InstallPlan, SecretString, Target};
use rustaller::plan_file::PrivatePlanFile;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "rustaller-plan-file-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create unique temporary directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn private_plan_file_is_atomic_private_readable_and_removed_on_drop() {
    let scratch = Scratch::new();
    let path = scratch.0.join("install.plan");
    let mut plan = InstallPlan::new(Target::Alpine);
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.password = Some(SecretString::new("plan-secret-sentinel"));

    {
        let file = PrivatePlanFile::create(&path, &plan).expect("create private plan");
        assert_eq!(file.path(), path);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let decoded = file.read().expect("read plan");
        assert_eq!(decoded.target, Target::Alpine);
        assert_eq!(
            decoded.password.as_ref().map(SecretString::expose),
            Some("plan-secret-sentinel")
        );
    }
    assert!(
        !path.exists(),
        "plan should be removed when its guard drops"
    );
}

#[test]
fn plan_file_can_be_explicitly_persisted_for_initrd_handoff() {
    let scratch = Scratch::new();
    let path = scratch.0.join("install.plan");
    let mut plan = InstallPlan::new(Target::Alpine);
    plan.confhome = Some("https://assets.example/reinstall".into());
    let mut private_file = PrivatePlanFile::create(&path, &plan).expect("create plan");

    assert_eq!(private_file.persist(), path);
    drop(private_file);
    assert!(path.is_file());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        PrivatePlanFile::open(&path).unwrap().read().unwrap().target,
        Target::Alpine
    );
    assert!(!path.exists(), "opening persisted plan still consumes it");
}

#[test]
fn opening_insecure_plan_does_not_delete_it_and_valid_open_consumes_it_on_drop() {
    let scratch = Scratch::new();
    let path = scratch.0.join("handoff.plan");
    let plan = InstallPlan::new(Target::Reset);
    let encoded = encode_plan(&plan).unwrap();
    fs::write(&path, encoded).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(PrivatePlanFile::open(&path).is_err());
    assert!(
        path.exists(),
        "rejecting the file must not delete its owner’s data"
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    {
        let file = PrivatePlanFile::open(&path).expect("open private plan");
        assert_eq!(file.read().unwrap().target, Target::Reset);
    }
    assert!(!path.exists(), "opened one-shot plan is consumed on drop");
}

#[test]
fn refuses_to_replace_existing_plan_or_create_invalid_plan() {
    let scratch = Scratch::new();
    let path = scratch.0.join("install.plan");
    fs::write(&path, b"preserve existing data").unwrap();
    let mut plan = InstallPlan::new(Target::Alpine);
    plan.confhome = Some("https://assets.example/reinstall".into());

    assert!(PrivatePlanFile::create(&path, &plan).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"preserve existing data");

    let invalid_path = scratch.0.join("invalid.plan");
    let mut invalid = InstallPlan::new(Target::Alpine);
    invalid.schema_version += 1;
    assert!(PrivatePlanFile::create(&invalid_path, &invalid).is_err());
    assert!(!invalid_path.exists());
}
