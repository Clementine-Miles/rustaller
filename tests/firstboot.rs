use rustaller::command::{CommandOutput, MockRunner};
use rustaller::worker::firstboot::{
    InitManager, apply_network_fixes_from_links, cleanup_firstboot, eth_names_by_mac,
    install_firstboot_service, lookup_ethx_by_mac, non_loopback_links, render_firstboot_service,
    rewrite_ifupdown, rewrite_networkd_file, wait_for_stable_links,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        Self(std::env::temp_dir().join(format!("rustaller-{label}-{}-{nonce}", std::process::id())))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const IP_LINKS: &str = "2: ens3: <UP> link/ether 00:11:22:33:44:55\n3: enp5s0: <UP> link/ether 00:11:22:33:44:66\n4: ens7: <UP> master ens3 link/ether 00:11:22:33:44:55\n";

#[test]
fn stability_input_excludes_only_the_loopback_interface() {
    assert_eq!(
        non_loopback_links("1: lo: <LOOPBACK>\n2: ens3: <UP>\n"),
        "2: ens3: <UP>\n"
    );
}

#[test]
fn waits_for_ten_consecutive_stable_ip_link_states() {
    let links = "1: lo: <LOOPBACK>\n2: ens3: <UP> link/ether 00:11:22:33:44:55\n";
    let filtered_links = non_loopback_links(links);
    let mut responses = vec![
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        CommandOutput::new(Some(0), filtered_links.as_bytes().to_vec(), Vec::new()),
    ];
    for _ in 0..10 {
        responses.push(CommandOutput::new(Some(0), Vec::new(), Vec::new()));
        responses.push(CommandOutput::new(Some(0), Vec::new(), Vec::new()));
        responses.push(CommandOutput::new(
            Some(0),
            filtered_links.as_bytes().to_vec(),
            Vec::new(),
        ));
    }
    let mut runner = MockRunner::new(responses);
    let stable = wait_for_stable_links(&mut runner).expect("stable NIC state");
    assert_eq!(stable, filtered_links.trim_end_matches(char::from(10)));
    assert_eq!(runner.calls().len(), 32);
    assert_eq!(runner.calls()[0].program, "ip");
    assert_eq!(runner.calls()[1].program, "awk");
    assert_eq!(runner.calls()[1].args[0], r#"$2 != "lo:""#);
    assert_eq!(runner.calls()[2].program, "sleep");
    assert_eq!(runner.calls()[31].program, "awk");
}

#[test]
fn mac_lookup_preserves_master_and_azure_slave_rules() {
    assert_eq!(
        eth_names_by_mac(IP_LINKS, "00:11:22:33:44:55", false),
        ["ens3"]
    );
    assert_eq!(
        eth_names_by_mac(IP_LINKS, "00:11:22:33:44:55", true),
        ["ens7"]
    );
    assert_eq!(
        eth_names_by_mac(IP_LINKS, "AA:BB:CC:DD:EE:FF", false),
        Vec::<String>::new()
    );
}

#[test]
fn mac_lookup_retains_the_original_tr_grep_awk_cut_pipeline() {
    let mut lower_mac = b"00:11:22:33:44:55".to_vec();
    lower_mac.push(10);
    let mut responses = vec![CommandOutput::new(Some(0), lower_mac, Vec::new())];
    for index in 0..6 {
        let output = if index == 5 {
            let mut name = b"ens3".to_vec();
            name.push(10);
            name
        } else {
            Vec::new()
        };
        responses.push(CommandOutput::new(Some(0), output, Vec::new()));
    }
    let mut runner = MockRunner::new(responses);

    assert_eq!(
        lookup_ethx_by_mac(&mut runner, "00:11:22:33:44:55", false).unwrap(),
        Some("ens3".into())
    );
    let calls = runner.calls();
    assert_eq!(calls.len(), 7);
    assert_eq!(
        calls
            .iter()
            .map(|call| call.program.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["tr", "ip", "grep", "grep", "awk", "cut", "grep"]
    );
    assert_eq!(calls[1].args, ["-o", "link"]);
    assert_eq!(calls[2].args, ["-i", "00:11:22:33:44:55"]);
    assert_eq!(calls[3].args, ["-v", "master"]);
    assert_eq!(calls[4].args, ["{print $2}"]);
    assert_eq!(calls[5].args, ["-d:", "-f1"]);
    assert_eq!(calls[6].args, ["."]);
}

#[test]
fn ifupdown_rewrite_matches_existing_firstboot_fixture_byte_for_byte() {
    let input = "auto lo\niface lo inet loopback\n# mac 00:11:22:33:44:55\nauto eth0\niface eth0 inet static\n    address 192.0.2.1/32\niface eth0 inet6 static\n    address 2001:db8::1/128\n    post-up ip route add default via 2001:db8::ff dev eth0\n# mac 00:11:22:33:44:66\nallow-hotplug eth1\niface eth1 inet dhcp\n";
    let expected = "auto lo\niface lo inet loopback\nauto ens3\niface ens3 inet static\n    address 192.0.2.1/32\niface ens3 inet6 static\n    address 2001:db8::1/128\n    post-up ip route add default via 2001:db8::ff dev ens3\nallow-hotplug enp5s0\niface enp5s0 inet dhcp\n";

    let output = rewrite_ifupdown(input, |mac| {
        if mac.ends_with("55") {
            Some("ens3".into())
        } else if mac.ends_with("66") {
            Some("enp5s0".into())
        } else {
            None
        }
    });
    assert_eq!(output, expected.as_bytes());
}

#[test]
fn filesystem_rewrite_updates_only_target_configs_and_renames_networkd_files() {
    let scratch = Scratch::new("firstboot");
    let root = &scratch.0;
    let interfaces = root.join("etc/network/interfaces");
    fs::create_dir_all(interfaces.parent().unwrap()).expect("create ifupdown directory");
    fs::write(&interfaces, "# mac 00:11:22:33:44:55\nauto eth0\n").expect("write config");

    let networkd = root.join("etc/systemd/network");
    fs::create_dir_all(&networkd).expect("create networkd directory");
    fs::write(
        networkd.join("10-cloud-init-eth0.network"),
        "[Match]\nMACAddress=00:11:22:33:44:55\nName=eth0\n",
    )
    .expect("write networkd config");

    apply_network_fixes_from_links(root, IP_LINKS).expect("apply fixes");
    assert_eq!(fs::read(&interfaces).unwrap(), b"auto ens3\n");
    assert_eq!(
        fs::read(networkd.join("10-ens3.network")).unwrap(),
        b"[Match]\nMACAddress=00:11:22:33:44:55\nName=ens3\n"
    );
    assert!(!networkd.join("10-cloud-init-eth0.network").exists());
}

#[test]
fn systemd_service_installs_worker_and_matches_the_rust_worker_contract() {
    let scratch = Scratch::new("service-systemd");
    let root = &scratch.0;
    fs::create_dir_all(root.join("lib/systemd/system-preset")).unwrap();
    let mut runner = MockRunner::new([CommandOutput::new(Some(0), Vec::new(), Vec::new())]);
    let worker_elf = format!("ELF {}", rustaller::RUNTIME_VERSION);

    install_firstboot_service(
        &mut runner,
        root,
        InitManager::Systemd,
        worker_elf.as_bytes(),
    )
    .expect("install generated systemd service");

    let service = fs::read_to_string(root.join("etc/systemd/system/fix-eth-name.service")).unwrap();
    assert_eq!(service, render_firstboot_service(InitManager::Systemd));
    assert!(service.contains("ExecStart=/usr/local/bin/rustaller-worker firstboot-fix"));
    assert!(service.contains("ExecStartPost=/usr/local/bin/rustaller-worker cleanup-systemd"));
    assert!(!service.contains("fix-eth-name.sh"));
    assert_eq!(
        fs::read(root.join("usr/local/bin/rustaller-worker")).unwrap(),
        worker_elf.as_bytes()
    );
    assert_eq!(
        fs::metadata(root.join("usr/local/bin/rustaller-worker"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        fs::read(root.join("lib/systemd/system-preset/01-fix-eth-name.preset")).unwrap(),
        b"enable fix-eth-name.service\n"
    );
    assert_eq!(runner.calls().len(), 1);
    assert_eq!(runner.calls()[0].program, "chroot");
    assert_eq!(
        runner.calls()[0]
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        [
            root.display().to_string(),
            "systemctl".into(),
            "enable".into(),
            "fix-eth-name.service".into()
        ]
    );
}

#[test]
fn mismatched_worker_version_is_rejected_before_writing_service_files() {
    let scratch = Scratch::new("mismatched-worker");
    let mut runner = MockRunner::empty();
    assert!(
        install_firstboot_service(
            &mut runner,
            &scratch.0,
            InitManager::Systemd,
            b"ELF without the current version",
        )
        .is_err()
    );
    assert!(!scratch.0.join("usr/local/bin/rustaller-worker").exists());
    assert!(runner.calls().is_empty());
}

#[test]
fn openrc_cleanup_runs_service_commands_then_removes_the_one_shot_worker() {
    let scratch = Scratch::new("cleanup-openrc");
    let root = &scratch.0;
    fs::create_dir_all(root.join("etc/init.d")).unwrap();
    fs::create_dir_all(root.join("usr/local/bin")).unwrap();
    fs::write(root.join("etc/init.d/fix-eth-name"), "service").unwrap();
    fs::write(root.join("usr/local/bin/rustaller-worker"), "worker").unwrap();
    let mut runner = MockRunner::new([
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
    ]);

    cleanup_firstboot(&mut runner, root, InitManager::OpenRc).expect("cleanup one-shot service");

    assert_eq!(runner.calls().len(), 2);
    assert_eq!(runner.calls()[0].program, "rc-service");
    assert_eq!(runner.calls()[0].args, ["fix-eth-name", "zap"]);
    assert_eq!(runner.calls()[1].program, "rc-update");
    assert_eq!(runner.calls()[1].args, ["del", "fix-eth-name", "boot"]);
    assert!(!root.join("etc/init.d/fix-eth-name").exists());
    assert!(!root.join("usr/local/bin/rustaller-worker").exists());
}

#[test]
fn networkd_rewrite_changes_only_matching_generated_files() {
    let input = "[Match]\nMACAddress=00:11:22:33:44:55\nName=eth0\n[Network]\nDHCP=yes\n";
    let (name, output) = rewrite_networkd_file("10-cloud-init-eth0.network", input, |mac| {
        (mac == "00:11:22:33:44:55").then(|| "ens3".into())
    })
    .expect("matching generated file");
    assert_eq!(name, "10-ens3.network");
    assert_eq!(
        output,
        "[Match]\nMACAddress=00:11:22:33:44:55\nName=ens3\n[Network]\nDHCP=yes\n"
    );
    assert!(rewrite_networkd_file("10-other.network", input, |_| Some("ens3".into())).is_none());
}
