mod support;

use rustaller::command::{CommandOutput, CommandRunner, CommandSpec, PipelineOutput};
use rustaller::config::{DnsConfig, DnsMode, Ipv6Policy, NetworkConfig};
use rustaller::deployment::{DeploymentManifest, DiskIdentity, NetworkInterface};
use rustaller::model::BootMode;
use rustaller::worker::initrd_network::{
    AddressFamily, ProxyEndpoint, address_from_ip_output, apply_network_policy,
    gateway_from_route_output, interface_for_mac, source_dns_seed,
};
use std::io;
use support::Scratch;

const MAC: &str = "00:11:22:33:44:55";
const LINKS: &str = concat!(
    "2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc mq state UP ",
    "link/ether 00:11:22:33:44:55 brd ff:ff:ff:ff:ff:ff\n",
    "3: eth1: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc mq ",
    "master eth0 state UP link/ether 00:11:22:33:44:55 brd ff:ff:ff:ff:ff:ff\n",
);
const V6: &str = "2001:db8::8/64";

#[test]
fn physical_interface_lookup_requires_a_unique_mac_on_a_non_member_link() {
    assert_eq!(interface_for_mac(LINKS, MAC).as_deref(), Some("eth0"));
    assert_eq!(interface_for_mac(LINKS, "00:11:22:33:44:56"), None);
    let duplicate = format!("{LINKS}4: eth2: <BROADCAST,LOWER_UP> mtu 1500 link/ether {MAC}\n");
    assert_eq!(interface_for_mac(&duplicate, MAC), None);
}

#[test]
fn address_and_gateway_parsers_preserve_prefix_and_skip_ipv6_dad() {
    let ipv6 = "2: eth0 inet6 2001:db8::1/64 scope global tentative\n\
                2: eth0 inet6 2001:db8::2/64 scope global dadfailed\n\
                2: eth0 inet6 2001:db8::8/64 scope global\n";
    assert_eq!(
        address_from_ip_output(ipv6, AddressFamily::Ipv6).as_deref(),
        Some(V6)
    );
    assert_eq!(
        gateway_from_route_output("default via 198.51.100.1 dev eth0 proto static\n"),
        Some("198.51.100.1".into())
    );
    assert_eq!(
        gateway_from_route_output("default dev eth0 proto kernel\n"),
        None
    );
    assert_eq!(gateway_from_route_output("default via malformed\n"), None);
}

#[test]
fn proxy_parser_drops_credentials_and_rejects_loopback_or_invalid_transports() {
    for (url, host, port) in [
        ("https://[2001:db8::1]:8443", "2001:db8::1", 8443),
        ("http://10.0.2.2:8080", "10.0.2.2", 8080),
        ("http://username:PRIVATE@proxy.example", "proxy.example", 80),
    ] {
        assert_eq!(
            ProxyEndpoint::parse(url).unwrap(),
            ProxyEndpoint {
                host: host.into(),
                port
            }
        );
    }
    for url in [
        "socks5://proxy.example:1080",
        "http://127.0.0.1:8080",
        "http://localhost:8080",
        "http://[::1]:8080",
        "http://proxy.example:0",
        "http://bad\nheader:80",
        "http://-e:80",
        "http://2001:db8::1",
    ] {
        assert!(ProxyEndpoint::parse(url).is_err(), "{url}");
    }
}

#[test]
fn source_dns_seed_uses_valid_upstream_dns_without_the_source_stub() {
    let scratch = Scratch::with_etc("network-dns-seed");
    scratch.write("etc/resolv.conf", "nameserver 127.0.0.53\n");
    scratch.write("run/systemd/resolve/resolv.conf",
                  "nameserver 192.0.2.53\nnameserver 2001:db8::53\nnameserver ::ffff:192.0.2.53\nsearch example.test\n");
    let dns = source_dns_seed(&scratch.0).unwrap();
    assert_eq!(dns.mode, DnsMode::Static);
    assert_eq!(
        dns.servers
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["192.0.2.53", "2001:db8::53"]
    );
    assert_eq!(dns.search_domains, ["example.test"]);
}

struct NetworkRunner {
    calls: Vec<CommandSpec>,
    links: String,
    v6_address: Option<String>,
    timeout_status: i32,
}

