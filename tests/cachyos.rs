use rustaller::command::{CommandOutput, MockRunner};
use rustaller::targets::cachyos::{self, IsaLevel, RELEASE_FINGERPRINT};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustaller-cachyos-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("etc")).unwrap();
        fs::write(path.join("etc/arch-release"), "").unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn ok() -> CommandOutput {
    CommandOutput::new(Some(0), vec![], vec![])
}
#[test]
fn isa_selection_uses_os_usable_glibc_hwcaps_instead_of_cpu_flags() {
    assert_eq!(
        cachyos::isa_level("x86-64-v4 (supported)\nx86-64-v3 (supported, searched)\n"),
        IsaLevel::V3
    );
    assert_eq!(
        cachyos::isa_level("x86-64-v4 (supported, searched)\nx86-64-v3 (supported, searched)"),
        IsaLevel::V4
    );
    assert_eq!(
        cachyos::isa_level("x86-64-v2 (supported, searched)"),
        IsaLevel::V2
    );
    assert_eq!(
        cachyos::isa_level("AVX512 avx2 x86-64-v4\n"),
        IsaLevel::Baseline
    );
}
#[test]
fn release_key_matches_exactly_one_pinned_primary_not_a_short_id_or_subkey() {
    let good = format!(
        "pub:::::::::\nfpr:::::::::{RELEASE_FINGERPRINT}:\nsub:::::::::\nfpr:::::::::OTHER:\n"
    );
    assert!(cachyos::valid_primary_key(&good));
    assert!(!cachyos::valid_primary_key(
        &good.replace(RELEASE_FINGERPRINT, "F3B607488DB35A47")
    ));
    assert!(!cachyos::valid_primary_key(&format!(
        "{good}pub:::::::::\nfpr:::::::::OTHER:\n"
    )));
    assert!(!cachyos::valid_primary_key(&format!(
        "sub:::::::::\nfpr:::::::::{RELEASE_FINGERPRINT}:\n"
    )));
}
#[test]
fn audited_official_script_is_noninteractive_and_uses_pinned_full_key() {
    let original = include_str!("fixtures/cachyos-repo-script.txt");
    let adapted = cachyos::adapt_official_script(original).unwrap();
    assert!(adapted.contains("pacman-key --add ./rustaller-cachyos.asc"));
    assert!(adapted.contains(&format!("pacman-key --lsign-key {RELEASE_FINGERPRINT}")));
    assert!(!adapted.contains("--recv-keys"));
    assert!(adapted.contains("pacman --noconfirm -U"));
    assert!(adapted.contains("pacman --noconfirm -Syu"));
    assert!(!adapted.contains("gawk -i inplace -f $gawk_script $pacman_conf_cachyos || true"));
    assert!(
        cachyos::adapt_official_script(
            &original.replace("    pacman -Syu\n", "    pacman -Syyu\n")
        )
        .is_err()
    );
}
#[test]
fn repo_policy_rejects_signature_downgrades_and_unsupported_isa() {
    let v3 = "[options]\nSigLevel = Required DatabaseOptional\n[cachyos-v3]\n[cachyos-core-v3]\n[cachyos-extra-v3]\n[cachyos]\n[core]\n";
    assert!(cachyos::validate_repository_config(v3, IsaLevel::V3).is_ok());
    assert!(cachyos::validate_repository_config(v3, IsaLevel::V2).is_err());
    assert!(cachyos::validate_repository_config(v3, IsaLevel::V4).is_err());
    assert!(
        cachyos::validate_repository_config(
            &v3.replace("Required DatabaseOptional", "Never"),
            IsaLevel::V3
        )
        .is_err()
    );
    assert!(
        cachyos::validate_repository_config(
            &format!("{v3}SigLevel = Optional TrustAll\n"),
            IsaLevel::V3
        )
        .is_err()
    );
}
#[test]
fn firstboot_failure_retains_pending_update_and_success_rebuilds_then_cleans() {
    for failure in 0..3 {
        let root = Root::new();
        let worker = format!("fake-test-ELF {}", rustaller::RUNTIME_VERSION);
        cachyos::install_firstboot_update_with_worker(&root.0, worker.as_bytes()).unwrap();
        let mut outputs = (0..failure).map(|_| ok()).collect::<Vec<_>>();
        outputs.push(CommandOutput::new(Some(1), vec![], vec![]));
        let mut runner = MockRunner::new(outputs);
        assert!(cachyos::run_firstboot_update(&mut runner, &root.0).is_err());
        assert!(
            root.0
                .join("var/lib/rustaller/cachyos-firstboot.pending")
                .is_file()
        );
        assert!(
            root.0
                .join("usr/local/libexec/rustaller-cachyos-worker")
                .is_file()
        );
        assert_eq!(runner.calls().len(), failure + 1);
        let mut retry = MockRunner::new([ok(), ok(), ok()]);
        cachyos::run_firstboot_update(&mut retry, &root.0).unwrap();
        assert_eq!(
            retry
                .calls()
                .iter()
                .map(|c| c.phase.as_str())
                .collect::<Vec<_>>(),
            [
                "cachyos-firstboot-full-upgrade",
                "cachyos-firstboot-initramfs",
                "cachyos-firstboot-grub"
            ]
        );
        assert!(
            !root
                .0
                .join("var/lib/rustaller/cachyos-firstboot.pending")
                .exists()
        );
        assert!(
            !root
                .0
                .join("usr/local/libexec/rustaller-cachyos-worker")
                .exists()
        );
    }
}
