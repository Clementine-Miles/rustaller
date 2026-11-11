use reinstall_rs::worker::args::{WorkerAction, parse_worker_args};

#[test]
fn worker_accepts_only_known_actions_and_validates_initrd_arguments() {
    assert_eq!(parse_worker_args(std::iter::empty::<&str>()).unwrap(), None);
    assert_eq!(parse_worker_args(["--help"]).unwrap(), None);
    assert_eq!(
        parse_worker_args(["live-modules"]).unwrap(),
        Some(WorkerAction::LiveModules)
    );
    assert!(parse_worker_args(["live-modules", "/arbitrary/image"]).is_err());
    assert_eq!(
        parse_worker_args(["firstboot-fix"]).unwrap(),
        Some(WorkerAction::FirstbootFix)
    );
    assert_eq!(
        parse_worker_args(["cleanup-openrc"]).unwrap(),
        Some(WorkerAction::CleanupOpenRc)
    );
    assert_eq!(
        parse_worker_args(["cleanup-systemd"]).unwrap(),
        Some(WorkerAction::CleanupSystemd)
    );
    let network = parse_worker_args([
        "initrd-network",
        "00:11:22:33:44:55",
        "198.51.100.8/24",
        "198.51.100.1",
        "2001:db8::8/64",
        "2001:db8::1",
        "",
    ])
    .unwrap();
    let Some(WorkerAction::InitrdNetwork(config)) = network else {
        panic!("expected validated initrd network action");
    };
    assert_eq!(config.mac_addr(), "00:11:22:33:44:55");
    assert!(parse_worker_args(["initrd-network", "not-a-mac", "", "", "", "", ""]).is_err());
    assert!(parse_worker_args(["firstboot-fix", "extra"]).is_err());
    assert!(parse_worker_args(["unknown"]).is_err());
    assert_eq!(
        parse_worker_args([
            "live-install",
            "/configs/install.plan",
            "/configs/deployment.manifest"
        ])
        .unwrap(),
        Some(WorkerAction::LiveInstall {
            plan: "/configs/install.plan".into(),
            manifest: "/configs/deployment.manifest".into()
        })
    );
    for args in [
        vec!["live-install", "relative", "/configs/m"],
        vec!["live-install", "/configs/p", "/configs/p"],
        vec!["live-install", "/configs/../p", "/configs/m"],
        vec!["live-install", "/configs/p", "/configs/m", "extra"],
    ] {
        assert!(parse_worker_args(args).is_err());
    }
}