impl NetworkRunner {
    fn new() -> Self {
        Self {
            calls: vec![],
            links: LINKS.into(),
            v6_address: Some(V6.into()),
            timeout_status: 0,
        }
    }

    fn ran(&self, program: &str, args: &[&str]) -> bool {
        self.calls
            .iter()
            .any(|call| call.program == program && call.args == args)
    }

    fn dhcp(&self) -> Vec<String> {
        self.calls
            .iter()
            .filter(|call| call.program == "timeout")
            .map(|call| call.args[1].to_string_lossy().into_owned())
            .collect()
    }
}

impl CommandRunner for NetworkRunner {
    fn run(&mut self, command: &CommandSpec) -> io::Result<CommandOutput> {
        self.calls.push(command.clone());
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_str().unwrap())
            .collect::<Vec<_>>();
        let text = if command.program == "ip" {
            match args.as_slice() {
                ["-o", "link"] => self.links.clone(),
                ["-4", "-o", "addr", "show", ..] => {
                    "2: eth0 inet 198.51.100.8/24 scope global eth0\n".into()
                }
                ["-6", "-o", "addr", "show", ..] => self
                    .v6_address
                    .as_deref()
                    .map(|value| format!("2: eth0 inet6 {value} scope global\n"))
                    .unwrap_or_default(),
                ["-4", "route", "show", ..] => "default via 198.51.100.1 dev eth0\n".into(),
                ["-6", "route", "show", ..] => "default via fe80::1 dev eth0\n".into(),
                _ => String::new(),
            }
        } else {
            String::new()
        };
        let status = if command.program == "timeout" {
            self.timeout_status
        } else {
            0
        };
        Ok(CommandOutput::new(Some(status), text.into_bytes(), vec![]))
    }

    fn run_pipeline(&mut self, _commands: &[CommandSpec]) -> io::Result<PipelineOutput> {
        unreachable!()
    }
}

fn manifest() -> DeploymentManifest {
    DeploymentManifest {
        disk: DiskIdentity {
            device: "/dev/vda".into(),
            size_bytes: 8 * (1 << 30),
            serial: "test".into(),
            wwn: String::new(),
            partition_table_id: String::new(),
        },
        boot_mode: BootMode::Bios,
        alpine_branch: "v3.24".into(),
        debian_codename: String::new(),
        gentoo_stage3: None,
        network: vec![NetworkInterface {
            name: "ens3".into(),
            mac: MAC.into(),
            ipv4_addresses: vec!["198.51.100.8/24".into()],
            ipv4_gateway: Some("198.51.100.1".into()),
            ipv6_addresses: vec![V6.into()],
            ipv6_gateway: Some("fe80::1".into()),
            dhcp4: false,
            dhcp6: false,
            accept_ra: false,
        }],
    }
}

fn scratch() -> Scratch {
    let scratch = Scratch::with_etc("live-network");
    for sysctl in ["accept_ra", "autoconf", "disable_ipv6"] {
        scratch.write(&format!("proc/sys/net/ipv6/conf/eth0/{sysctl}"), "1\n");
    }
    scratch
}

#[test]
fn combined_router_advertisement_policy_can_proceed_without_a_stateful_lease() {
    let scratch = scratch();
    let mut plan = manifest();
    plan.network[0].accept_ra = true;
    plan.network[0].dhcp6 = true;
    let mut runner = NetworkRunner::new();
    runner.timeout_status = 1;
    apply_network_policy(
        &mut runner,
        &scratch.0,
        &plan,
        &NetworkConfig::default(),
        &DnsConfig::default(),
        None,
    )
    .unwrap();
    assert_eq!(runner.dhcp(), ["udhcpc6"]);

    runner.v6_address = None;
    assert!(
        apply_network_policy(
            &mut runner,
            &scratch.0,
            &plan,
            &NetworkConfig::default(),
            &DnsConfig::default(),
            None,
        )
        .is_err()
    );
}

