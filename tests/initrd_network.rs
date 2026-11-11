use reinstall_rs::command::{
    CommandOutput, CommandRunner, CommandSpec, MockRunner, PipelineOutput,
};
use reinstall_rs::worker::initrd_network::{
    AddressFamily, InitrdNetworkConfig, ProxyEndpoint, address_from_ip_output,
    gateway_from_route_output, interface_for_mac, run_initrd_network,
    run_initrd_network_with_proxy,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn physical_interface_lookup_matches_mac_and_skips_azure_master_slaves() {
    let links = concat!(
        "2: eth0: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc mq state UP qlen 1000 ",
        "link/ether 00:11:22:33:44:55 brd ff:ff:ff:ff:ff:ff\n",
        "3: eth1: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc mq ",
        "master eth0 state UP qlen 1000 link/ether 00:11:22:33:44:55 ",
        "brd ff:ff:ff:ff:ff:ff\n"
    );
    assert_eq!(
        interface_for_mac(links, "00:11:22:33:44:55").as_deref(),
        Some("eth0")
    );
    assert_eq!(interface_for_mac(links, "00:11:22:33:44:56"), None);
}

#[test]
fn proxy_transport_preserves_working_interfaces_without_direct_public_egress_or_credential_arguments()
 {
    let scratch = Scratch::new();
    let mut runner = NetworkRunner::new(true);
    let config = InitrdNetworkConfig::new(
        "00:11:22:33:44:55",
        "198.51.100.8/24",
        "198.51.100.1",
        "2001:db8::8/64",
        "2001:db8::1",
        "",
    )
    .unwrap();
    let result = run_initrd_network_with_proxy(
        &mut runner,
        &scratch.0,
        &config,
        Some("http://username:PRIVATE-PROXY-PASSWORD@proxy.example:3128"),
    )
    .unwrap()
    .unwrap();
    assert!(result.ipv4_has_internet && result.ipv6_has_internet);
    let probes = runner
        .calls
        .iter()
        .filter(|call| call.program == "nc")
        .collect::<Vec<_>>();
    assert!(!probes.is_empty());
    for probe in probes {
        assert_eq!(probe.args[probe.args.len() - 2], "proxy.example");
        assert_eq!(probe.args.last().unwrap(), "3128");
        assert!(
            !probe
                .args
                .iter()
                .any(|arg| arg.to_string_lossy().contains("PRIVATE-PROXY-PASSWORD"))
        );
    }
    for (url, host, port) in [
        ("https://[2001:db8::1]:8443", "2001:db8::1", 8443),
        ("http://10.0.2.2:8080", "10.0.2.2", 8080),
        ("http://proxy.example", "proxy.example", 80),
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
        assert!(ProxyEndpoint::parse(url).is_err());
    }
}

#[test]
fn address_parser_selects_first_global_address_and_preserves_prefix() {
    let ipv4 = "2: eth0 inet 198.51.100.8/24 brd 198.51.100.255 scope global eth0\n";
    assert_eq!(
        address_from_ip_output(ipv4, AddressFamily::Ipv4).as_deref(),
        Some("198.51.100.8/24")
    );
    let ipv6 = concat!(
        "2: eth0 inet6 2001:db8::8/64 scope global\n",
        "2: eth0 inet6 2001:db8::9/64 scope global\n"
    );
    assert_eq!(
        address_from_ip_output(ipv6, AddressFamily::Ipv6).as_deref(),
        Some("2001:db8::8/64")
    );
    assert_eq!(address_from_ip_output("", AddressFamily::Ipv4), None);
}

#[test]
fn gateway_parser_reads_default_route_next_hop_without_shell_cut_pipeline() {
    assert_eq!(
        gateway_from_route_output("default via 198.51.100.1 dev eth0 proto static\n"),
        Some("198.51.100.1".into())
    );
    assert_eq!(
        gateway_from_route_output("default dev eth0 proto kernel\n"),
        None
    );
}

struct NetworkRunner {
    calls: Vec<CommandSpec>,
    nc_success: bool,
}

impl NetworkRunner {
    fn new(nc_success: bool) -> Self {
        Self {
            calls: Vec::new(),
            nc_success,
        }
    }
}

impl CommandRunner for NetworkRunner {
    fn run(&mut self, command: &CommandSpec) -> io::Result<CommandOutput> {
        self.calls.push(command.clone());
        let program = command.program.to_string_lossy();
        let args: Vec<String> = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let output = if program == "ip" && args == ["-o", "link"] {
            b"2: eth0: <BROADCAST,UP,LOWER_UP> mtu 1500 link/ether 00:11:22:33:44:55 brd ff:ff:ff:ff:ff:ff\n".to_vec()
        } else if program == "ip" && args.starts_with(&["-4", "-o", "addr"].map(str::to_owned)) {
            b"2: eth0 inet 198.51.100.8/24 scope global eth0\n".to_vec()
        } else if program == "ip" && args.starts_with(&["-6", "-o", "addr"].map(str::to_owned)) {
            b"2: eth0 inet6 2001:db8::8/64 scope global\n".to_vec()
        } else if program == "ip" && args.starts_with(&["-4", "addr", "show"].map(str::to_owned)) {
            b"2: eth0 inet 198.51.100.8/24 scope global eth0\n".to_vec()
        } else if program == "ip" && args.starts_with(&["-6", "addr", "show"].map(str::to_owned)) {
            b"2: eth0 inet6 2001:db8::8/64 scope global\n".to_vec()
        } else if program == "ip" && args.starts_with(&["-4", "route", "show"].map(str::to_owned)) {
            b"default via 198.51.100.1 dev eth0 proto static\n".to_vec()
        } else if program == "ip" && args.starts_with(&["-6", "route", "show"].map(str::to_owned)) {
            b"default via 2001:db8::1 dev eth0 proto ra\n".to_vec()
        } else {
            Vec::new()
        };
        let status = if program == "timeout" || (program == "nc" && !self.nc_success) {
            1
        } else {
            0
        };
        Ok(CommandOutput::new(Some(status), output, Vec::new()))
    }

    fn run_pipeline(&mut self, _commands: &[CommandSpec]) -> io::Result<PipelineOutput> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pipeline unused",
        ))
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "reinstall-initrd-network-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("etc")).unwrap();
        fs::write(
            root.join("etc/resolv.conf"),
            "nameserver 1.1.1.1\nnameserver 2001:4860:4860::8888\n",
        )
        .unwrap();
        Self(root)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn initrd_setup_uses_mac_networks_and_publishes_live_netconf() {
    let scratch = Scratch::new();
    let config = InitrdNetworkConfig::new(
        "00:11:22:33:44:55",
        "198.51.100.8/24",
        "198.51.100.1",
        "2001:db8::8/64",
        "2001:db8::1",
        "",
    )
    .unwrap();
    let mut runner = NetworkRunner::new(true);
    let result = run_initrd_network(&mut runner, &scratch.0, &config)
        .unwrap()
        .unwrap();
    assert_eq!(result.interface, "eth0");
    assert!(result.dhcpv4);
    assert!(result.dhcpv6_or_slaac);
    assert!(result.ipv4_has_internet);
    assert!(result.ipv6_has_internet);
    assert!(!result.should_disable_dhcpv4);
    assert!(!result.should_disable_accept_ra);
    let netconf = scratch.0.join("dev/netconf/eth0");
    assert_eq!(fs::read_to_string(netconf.join("dhcpv4")).unwrap(), "1\n");
    assert_eq!(
        fs::read_to_string(netconf.join("ipv4_addr")).unwrap(),
        "198.51.100.8/24\n"
    );
    assert_eq!(
        fs::read_to_string(netconf.join("ipv6_has_internet")).unwrap(),
        "1\n"
    );
    assert_eq!(runner.calls[0].args, ["-o", "link"]);
    assert_eq!(runner.calls[1].args, ["link", "set", "dev", "lo", "up"]);
    assert_eq!(runner.calls[2].args, ["link", "set", "dev", "eth0", "up"]);
    assert!(runner.calls.iter().any(|call| {
        call.program == "timeout" && call.args == ["15", "udhcpc", "-i", "eth0", "-f", "-q", "-n"]
    }));
    assert!(runner.calls.iter().any(|call| {
        call.program == "nc"
            && call.args[4] == "-s"
            && call.args[5] == "198.51.100.8"
            && call.args[6] == "1.1.1.1"
            && call.args[7] == "443"
    }));
    assert!(runner.calls.iter().all(|call| call.program != "sh"));
}

