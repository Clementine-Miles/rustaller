use rustaller::command::{CommandOutput, MockRunner};
use rustaller::model::{BootMode, InstallPlan, Target};
use rustaller::shared::storage::mount_target;
use rustaller::shared::storage::{DiskLayout, PartitionEnvironment, refresh_partition_table};
use rustaller::worker::live::validate_live_plan_with_uid;
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
        let root = std::env::temp_dir().join(format!(
            "rustaller-storage-cleanup-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn plan() -> rustaller::worker::live::ValidatedLivePlan {
    let mut plan = InstallPlan::new(Target::Debian);
    plan.confhome = Some("https://assets.example/reinstall".into());
    validate_live_plan_with_uid(plan, "x86_64", 512, rustaller::RUNTIME_VERSION, 0).unwrap()
}
fn success(count: usize) -> Vec<CommandOutput> {
    (0..count)
        .map(|_| CommandOutput::new(Some(0), vec![], vec![]))
        .collect()
}

#[test]
fn device_link_cleanup_does_not_follow_links_or_remove_dotfiles() {
    let scratch = Scratch::new();
    let links = scratch.0.join("dev/disk");
    fs::create_dir_all(links.join("by-id")).unwrap();
    fs::create_dir(scratch.0.join("outside")).unwrap();
    fs::write(scratch.0.join("outside/keep"), b"keep").unwrap();
    fs::write(links.join(".keep"), b"keep").unwrap();
    std::os::unix::fs::symlink(scratch.0.join("outside"), links.join("by-id/disk")).unwrap();
    std::os::unix::fs::symlink("../../vda1", links.join("by-id/target")).unwrap();
    std::os::unix::fs::symlink("../../vdb1", links.join("by-id/other-disk")).unwrap();
    std::os::unix::fs::symlink("../../vdaa1", links.join("by-id/similar-name")).unwrap();
    let layout = DiskLayout::new(&plan(), "vda", BootMode::Bios, 8 * 1024_u64.pow(3)).unwrap();
    let mut runner = MockRunner::new(success(100));
    refresh_partition_table(
        &mut runner,
        &layout,
        &PartitionEnvironment {
            has_partprobe: false,
            has_partx: false,
            device_links: Some(links.clone()),
        },
    )
    .unwrap();
    assert!(scratch.0.join("outside/keep").is_file());
    assert!(links.join(".keep").is_file());
    assert!(links.join("by-id").is_dir());
    assert!(fs::symlink_metadata(links.join("by-id/target")).is_err());
    for name in ["disk", "other-disk", "similar-name"] {
        assert!(
            fs::symlink_metadata(links.join("by-id").join(name))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[test]
fn mount_target_refuses_symlinked_roots_before_running_mount() {
    let scratch = Scratch::new();
    fs::create_dir(scratch.0.join("outside")).unwrap();
    std::os::unix::fs::symlink(scratch.0.join("outside"), scratch.0.join("os")).unwrap();
    let layout = DiskLayout::new(&plan(), "vda", BootMode::Bios, 8 * 1024_u64.pow(3)).unwrap();
    let mut runner = MockRunner::empty();
    assert!(mount_target(&mut runner, &layout, &scratch.0.join("os/child")).is_err());
    assert!(runner.calls().is_empty());
    assert!(!scratch.0.join("outside/child").exists());
}
