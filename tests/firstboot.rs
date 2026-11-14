mod support;
use rustaller::command::{CommandOutput, MockRunner};
use rustaller::worker::firstboot::{
    apply_network_fixes_from_links, cleanup_firstboot, eth_names_by_mac, install_firstboot_service,
    lookup_ethx_by_mac, non_loopback_links, render_firstboot_service, rewrite_ifupdown,
    wait_for_stable_links,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;

use crate::support::Scratch;

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
    let mut responses = vec![CommandOutput::new(
        Some(0),
        links.as_bytes().to_vec(),
        Vec::new(),
    )];
    for _ in 0..10 {
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
    assert_eq!(runner.calls().len(), 21);
    assert_eq!(runner.calls()[0].program, "ip");
    assert_eq!(runner.calls()[1].program, "sleep");
    assert_eq!(runner.calls()[20].program, "ip");
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
fn mac_lookup_uses_native_rust_parsing_and_checks_ip_failures_and_ambiguity() {
    let mut runner = MockRunner::new([CommandOutput::new(
        Some(0),
        IP_LINKS.as_bytes().to_vec(),
        Vec::new(),
    )]);
    assert_eq!(
        lookup_ethx_by_mac(&mut runner, "00:11:22:33:44:55", false).unwrap(),
        Some("ens3".into())
    );
    assert_eq!(runner.calls().len(), 1);
    assert_eq!(runner.calls()[0].program, "ip");
    let mut failed = MockRunner::new([CommandOutput::new(Some(1), Vec::new(), Vec::new())]);
    assert!(lookup_ethx_by_mac(&mut failed, "00:11:22:33:44:55", false).is_err());
    let duplicate = format!("{IP_LINKS}5: eth9: <UP> link/ether 00:11:22:33:44:55\n");
    let mut ambiguous = MockRunner::new([CommandOutput::new(
        Some(0),
        duplicate.into_bytes(),
        Vec::new(),
    )]);
    assert!(lookup_ethx_by_mac(&mut ambiguous, "00:11:22:33:44:55", false).is_err());
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
fn alpine_filesystem_rewrite_uses_mac_identity_and_is_idempotent() {
    let scratch = Scratch::new("firstboot");
    let interfaces = scratch.0.join("etc/network/interfaces");
    fs::create_dir_all(interfaces.parent().unwrap()).unwrap();
    fs::write(&interfaces, "# mac 00:11:22:33:44:55\nauto eth0\n").unwrap();
    apply_network_fixes_from_links(&scratch.0, IP_LINKS).unwrap();
    assert_eq!(fs::read(&interfaces).unwrap(), b"auto ens3\n");
    apply_network_fixes_from_links(&scratch.0, IP_LINKS).unwrap();
    assert_eq!(fs::read(&interfaces).unwrap(), b"auto ens3\n");
}

#[test]
fn openrc_service_installs_matching_worker_and_only_native_integration_glue() {
    let scratch = Scratch::new("service-openrc");
    fs::create_dir_all(&scratch.0).unwrap();
    let mut runner = MockRunner::new([CommandOutput::new(Some(0), vec![], vec![])]);
    let worker = format!("ELF {}", rustaller::RUNTIME_VERSION);
    install_firstboot_service(&mut runner, &scratch.0, worker.as_bytes()).unwrap();
    let path = scratch.0.join("etc/init.d/fix-eth-name");
    let service = fs::read_to_string(&path).unwrap();
    assert_eq!(service, render_firstboot_service());
    assert!(service.starts_with("#!/sbin/openrc-run\n"));
    assert!(service.contains("before net networking dhcpcd"));
    assert!(service.contains("/usr/local/bin/rustaller-worker firstboot-fix"));
    assert!(service.contains("/usr/local/bin/rustaller-worker cleanup-openrc"));
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::read(scratch.0.join("usr/local/bin/rustaller-worker")).unwrap(),
        worker.as_bytes()
    );
    assert_eq!(runner.calls().len(), 1);
    assert_eq!(runner.calls()[0].program, "chroot");
    assert_eq!(
        runner.calls()[0].args[1..],
        ["rc-update", "add", "fix-eth-name", "boot"]
    );
}

#[test]
fn mismatched_worker_version_is_rejected_before_writing_service_files() {
    let scratch = Scratch::new("mismatched-worker");
    let mut runner = MockRunner::empty();
    assert!(
        install_firstboot_service(&mut runner, &scratch.0, b"ELF without the current version",)
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

    cleanup_firstboot(&mut runner, root).expect("cleanup one-shot service");

    assert_eq!(runner.calls().len(), 2);
    assert_eq!(runner.calls()[0].program, "rc-service");
    assert_eq!(runner.calls()[0].args, ["fix-eth-name", "zap"]);
    assert_eq!(runner.calls()[1].program, "rc-update");
    assert_eq!(runner.calls()[1].args, ["del", "fix-eth-name", "boot"]);
    assert!(!root.join("etc/init.d/fix-eth-name").exists());
    assert!(!root.join("usr/local/bin/rustaller-worker").exists());
}

#[test]
fn firstboot_rejects_symlinks_hardlinks_and_missing_macs_without_publication() {
    use std::os::unix::fs::symlink;
    for scenario in ["symlink", "hardlink", "missing", "absent"] {
        let scratch = Scratch::new(scenario);
        fs::create_dir_all(scratch.0.join("etc/network")).unwrap();
        let outside = scratch.0.join("outside");
        fs::write(&outside, "unchanged\n").unwrap();
        let interfaces = scratch.0.join("etc/network/interfaces");
        match scenario {
            "symlink" => symlink(&outside, &interfaces).unwrap(),
            "hardlink" => fs::hard_link(&outside, &interfaces).unwrap(),
            "absent" => {}
            _ => fs::write(
                &interfaces,
                "# mac 52:54:00:99:99:99\nauto eth0\niface eth0 inet dhcp\n",
            )
            .unwrap(),
        }
        assert!(
            apply_network_fixes_from_links(&scratch.0, IP_LINKS).is_err(),
            "{scenario}"
        );
        assert_eq!(fs::read_to_string(&outside).unwrap(), "unchanged\n");
        if scenario == "missing" {
            assert!(fs::read_to_string(&interfaces).unwrap().contains("# mac "));
        }
    }
}