#[test]
fn static_dual_stack_sets_on_link_gateways_and_resolver_without_dhcp() {
    let scratch = scratch();
    let mut runner = NetworkRunner::new();
    let policy = NetworkConfig {
        mtu: Some(1450),
        ..NetworkConfig::default()
    };
    let dns = DnsConfig {
        mode: DnsMode::Static,
        servers: vec!["2001:db8::53".parse().unwrap()],
        search_domains: vec!["example.test".into()],
    };
    apply_network_policy(&mut runner, &scratch.0, &manifest(), &policy, &dns, None).unwrap();
    for args in [
        vec!["link", "set", "dev", "eth0", "mtu", "1450"],
        vec!["-4", "addr", "replace", "198.51.100.8/24", "dev", "eth0"],
        vec!["-6", "addr", "replace", V6, "dev", "eth0"],
        vec!["-6", "route", "replace", "fe80::1", "dev", "eth0"],
        vec![
            "-6", "route", "replace", "default", "via", "fe80::1", "dev", "eth0",
        ],
    ] {
        assert!(runner.ran("ip", &args), "{args:?}");
    }
    assert!(runner.dhcp().is_empty());
    assert_eq!(scratch.read("proc/sys/net/ipv6/conf/eth0/accept_ra"), "0\n");
    assert!(
        scratch
            .read("etc/resolv.conf")
            .contains("nameserver 2001:db8::53")
    );
}

#[test]
fn dhcp_and_ra_follow_the_plan_and_dynamic_observations_are_never_replayed() {
    let scratch = scratch();
    let mut plan = manifest();
    plan.network[0].dhcp4 = true;
    plan.network[0].accept_ra = true;
    let mut runner = NetworkRunner::new();
    apply_network_policy(
        &mut runner,
        &scratch.0,
        &plan,
        &NetworkConfig::default(),
        &DnsConfig::default(),
        None,
    )
    .unwrap();
    assert_eq!(runner.dhcp(), ["udhcpc"]);
    assert!(runner.calls.iter().all(|call| {
        !call
            .args
            .iter()
            .any(|arg| arg == "flush" || arg == "replace")
    }));
    assert_eq!(scratch.read("proc/sys/net/ipv6/conf/eth0/accept_ra"), "1\n");
    assert_eq!(scratch.read("proc/sys/net/ipv6/conf/eth0/autoconf"), "1\n");

    plan.network[0].accept_ra = false;
    plan.network[0].dhcp6 = true;
    let mut runner = NetworkRunner::new();
    apply_network_policy(
        &mut runner,
        &scratch.0,
        &plan,
        &NetworkConfig::default(),
        &DnsConfig::default(),
        None,
    )
    .unwrap();
    assert_eq!(runner.dhcp(), ["udhcpc", "udhcpc6"]);
    assert!(runner.ran(
        "ip",
        &[
            "-6", "route", "replace", "default", "via", "fe80::1", "dev", "eth0"
        ]
    ));

    let mut runner = NetworkRunner::new();
    let policy = NetworkConfig {
        ipv6: Ipv6Policy::Disable,
        ..NetworkConfig::default()
    };
    apply_network_policy(
        &mut runner,
        &scratch.0,
        &plan,
        &policy,
        &DnsConfig::default(),
        None,
    )
    .unwrap();
    assert_eq!(runner.dhcp(), ["udhcpc"]);
    assert_eq!(
        scratch.read("proc/sys/net/ipv6/conf/eth0/disable_ipv6"),
        "1\n"
    );
}

#[test]
fn missing_live_address_never_replaces_resolver_or_flushes_interfaces() {
    let scratch = scratch();
    scratch.write("etc/resolv.conf", "nameserver 192.0.2.53\n");
    let mut runner = NetworkRunner::new();
    runner.v6_address = None;
    assert!(
        apply_network_policy(
            &mut runner,
            &scratch.0,
            &manifest(),
            &NetworkConfig::default(),
            &DnsConfig::default(),
            None
        )
        .is_err()
    );
    assert_eq!(scratch.read("etc/resolv.conf"), "nameserver 192.0.2.53\n");
    assert!(
        runner
            .calls
            .iter()
            .all(|call| !call.args.iter().any(|argument| argument == "flush"))
    );
}

#[test]
fn wireless_interface_cannot_enter_the_wired_live_policy() {
    let scratch = scratch();
    std::fs::create_dir_all(scratch.0.join("sys/class/net/eth0/phy80211")).unwrap();
    let mut runner = NetworkRunner::new();
    let error = apply_network_policy(
        &mut runner,
        &scratch.0,
        &manifest(),
        &NetworkConfig::default(),
        &DnsConfig::default(),
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("wireless"));
    assert!(runner.dhcp().is_empty());
}
