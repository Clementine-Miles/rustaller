use rustaller::host::hostname::{default_hostname, detect_hostname, resolve_install_hostname};
use rustaller::model::{InstallPlan, Target};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs, path::PathBuf};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustaller-hostname-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("etc")).unwrap();
        fs::create_dir_all(path.join("proc/sys/kernel")).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn source_hostname_is_bounded_validated_and_preferred_to_runtime_hostname() {
    let dir = Directory::new();
    fs::write(dir.0.join("etc/hostname"), "previous-vps.example.org\n").unwrap();
    fs::write(dir.0.join("proc/sys/kernel/hostname"), "running-vps\n").unwrap();
    assert_eq!(detect_hostname(&dir.0).as_deref(), Some("previous-vps"));
    for invalid in [
        "localhost\n".into(),
        "bad name".into(),
        "safe.bad/domain".into(),
        "x".repeat(257),
    ] {
        fs::write(dir.0.join("etc/hostname"), invalid).unwrap();
        assert_eq!(detect_hostname(&dir.0).as_deref(), Some("running-vps"));
    }
}

#[test]
fn missing_hostname_uses_distribution_and_explicit_values_and_rescue_are_preserved() {
    let dir = Directory::new();
    for (target, expected) in [
        (Target::Alpine, "alpine"),
        (Target::Debian, "debian"),
        (Target::Arch, "arch"),
        (Target::Gentoo, "gentoo"),
    ] {
        assert_eq!(default_hostname(target, &dir.0), expected);
        let mut plan = InstallPlan::new(target);
        resolve_install_hostname(&mut plan, &dir.0);
        assert_eq!(plan.post_install.hostname.as_deref(), Some(expected));
        plan.post_install.hostname = Some("chosen".into());
        resolve_install_hostname(&mut plan, &dir.0);
        assert_eq!(plan.post_install.hostname.as_deref(), Some("chosen"));
    }
    let mut rescue = InstallPlan::new(Target::Alpine);
    rescue.hold = Some(1);
    resolve_install_hostname(&mut rescue, &dir.0);
    assert!(rescue.post_install.hostname.is_none());
}
