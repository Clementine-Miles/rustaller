use reinstall_rs::codec::{PlanCodecError, decode_plan, encode_plan};
use reinstall_rs::config::{DnsConfig, DnsMode, InstallProfile, SystemConfig, SystemConfigError};
use reinstall_rs::host::args::{CliOutcome, parse_args};
use reinstall_rs::model::{BootMode, InstallPlan, PlanError, Target};
use reinstall_rs::shared::configure::apply_basic_configuration;
use reinstall_rs::shared::dns::apply_target_dns;
use reinstall_rs::targets::packages::{
    PRESERVED_ROOTS, apply_package_policy, arch_policy, cleanup_plan,
};
use reinstall_rs::targets::{debian, packages as profile};
use reinstall_rs::worker::live::{ValidatedLivePlan, validate_live_plan_with_uid};
use std::fs;
use std::net::IpAddr;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "reinstall-system-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("etc")).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, contents: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn plan(target: Target) -> InstallPlan {
    let mut plan = InstallPlan::new(target);
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan
}
fn live(plan: InstallPlan) -> ValidatedLivePlan {
    validate_live_plan_with_uid(plan, "x86_64", 2048, reinstall_rs::RUNTIME_VERSION, 0).unwrap()
}
fn dns() -> DnsConfig {
    DnsConfig {
        mode: DnsMode::Static,
        servers: vec![
            "1.1.1.1".parse().unwrap(),
            "2606:4700:4700::1111".parse().unwrap(),
        ],
        search_domains: vec!["example.internal".into()],
    }
}

#[test]
fn dns_requires_explicit_unicast_upstreams_and_bounded_unique_search_domains() {
    dns().validate().unwrap();
    for address in [
        "0.0.0.0",
        "127.0.0.1",
        "255.255.255.255",
        "224.0.0.1",
        "::",
        "::1",
        "ff02::1",
        "fe80::1",
        "::ffff:127.0.0.1",
    ] {
        let value = DnsConfig {
            servers: vec![address.parse().unwrap()],
            ..dns()
        };
        assert_eq!(
            value.validate(),
            Err(SystemConfigError::InvalidDnsServer),
            "{address}"
        );
    }
    let mut value = dns();
    value.servers = vec!["10.0.0.53".parse().unwrap(), "fd00::53".parse().unwrap()];
    value.validate().unwrap();
    value.servers = vec![
        "1.1.1.1".parse().unwrap(),
        "::ffff:1.1.1.1".parse().unwrap(),
    ];
    assert!(value.validate().is_err());
    value = dns();
    value.mode = DnsMode::Auto;
    assert_eq!(
        value.validate(),
        Err(SystemConfigError::AutomaticDnsWithValues)
    );
    value = dns();
    value.servers.clear();
    assert!(value.validate().is_err());
    value.servers = vec!["1.1.1.1".parse::<IpAddr>().unwrap(); 4];
    assert!(value.validate().is_err());
    for domain in [
        "",
        ".",
        "bad..domain",
        "../etc",
        "bad\nname",
        "-bad.example",
        "bad-.example",
        "example.com;id",
    ] {
        value = dns();
        value.search_domains = vec![domain.into()];
        assert!(value.validate().is_err());
    }
    value = dns();
    value.search_domains = vec!["Example.com".into(), "example.COM".into()];
    assert!(value.validate().is_err());
    value.search_domains = vec!["internal".into()];
    value.validate().unwrap();
    let auto = DnsConfig::default();
    assert!(auto.resolv_conf().unwrap().is_none());
    assert!(auto.networkd_settings().unwrap().is_none());
}

#[test]
fn minimal_cli_options_roundtrip_and_old_plan_schemas_fail_closed() {
    let CliOutcome::Plan(plan) = parse_args(
        [
            "debian",
            "--minimal",
            "--with-tools",
            "--keep-docs",
            "--keep-package-cache",
            "--dns",
            "1.1.1.1,2606:4700:4700::1111",
            "--dns-search",
            "example.internal",
            "--kernel",
            "cloud",
        ],
        Some("https://assets.example/reinstall"),
    )
    .unwrap() else {
        panic!("plan")
    };
    assert_eq!(plan.system.profile, InstallProfile::Minimal);
    assert!(plan.system.with_tools);
    assert!(plan.system.keep_docs);
    assert!(plan.system.keep_package_cache);
    assert_eq!(plan.system.dns, dns());
    let decoded = decode_plan(&encode_plan(&plan).unwrap()).unwrap();
    assert_eq!(decoded.system, plan.system);
    assert_eq!(
        debian::apt_package_list_for_plan(&plan, BootMode::Bios, &[]).unwrap()[5],
        "linux-image-cloud-amd64"
    );
    for schema in [
        1_u16,
        2_u16,
        3_u16,
        4_u16,
        reinstall_rs::model::CURRENT_SCHEMA_VERSION + 1,
    ] {
        let mut bytes = b"RINSTALL\0".to_vec();
        bytes.extend_from_slice(&schema.to_le_bytes());
        assert!(
            matches!(decode_plan(&bytes),Err(PlanCodecError::InvalidPlan(PlanError::UnsupportedSchema(value))) if value==schema)
        );
    }
    for args in [
        vec!["debian", "--dns-mode", "auto", "--dns", "1.1.1.1"],
        vec!["debian", "--dns-mode", "static"],
        vec!["debian", "--dns", "1.1.1.1:53"],
        vec!["debian", "--dns-search", "example.com"],
        vec!["debian", "--profile", "tiny"],
    ] {
        assert!(parse_args(args, Some("https://assets.example/reinstall")).is_err());
    }
}

