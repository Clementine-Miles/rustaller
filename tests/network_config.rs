//! Complete target output tests. Only disposable directories are mutated.
mod support;
use rustaller::config::{
    AddressPreference, CongestionControl, DnsMode, Ipv6Policy, QueueDiscipline,
};
use rustaller::deployment::{DeploymentManifest, DiskIdentity, NetworkInterface};
use rustaller::model::{BootMode, InstallPlan, Target};
use rustaller::shared::network::{
    apply_network_configuration, render_network_configuration, required_modules,
};
use rustaller::worker::firstboot::rewrite_ifupdown;
use rustaller::worker::live::validate_live_plan_with_uid;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

const KERNEL: &str =
    "CONFIG_TCP_CONG_BBR=m\nCONFIG_NET_SCH_FQ=y\nCONFIG_IPV6=y\nCONFIG_SYN_COOKIES=y\n";
use crate::support::Scratch;
impl Scratch {
    fn apply(&self, plan: InstallPlan, manifest: &DeploymentManifest) -> Result<(), String> {
        let plan = validate_live_plan_with_uid(plan, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0)
            .map_err(|e| e.to_string())?;
        apply_network_configuration(&self.0, &plan, manifest, KERNEL)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn manifest() -> DeploymentManifest {
    DeploymentManifest {
        disk: DiskIdentity {
            device: "/dev/vda".into(),
            size_bytes: 8 * 1024_u64.pow(3),
            serial: "fixture".into(),
            wwn: String::new(),
            partition_table_id: String::new(),
        },
        boot_mode: BootMode::Bios,
        alpine_branch: "v3.24".into(),
        debian_codename: "trixie".into(),
        gentoo_stage3: None,
        network: vec![NetworkInterface {
            name: "ens3".into(),
            mac: "52:54:00:11:22:33".into(),
            ipv4_addresses: vec!["192.0.2.2/32".into()],
            ipv4_gateway: Some("192.0.2.1".into()),
            ipv6_addresses: vec!["2001:db8::2/128".into()],
            ipv6_gateway: Some("fe80::1".into()),
            dhcp4: false,
            dhcp6: false,
            accept_ra: false,
        }],
    }
}
fn static_dns(target: Target) -> InstallPlan {
    let mut plan = InstallPlan::new(target);
    plan.system.dns.mode = DnsMode::Static;
    plan.system.dns.servers = vec![
        "1.1.1.1".parse().unwrap(),
        "2606:4700:4700::1111".parse().unwrap(),
    ];
    plan.system.dns.search_domains = vec!["example.test".into()];
    plan
}

#[test]
fn offline_inventory_validation_cannot_authorize_deployment_or_target_writes() {
    let mut inventory = manifest();
    inventory.alpine_branch.clear();
    inventory.debian_codename.clear();
    let mut plan = InstallPlan::new(Target::Gentoo);
    assert!(inventory.validate_inventory().is_ok());
    assert!(inventory.validate().is_err());
    assert!(rustaller::deployment::encode_manifest(&inventory).is_err());
    assert!(
        rustaller::shared::network::validate_inventory_network_policy(&plan, &inventory).is_ok()
    );
    assert!(render_network_configuration(&plan, &inventory).is_err());
    plan.system.network.ipv6 = Ipv6Policy::Disable;
    inventory.network[0].ipv4_addresses.clear();
    inventory.network[0].ipv4_gateway = None;
    assert!(
        rustaller::shared::network::validate_inventory_network_policy(&plan, &inventory).is_err()
    );
}
fn text(plan: &InstallPlan, manifest: &DeploymentManifest, name: &str) -> String {
    let files = render_network_configuration(plan, manifest).unwrap();
    String::from_utf8(
        files
            .into_iter()
            .find(|file| file.path == Path::new(name))
            .unwrap()
            .contents,
    )
    .unwrap()
}

#[test]
fn native_networkd_final_files_cover_static_dhcp_ra_mtu_and_mac_matching() {
    for target in [Target::Debian, Target::Arch] {
        for (v4, v6) in [(true, false), (false, true), (true, true)] {
            let mut m = manifest();
            if !v4 {
                m.network[0].ipv4_addresses.clear();
                m.network[0].ipv4_gateway = None;
            }
            if !v6 {
                m.network[0].ipv6_addresses.clear();
                m.network[0].ipv6_gateway = None;
            }
            let mut plan = static_dns(target);
            plan.system.network.mtu = Some(1450);
            let name = "etc/systemd/network/10-ens3.network";
            let static_text = text(&plan, &m, name);
            assert!(static_text.contains("MACAddress=52:54:00:11:22:33"));
            assert!(!static_text.contains("Name="));
            assert!(static_text.contains("MTUBytes=1450"));
            assert_eq!(static_text.contains("Address=192.0.2.2/32"), v4);
            assert_eq!(static_text.contains("Address=2001:db8::2/128"), v6);
            assert_eq!(
                static_text.matches("GatewayOnLink=yes").count(),
                usize::from(v4) + usize::from(v6)
            );
            for section in ["[DHCPv4]", "[DHCPv6]", "[IPv6AcceptRA]"] {
                assert!(static_text.contains(&format!("{section}\nUseDNS=no\nUseDomains=no")));
            }
            assert!(static_text.contains("Domains=example.test"));
            m.network[0].dhcp4 = v4;
            m.network[0].dhcp6 = v6;
            m.network[0].accept_ra = v6;
            let dynamic = text(&plan, &m, name);
            assert!(dynamic.contains(&format!(
                "DHCP={}\n",
                match (v4, v6) {
                    (true, true) => "yes",
                    (true, false) => "ipv4",
                    _ => "ipv6",
                }
            )));
            assert!(!dynamic.contains("\nAddress="));
            assert!(!dynamic.contains("\nGateway="));
        }
    }
}

#[test]
fn ra_only_and_dhcpv6_without_ra_are_distinct_and_disabled_ipv6_never_leaks_routes() {
    let mut m = manifest();
    m.network[0].accept_ra = true;
    let mut plan = InstallPlan::new(Target::Debian);
    let name = "etc/systemd/network/10-ens3.network";
    let ra = text(&plan, &m, name);
    assert!(ra.contains("DHCP=no\nIPv6AcceptRA=yes"));
    assert!(!ra.contains("Address=2001:db8::2"));
    assert!(!ra.contains("Gateway=fe80::1"));
    m.network[0].accept_ra = false;
    m.network[0].dhcp6 = true;
    assert!(text(&plan, &m, name).contains("WithoutRA=solicit"));
    plan.system.network.ipv6 = Ipv6Policy::Disable;
    let disabled = text(&plan, &m, name);
    assert!(disabled.contains("DHCP=no\nIPv6AcceptRA=no\nLinkLocalAddressing=no"));
    assert!(!disabled.contains("fe80::1"));
    assert!(!disabled.contains("WithoutRA"));
    m.network[0].ipv4_addresses.clear();
    m.network[0].ipv4_gateway = None;
    assert!(render_network_configuration(&plan, &m).is_err());
    m.network[0].dhcp4 = true; // DHCP obtains the default route after boot.
    assert!(render_network_configuration(&plan, &m).is_ok());
    plan.system.dns.mode = DnsMode::Static;
    plan.system
        .dns
        .servers
        .push("2001:db8::53".parse().unwrap());
    assert!(render_network_configuration(&plan, &m).is_err());
}

#[test]
fn alpine_renders_both_stacks_extra_addresses_mtu_and_one_dns_owner() {
    let scratch = Scratch::new("network_config");
    let mut m = manifest();
    m.network[0].ipv4_addresses.push("192.0.2.3/32".into());
    m.network[0].ipv6_addresses.push("2001:db8::3/128".into());
    let mut plan = static_dns(Target::Alpine);
    plan.system.network.mtu = Some(1400);
    scratch.apply(plan, &m).unwrap();
    let interfaces = scratch.read("etc/network/interfaces");
    assert_eq!(interfaces.matches("mtu 1400").count(), 2);
    assert!(!interfaces.contains("loopback\n    mtu"));
    assert!(interfaces.contains("post-up ip -4 addr replace 192.0.2.3/32 dev ens3"));
    assert!(interfaces.contains("post-up ip -6 addr replace 2001:db8::3/128 dev ens3"));
    assert!(interfaces.contains("via fe80::1 onlink dev ens3"));
    let renamed =
        String::from_utf8(rewrite_ifupdown(&interfaces, |_| Some("eth0".into()))).unwrap();
    assert!(!renamed.contains("ens3"));
    assert!(
        scratch
            .read("etc/dhcpcd.conf")
            .contains("nohook resolv.conf")
    );
    assert!(
        scratch
            .read("etc/udhcpc/udhcpc.conf")
            .contains("RESOLV_CONF=\"no\"")
    );
    assert!(
        scratch
            .read("etc/resolv.conf")
            .contains("search example.test")
    );
    m.network[0].dhcp4 = true;
    m.network[0].accept_ra = true;
    scratch.apply(InstallPlan::new(Target::Alpine), &m).unwrap();
    let auto = scratch.read("etc/network/interfaces");
    assert!(auto.contains("inet dhcp"));
    assert!(auto.contains("inet6 auto"));
    assert!(!auto.contains("post-up"));
    assert!(!scratch.read("etc/dhcpcd.conf").contains("nohook"));
}

#[test]
fn automatic_and_static_resolvers_are_published_after_packages_without_following_links() {
    let scratch = Scratch::new("network_config");
    let m = manifest();
    scratch.write("etc/resolv.conf", "nameserver 192.0.2.53\n");
    for _ in 0..2 {
        scratch.apply(InstallPlan::new(Target::Debian), &m).unwrap();
    }
    assert_eq!(
        fs::read_link(scratch.0.join("etc/resolv.conf")).unwrap(),
        Path::new("/run/systemd/resolve/stub-resolv.conf")
    );
    for _ in 0..2 {
        scratch.apply(static_dns(Target::Debian), &m).unwrap();
    }
    assert!(
        !fs::symlink_metadata(scratch.0.join("etc/resolv.conf"))
            .unwrap()
            .is_symlink()
    );
    assert!(
        scratch
            .read("etc/resolv.conf")
            .contains("nameserver 1.1.1.1")
    );
    let network = scratch.read("etc/systemd/network/10-ens3.network");
    assert_eq!(network.matches("MTUBytes=").count(), 0);
    assert_eq!(network.matches("DNS=1.1.1.1").count(), 1);
    // A fresh Alpine target retains the bootstrap resolver for native DHCP.
    let alpine = Scratch::new("network_config");
    alpine.write("etc/resolv.conf", "nameserver 192.0.2.53\n");
    alpine.apply(InstallPlan::new(Target::Alpine), &m).unwrap();
    assert_eq!(alpine.read("etc/resolv.conf"), "nameserver 192.0.2.53\n");
}

#[test]
fn debian_relative_resolved_links_are_replaced_without_following_them() {
    for target in [
        "../run/systemd/resolve/stub-resolv.conf",
        "../run/systemd/resolve/resolv.conf",
    ] {
        let scratch = Scratch::new("relative-resolved");
        fs::create_dir(scratch.0.join("etc")).unwrap();
        scratch.write("run/systemd/resolve/stub-resolv.conf", "do not overwrite\n");
        std::os::unix::fs::symlink(target, scratch.0.join("etc/resolv.conf")).unwrap();
        scratch
            .apply(InstallPlan::new(Target::Debian), &manifest())
            .unwrap();
        assert_eq!(
            fs::read_link(scratch.0.join("etc/resolv.conf")).unwrap(),
            Path::new("/run/systemd/resolve/stub-resolv.conf")
        );
        assert_eq!(
            scratch.read("run/systemd/resolve/stub-resolv.conf"),
            "do not overwrite\n"
        );
    }
}

#[test]
fn multiple_interfaces_have_independent_mac_matching_and_byte_identical_reapplication() {
    let scratch = Scratch::new("network_config");
    let mut m = manifest();
    let mut second = m.network[0].clone();
    second.name = "enp5s0".into();
    second.mac = "52:54:00:11:22:44".into();
    second.ipv4_addresses = vec!["198.51.100.2/24".into()];
    second.ipv4_gateway = None;
    second.ipv6_addresses.clear();
    second.ipv6_gateway = None;
    m.network.push(second);
    let mut plan = static_dns(Target::Debian);
    plan.system.network.mtu = Some(1450);
    let rendered = render_network_configuration(&plan, &m).unwrap();
    scratch.apply(plan, &m).unwrap();
    scratch
        .apply(static_dns_with_mtu(Target::Debian), &m)
        .unwrap();
    for file in rendered {
        assert_eq!(fs::read(scratch.0.join(file.path)).unwrap(), file.contents);
    }
    assert!(
        scratch
            .read("etc/systemd/network/11-enp5s0.network")
            .contains("MACAddress=52:54:00:11:22:44")
    );
}
fn static_dns_with_mtu(target: Target) -> InstallPlan {
    let mut plan = static_dns(target);
    plan.system.network.mtu = Some(1450);
    plan
}

#[test]
fn invalid_manifest_values_fail_native_validation_before_rendering() {
    let plan = InstallPlan::new(Target::Debian);
    for bad in [
        "-option",
        ".",
        "..",
        "lo",
        "bad/name",
        "x\n[Network]",
        "bad;id",
    ] {
        let mut m = manifest();
        m.network[0].name = bad.into();
        assert!(render_network_configuration(&plan, &m).is_err(), "{bad}");
    }
    for bad in [
        "127.1/8",
        "01.2.3.4/24",
        "255.255.255.255/32",
        "224.0.0.1/24",
        "192.0.2.1/33",
        "192.0.2.1/+1",
        "192.0.2.1/01",
        "1.1.1.1/24\n",
    ] {
        let mut m = manifest();
        m.network[0].ipv4_addresses = vec![bad.into()];
        assert!(render_network_configuration(&plan, &m).is_err(), "{bad}");
    }
    for bad in [
        "::/64",
        "::1/128",
        "ff02::1/64",
        "fe80::1%ens3/64",
        "::ffff:192.0.2.1/128",
        "1::2::3/64",
        "2001:db8::1/129",
    ] {
        let mut m = manifest();
        m.network[0].ipv6_addresses = vec![bad.into()];
        assert!(render_network_configuration(&plan, &m).is_err(), "{bad}");
    }
    let mut m = manifest();
    m.network[0]
        .ipv6_addresses
        .push("2001:0db8:0:0:0:0:0:2/64".into());
    assert!(render_network_configuration(&plan, &m).is_err());
    m = manifest();
    m.network.push(m.network[0].clone());
    assert!(render_network_configuration(&plan, &m).is_err());
    m = manifest();
    m.network[0].ipv4_gateway = Some("2001:db8::1".into());
    assert!(render_network_configuration(&plan, &m).is_err());
    m = manifest();
    m.network[0].ipv4_addresses.clear();
    assert!(render_network_configuration(&plan, &m).is_err());
}

#[test]
fn complete_output_validation_rejects_unsafe_files_parents_aliases_and_resolver_owners() {
    for bad in [
        "etc/systemd/network/10-ens3.network",
        "etc/sysctl.d",
        "etc/resolv.conf",
        "etc/gai.conf",
        "etc/modules-load.d/90-rustaller-network.conf",
    ] {
        let scratch = Scratch::new("network_config");
        scratch.write("outside", "unchanged\n");
        let path = scratch.0.join(bad);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(scratch.0.join("outside"), &path).unwrap();
        let mut plan = static_dns(Target::Debian);
        plan.system.network.congestion = CongestionControl::Bbr;
        plan.system.network.qdisc = QueueDiscipline::Fq;
        plan.system.network.address_preference = AddressPreference::Ipv4;
        assert!(scratch.apply(plan, &manifest()).is_err(), "{bad}");
        assert_eq!(scratch.read("outside"), "unchanged\n");
        assert!(!scratch.0.join("etc/dhcpcd.conf").exists());
        assert!(
            !scratch
                .0
                .join("etc/systemd/network/10-ens3.network")
                .is_file()
                || bad == "etc/systemd/network/10-ens3.network"
        );
    }
    for kind in ["hardlink", "directory", "writable"] {
        let scratch = Scratch::new("network_config");
        let path = scratch.0.join("etc/systemd/network/10-ens3.network");
        scratch.write("outside", "unchanged\n");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        match kind {
            "hardlink" => fs::hard_link(scratch.0.join("outside"), &path).unwrap(),
            "directory" => fs::create_dir(&path).unwrap(),
            _ => {
                fs::write(&path, "untrusted").unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
            }
        }
        assert!(
            scratch
                .apply(static_dns(Target::Debian), &manifest())
                .is_err()
        );
        assert!(!scratch.0.join("etc/resolv.conf").exists());
        assert_eq!(scratch.read("outside"), "unchanged\n");
    }
    let scratch = Scratch::new("network_config");
    let plan = validate_live_plan_with_uid(
        InstallPlan::new(Target::Debian),
        "x86_64",
        4096,
        rustaller::RUNTIME_VERSION,
        0,
    )
    .unwrap();
    for root in [Path::new("/"), Path::new("relative")] {
        assert!(apply_network_configuration(root, &plan, &manifest(), "").is_err());
    }
    let unsafe_root = scratch.0.join("..").join(scratch.0.file_name().unwrap());
    assert!(apply_network_configuration(&unsafe_root, &plan, &manifest(), "").is_err());
}

#[test]
fn target_kernel_gai_and_openrc_dependencies_are_checked_before_publication() {
    let scratch = Scratch::new("network_config");
    let mut plan = InstallPlan::new(Target::Debian);
    plan.system.network.congestion = CongestionControl::Bbr;
    plan.system.network.qdisc = QueueDiscipline::Fq;
    assert!(required_modules(&plan.system.network, "").is_err());
    assert!(
        required_modules(
            &plan.system.network,
            "CONFIG_TCP_CONG_BBR=m\nCONFIG_TCP_CONG_BBR=y\nCONFIG_NET_SCH_FQ=y\n"
        )
        .is_err()
    );
    assert_eq!(
        required_modules(&plan.system.network, KERNEL).unwrap(),
        ["tcp_bbr"]
    );
    plan.system.network.syncookies = Some(true);
    assert!(
        required_modules(
            &plan.system.network,
            "CONFIG_TCP_CONG_BBR=y\nCONFIG_NET_SCH_FQ=y\n"
        )
        .is_err()
    );
    plan.system.network.address_preference = AddressPreference::Ipv4;
    scratch.write("etc/gai.conf", "precedence ::/0 99\n");
    assert!(scratch.apply(plan, &manifest()).is_err());
    assert!(!scratch.0.join("etc/systemd/network").exists());
    let mut plan = InstallPlan::new(Target::Alpine);
    plan.system.network.ipv6 = Ipv6Policy::Enable;
    assert!(scratch.apply(plan, &manifest()).is_err()); // missing sysctl boot service
    assert!(!scratch.0.join("etc/network/interfaces").exists());
    scratch.service("sysctl");
    let mut plan = InstallPlan::new(Target::Alpine);
    plan.system.network.ipv6 = Ipv6Policy::Enable;
    scratch.apply(plan, &manifest()).unwrap();
    assert_eq!(
        fs::read_link(scratch.0.join("etc/runlevels/boot/sysctl")).unwrap(),
        Path::new("/etc/init.d/sysctl")
    );
    let mut plan = InstallPlan::new(Target::Alpine);
    plan.system.network.address_preference = AddressPreference::Ipv4;
    assert!(render_network_configuration(&plan, &manifest()).is_err());
    for bad in [
        "# BEGIN rustaller target network\n",
        "# END rustaller target network\n",
        "# BEGIN rustaller target network x\n# END rustaller target network\n",
    ] {
        let scratch = Scratch::new("network_config");
        scratch.write("etc/gai.conf", bad);
        let mut plan = InstallPlan::new(Target::Debian);
        plan.system.network.address_preference = AddressPreference::Ipv4;
        assert!(scratch.apply(plan, &manifest()).is_err());
        assert!(!scratch.0.join("etc/systemd/network").exists());
    }
}

#[test]
fn manual_tcp_modules_and_complete_glibc_precedence_are_idempotent_and_offline() {
    let scratch = Scratch::new("network_config");
    scratch.write("etc/gai.conf", "# keep\nlabel ::1/128 0\n");
    scratch.write(
        "etc/modules-load.d/90-rustaller-network.conf",
        "# keep\nloop\n",
    );
    let mut plan = InstallPlan::new(Target::Debian);
    plan.system.network = rustaller::config::NetworkConfig {
        congestion: CongestionControl::Bbr,
        qdisc: QueueDiscipline::Fq,
        address_preference: AddressPreference::Ipv4,
        buffer_mib: Some(16),
        somaxconn: Some(8192),
        syn_backlog: Some(8192),
        netdev_backlog: Some(16384),
        mtu_probing: Some(1),
        fastopen: Some(3),
        syncookies: Some(true),
        ..Default::default()
    };
    let live =
        validate_live_plan_with_uid(plan, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0).unwrap();
    for _ in 0..2 {
        apply_network_configuration(&scratch.0, &live, &manifest(), KERNEL).unwrap();
    }
    let sysctl = scratch.read("etc/sysctl.d/90-rustaller-network.conf");
    for setting in [
        "net.ipv4.tcp_congestion_control = bbr",
        "net.core.default_qdisc = fq",
        "net.core.rmem_max = 16777216",
        "net.ipv4.tcp_rmem = 4096 87380 16777216",
        "net.core.somaxconn = 8192",
        "net.ipv4.tcp_max_syn_backlog = 8192",
        "net.core.netdev_max_backlog = 16384",
        "net.ipv4.tcp_mtu_probing = 1",
        "net.ipv4.tcp_fastopen = 3",
        "net.ipv4.tcp_syncookies = 1",
    ] {
        assert!(sysctl.contains(setting), "{setting}");
    }
    assert!(!sysctl.contains("accept_ra"));
    assert!(!sysctl.contains("kernel.panic"));
    let gai = scratch.read("etc/gai.conf");
    assert_eq!(gai.matches("precedence ").count(), 5);
    assert!(gai.contains("label ::1/128 0"));
    assert!(gai.contains("precedence ::ffff:0:0/96 100"));
    let modules = scratch.read("etc/modules-load.d/90-rustaller-network.conf");
    assert_eq!(modules.matches("tcp_bbr").count(), 1);
    assert!(modules.contains("loop\n"));
}

#[test]
fn failed_file_publication_reports_partial_progress_without_reboot_or_rollback_claims() {
    // A native read-only directory reliably rejects creation for unprivileged
    // test users. Root bypasses DAC, so that case cannot exercise this failure.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let scratch = Scratch::new("network_config");
    scratch.write("etc/resolv.conf", "original resolver\n");
    fs::create_dir_all(scratch.0.join("etc/systemd/network")).unwrap();
    fs::set_permissions(
        scratch.0.join("etc/systemd/network"),
        fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    let error = scratch
        .apply(static_dns(Target::Debian), &manifest())
        .unwrap_err();
    assert!(error.contains("10-ens3.network"), "{error}");
    assert!(error.contains("earlier files may already have been replaced"));
    assert_eq!(scratch.read("etc/resolv.conf"), "original resolver\n");
    assert_eq!(
        fs::read_dir(scratch.0.join("etc/systemd/network"))
            .unwrap()
            .count(),
        0
    );
    fs::set_permissions(
        scratch.0.join("etc/systemd/network"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
}

#[test]
fn alpine_native_client_flags_honor_ipv4_ipv6_and_ra_policies() {
    for (dhcp4, dhcp6, ra, options) in [
        (true, false, false, "-4"),
        (false, true, false, "-6 --noipv6rs --ia_na"),
        (false, false, true, "-6 --nodhcp6"),
        (true, false, true, "--nodhcp6"),
        (false, true, true, "-6"),
    ] {
        let mut m = manifest();
        m.network[0].dhcp4 = dhcp4;
        m.network[0].dhcp6 = dhcp6;
        m.network[0].accept_ra = ra;
        let plan = InstallPlan::new(Target::Alpine);
        let output = text(&plan, &m, "etc/network/interfaces");
        assert!(
            output.contains(&format!("dhcp-opts {options}\n")),
            "{output}"
        );
        assert!(output.contains(&format!("ipv6-accept-ra {}\n", u8::from(ra))));
        assert!(output.contains("dhcp-program dhcpcd"));
        assert!(output.contains("use ipv6"));
        if !dhcp4 && !dhcp6 {
            assert!(output.contains("use dhcp"));
        }
    }
    let mut m = manifest();
    m.network[0].ipv4_addresses.clear();
    m.network[0].ipv4_gateway = None;
    let mut second = m.network[0].clone();
    second.name = "eth1".into();
    second.mac = "52:54:00:11:22:44".into();
    second.dhcp4 = true;
    second.ipv6_gateway = None;
    m.network.push(second);
    let mut plan = InstallPlan::new(Target::Alpine);
    plan.system.network.ipv6 = Ipv6Policy::Disable;
    let output = text(&plan, &m, "etc/network/interfaces");
    assert!(output.contains("iface ens3 inet manual\n    use ipv6\n    ipv6-accept-ra 0\n"));
    assert!(output.contains("dhcp-opts -4\n"));
    assert!(!output.contains("inet6"));
}

#[test]
fn competing_default_gateways_are_rejected_before_any_target_file_is_written() {
    for v6 in [false, true] {
        let scratch = Scratch::new("multiple-defaults");
        let mut m = manifest();
        let mut second = m.network[0].clone();
        second.name = "eth1".into();
        second.mac = "52:54:00:11:22:44".into();
        if v6 {
            second.ipv4_gateway = None;
        } else {
            second.ipv6_gateway = None;
        }
        m.network.push(second);
        assert!(scratch.apply(InstallPlan::new(Target::Debian), &m).is_err());
        assert!(!scratch.0.join("etc").exists());
    }
}

#[test]
fn gentoo_openrc_netifrc_preserves_static_and_dynamic_families_without_systemd() {
    let mut p = static_dns(Target::Gentoo);
    p.system.network.mtu = Some(1450);
    let mut m = manifest();
    let files = render_network_configuration(&p, &m).unwrap();
    assert!(
        !files
            .iter()
            .any(|file| file.path.to_string_lossy().contains("systemd"))
    );
    let static_text = text(&p, &m, "etc/conf.d/net");
    assert!(static_text.contains("config_ens3=( \"192.0.2.2/32\" \"2001:db8::2/128\" )"));
    assert!(static_text.contains("-4 default via 192.0.2.1 onlink"));
    assert!(static_text.contains("-6 default via fe80::1 onlink"));
    assert!(static_text.contains("mtu_ens3=\"1450\""));
    assert!(text(&p, &m, "etc/dhcpcd.conf").contains("nohook resolv.conf"));
    assert!(
        text(&p, &m, "etc/udev/rules.d/70-rustaller-network.rules").contains("52:54:00:11:22:33")
    );
    m.network[0].accept_ra = true;
    let mixed = text(&p, &m, "etc/conf.d/net");
    assert!(mixed.contains("\"dhcp\" \"192.0.2.2/32\""));
    assert!(mixed.contains("-6 --nodhcp6"));
    assert!(!mixed.contains("2001:db8::2"));
    m.network[0].dhcp4 = true;
    m.network[0].accept_ra = false;
    let other_mixed = text(&p, &m, "etc/conf.d/net");
    assert!(other_mixed.contains("-4"));
    assert!(other_mixed.contains("2001:db8::2/128"));
    assert!(!other_mixed.contains("192.0.2.2"));
}

#[test]
fn gentoo_openrc_rejects_netifrc_variable_name_collisions() {
    let p = InstallPlan::new(Target::Gentoo);
    let mut m = manifest();
    m.network[0].name = "eth-0".into();
    let mut other = m.network[0].clone();
    other.name = "eth_0".into();
    other.mac = "52:54:00:11:22:34".into();
    other.ipv4_gateway = None;
    other.ipv6_gateway = None;
    m.network.push(other);
    assert!(render_network_configuration(&p, &m).is_err());
}
