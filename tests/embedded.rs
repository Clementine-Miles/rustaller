#[test]
fn debug_host_embeds_explicit_x86_64_worker_fixture_with_current_runtime_marker() {
    let worker = reinstall_rs::worker_bytes();
    assert_eq!(&worker[..4], b"\x7fELF");
    assert_eq!(worker[4], 2);
    assert_eq!(worker[5], 1);
    assert_eq!(&worker[18..20], &[0x3e, 0]);
    assert!(
        worker
            .windows(reinstall_rs::RUNTIME_VERSION.len())
            .any(|window| window == reinstall_rs::RUNTIME_VERSION.as_bytes())
    );
}