#[test]
fn defaults_preserve_existing_behavior_and_profiles_never_apply_to_rescue_or_media() {
    assert!(SystemConfig::default().is_default());
    for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
        let original = plan(target);
        original.validate().unwrap();
        assert!(!original.system.strip_docs());
        assert!(!original.system.clear_package_cache());
        let mut selected = plan(target);
        selected.system.profile = InstallProfile::Minimal;
        let packages = profile::package_plan(&selected, BootMode::Bios, true, &[]).unwrap();
        assert!(packages.strip_docs);
        assert!(packages.clear_package_cache);
        assert!(
            packages
                .bootstrap_packages
                .iter()
                .chain(&packages.later_packages)
                .any(|package| package.contains("openssh"))
        );
        selected.hold = Some(1);
        assert!(selected.validate().is_err());
    }
    for target in [Target::LocalIso, Target::NetbootXyz, Target::Reset] {
        let system = SystemConfig {
            profile: InstallProfile::Minimal,
            ..Default::default()
        };
        assert!(system.validate(target, None).is_err());
        let system = SystemConfig {
            dns: dns(),
            ..Default::default()
        };
        assert!(system.validate(target, None).is_err());
    }
}

#[test]
fn static_dns_updates_networkd_and_replaces_resolver_link_without_following_it() {
    for target in [Target::Debian, Target::Arch, Target::Gentoo] {
        let scratch = Scratch::new();
        scratch.write(
            "etc/systemd/network/10-eth0.network",
            "[Match]\nName=eth0\n[Network]\nDHCP=yes\nDNS=9.9.9.9\n",
        );
        let outside = scratch.0.join("outside-resolver");
        fs::write(&outside, "keep\n").unwrap();
        symlink(&outside, scratch.0.join("etc/resolv.conf")).unwrap();
        let mut plan = plan(target);
        plan.system.dns = dns();
        let plan = live(plan);
        for _ in 0..2 {
            apply_target_dns(&scratch.0, &plan).unwrap();
        }
        assert_eq!(fs::read_to_string(&outside).unwrap(), "keep\n");
        assert_eq!(
            fs::read_to_string(scratch.0.join("etc/resolv.conf")).unwrap(),
            dns().resolv_conf().unwrap().unwrap()
        );
        let network =
            fs::read_to_string(scratch.0.join("etc/systemd/network/10-eth0.network")).unwrap();
        assert!(network.contains("DHCP=yes"));
        assert!(network.contains("DNS=1.1.1.1"));
        assert!(network.contains("DNS=2606:4700:4700::1111"));
        assert_eq!(network.matches("# BEGIN rust-aller target DNS").count(), 1);
        assert!(network.contains("[IPv6AcceptRA]\nUseDNS=no\nUseDomains=no"));
        fs::rename(
            scratch.0.join("etc/systemd/network/10-eth0.network"),
            scratch.0.join("etc/systemd/network/10-ens3.network"),
        )
        .unwrap();
        assert!(
            fs::read_to_string(scratch.0.join("etc/systemd/network/10-ens3.network"))
                .unwrap()
                .contains("DNS=1.1.1.1")
        );
    }
}

#[test]
fn alpine_static_dns_disables_both_dhcpcd_and_udhcpc_dns_hooks_globally() {
    let scratch = Scratch::new();
    scratch.write(
        "etc/dhcpcd.conf",
        "slaac hwaddr\ninterface eth0\nmetric 100\n",
    );
    let mut plan = plan(Target::Alpine);
    plan.system.dns = dns();
    let plan = live(plan);
    for _ in 0..2 {
        apply_target_dns(&scratch.0, &plan).unwrap();
    }
    let text = fs::read_to_string(scratch.0.join("etc/dhcpcd.conf")).unwrap();
    assert!(text.find("nohook resolv.conf").unwrap() < text.find("interface eth0").unwrap());
    assert!(text.contains("slaac hwaddr"));
    assert_eq!(text.matches("nohook resolv.conf").count(), 1);
    assert!(
        fs::read_to_string(scratch.0.join("etc/udhcpc/udhcpc.conf"))
            .unwrap()
            .contains("RESOLV_CONF=\"no\"")
    );
}