#[test]
fn absent_interface_keeps_the_legacy_twenty_probe_nonfatal_behavior() {
    let config = InitrdNetworkConfig::new("00:11:22:33:44:55", "", "", "", "", "").unwrap();
    let mut outputs = Vec::new();
    for _ in 0..20 {
        outputs.push(CommandOutput::new(Some(0), Vec::new(), Vec::new()));
        outputs.push(CommandOutput::new(Some(0), Vec::new(), Vec::new()));
    }
    let mut runner = MockRunner::new(outputs);
    assert!(
        run_initrd_network(&mut runner, Path::new("/unused"), &config)
            .unwrap()
            .is_none()
    );
    assert_eq!(runner.calls().len(), 40);
    let (pairs, tail) = runner.calls().as_chunks::<2>();
    assert!(tail.is_empty());
    for calls in pairs {
        assert_eq!(calls[0].program, "ip");
        assert_eq!(calls[0].args, ["-o", "link"]);
        assert_eq!(calls[1].program, "sleep");
        assert_eq!(calls[1].args, ["1"]);
    }
}

#[test]
fn failed_connectivity_flushes_unusable_network_state_and_restores_dns_fallbacks() {
    let scratch = Scratch::new();
    let proc = scratch.0.join("proc/sys/net/ipv6/conf/eth0");
    fs::create_dir_all(&proc).unwrap();
    fs::write(proc.join("accept_ra"), "1\n").unwrap();
    fs::write(proc.join("autoconf"), "1\n").unwrap();
    let config = InitrdNetworkConfig::new(
        "00:11:22:33:44:55",
        "198.51.100.8/24",
        "198.51.100.1",
        "2001:db8::8/64",
        "2001:db8::1",
        "",
    )
    .unwrap();
    let mut runner = NetworkRunner::new(false);
    let result = run_initrd_network(&mut runner, &scratch.0, &config)
        .unwrap()
        .unwrap();
    assert!(!result.ipv4_has_internet);
    assert!(!result.ipv6_has_internet);
    assert!(result.should_disable_dhcpv4);
    assert!(result.should_disable_accept_ra);
    assert!(result.should_disable_autoconf);
    assert_eq!(fs::read_to_string(proc.join("accept_ra")).unwrap(), "0\n");
    assert_eq!(fs::read_to_string(proc.join("autoconf")).unwrap(), "0\n");
    assert_eq!(
        fs::read_to_string(scratch.0.join("etc/resolv.conf")).unwrap(),
        "nameserver 1.1.1.1\nnameserver 8.8.8.8\nnameserver 2606:4700:4700::1111\nnameserver 2001:4860:4860::8888\n"
    );
    assert!(runner.calls.iter().any(|call| {
        call.program == "ip"
            && call.args.len() >= 3
            && call.args[0] == "-4"
            && call.args[1] == "addr"
            && call.args[2] == "flush"
    }));
    assert!(runner.calls.iter().any(|call| {
        call.program == "ip"
            && call.args.len() >= 3
            && call.args[0] == "-6"
            && call.args[1] == "route"
            && call.args[2] == "flush"
    }));
}

#[test]
fn initrd_network_config_rejects_bad_mac_and_preserves_six_field_protocol() {
    let config = InitrdNetworkConfig::new(
        "00:11:22:33:44:55",
        "198.51.100.8/24",
        "198.51.100.1",
        "2001:db8::8/64",
        "2001:db8::1",
        "2001:db8::9/64",
    )
    .unwrap();
    assert_eq!(config.mac_addr(), "00:11:22:33:44:55");
    assert_eq!(config.ipv6_extra_addrs(), "2001:db8::9/64");
    assert!(InitrdNetworkConfig::new("not-a-mac", "", "", "", "", "").is_err());
}