#[test]
fn automatic_dns_is_a_noop_and_unsafe_target_paths_fail_before_hostname_is_written() {
    let scratch = Scratch::new();
    scratch.write("etc/resolv.conf", "original\n");
    apply_target_dns(&scratch.0, &live(plan(Target::Debian))).unwrap();
    assert_eq!(
        fs::read_to_string(scratch.0.join("etc/resolv.conf")).unwrap(),
        "original\n"
    );
    let outside = scratch.0.join("outside.network");
    fs::write(&outside, "do not edit\n").unwrap();
    fs::create_dir_all(scratch.0.join("etc/systemd/network")).unwrap();
    symlink(
        &outside,
        scratch.0.join("etc/systemd/network/10-eth0.network"),
    )
    .unwrap();
    let mut selected = plan(Target::Debian);
    selected.system.dns = dns();
    selected.post_install.hostname = Some("new-host".into());
    assert!(apply_basic_configuration(&scratch.0, &live(selected), true).is_err());
    assert!(!scratch.0.join("etc/hostname").exists());
    assert_eq!(fs::read_to_string(outside).unwrap(), "do not edit\n");
    assert!(apply_target_dns(std::path::Path::new("/"), &live(plan(Target::Alpine))).is_err());
}

#[test]
fn native_minimal_package_policies_preserve_boot_crypto_databases_and_licenses() {
    for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
        let scratch = Scratch::new();
        scratch.write("etc/pacman.conf","[options]\nArchitecture = auto\n[core]\nServer = https://mirror.example/$repo/os/$arch\n");
        scratch.write("etc/portage/make.conf", "COMMON_FLAGS=\"-O2 -pipe\"\n");
        let mut selected = plan(target);
        selected.system.profile = InstallProfile::Minimal;
        let selected = live(selected);
        let cleanup = apply_package_policy(&scratch.0, &selected).unwrap();
        apply_package_policy(&scratch.0, &selected).unwrap();
        assert!(cleanup.documentation_roots.contains(&"usr/share/doc"));
        assert!(!cleanup.package_cache_roots.is_empty());
        for protected in PRESERVED_ROOTS {
            assert!(!cleanup.documentation_roots.contains(protected));
            assert!(!cleanup.package_cache_roots.contains(protected));
        }
        match target {
            Target::Debian => assert!(
                fs::read_to_string(scratch.0.join("etc/dpkg/dpkg.cfg.d/99-rust-aller-minimal"))
                    .unwrap()
                    .contains("path-include=/usr/share/doc/*/copyright")
            ),
            Target::Arch => {
                let value = fs::read_to_string(scratch.0.join("etc/pacman.conf")).unwrap();
                assert!(value.contains("SigLevel = Required DatabaseOptional"));
                assert!(value.find("NoExtract").unwrap() < value.find("[core]").unwrap());
                assert_eq!(
                    value.matches("# BEGIN rust-aller minimal packages").count(),
                    1
                );
            }
            Target::Gentoo => assert!(
                fs::read_to_string(scratch.0.join("etc/portage/make.conf"))
                    .unwrap()
                    .contains("nodoc noman noinfo")
            ),
            _ => {}
        }
        // A cleanup plan never deletes existing data by itself.
        scratch.write("usr/share/doc/example/README", "still present\n");
        assert!(scratch.0.join("usr/share/doc/example/README").exists());
    }
    let mut selected = plan(Target::Debian);
    selected.system.profile = InstallProfile::Minimal;
    selected.system.keep_docs = true;
    selected.system.keep_package_cache = true;
    let cleanup = cleanup_plan(&live(selected)).unwrap();
    assert!(cleanup.documentation_roots.is_empty());
    assert!(cleanup.package_cache_roots.is_empty());
}

#[test]
fn arch_minimal_policy_rejects_repository_signature_bypasses_and_replaces_global_legacy_policy() {
    assert!(arch_policy("[options]\n[core]\nSigLevel = Never\n", true).is_err());
    assert!(arch_policy("[core]\n", true).is_err());
    let value=arch_policy("[options]\nSigLevel = Never\nArchitecture = auto\n[core]\nServer = https://example.org/$repo/os/$arch\n",true).unwrap();
    assert!(!value.contains("SigLevel = Never"));
    assert!(value.contains("SigLevel = Required DatabaseOptional"));
    assert_eq!(arch_policy(&value, true).unwrap(), value);
}
