use reinstall_rs::deployment::{
    DeploymentManifest, DiskIdentity, NetworkInterface, decode_manifest, encode_manifest,
};
use reinstall_rs::host::sources::{GENTOO_ARCHIVE, GentooStage3};
use reinstall_rs::model::{BootMode, InstallPlan, Target};
use reinstall_rs::targets::install::{
    gentoo_sha512, valid_gentoo_signature, validate_install_options, write_network,
};
use reinstall_rs::worker::install::{
    parse_unmounted_descendants, reject_target_swaps, same_physical_disk, validate_live_root,
};
use reinstall_rs::worker::live::validate_live_plan_with_uid;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustaller-live-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn manifest() -> DeploymentManifest {
    DeploymentManifest {
        disk: DiskIdentity {
            device: "/dev/vda".into(),
            size_bytes: 8 * 1024 * 1024 * 1024,
            serial: "rustaller-test-disk".into(),
            wwn: "".into(),
            partition_table_id: "aabbccdd".into(),
        },
        boot_mode: BootMode::Bios,
        alpine_branch: "v3.24".into(),
        debian_codename: "trixie".into(),
        gentoo_stage3: None,
        network: vec![NetworkInterface {
            name: "ens3".into(),
            mac: "52:54:00:11:22:33".into(),
            ipv4_addresses: vec!["192.0.2.7/32".into()],
            ipv4_gateway: Some("192.0.2.1".into()),
            ipv6_addresses: vec!["2001:db8::7/128".into(), "2001:db8::8/128".into()],
            ipv6_gateway: Some("fe80::1".into()),
            dhcp4: false,
            dhcp6: false,
            accept_ra: false,
        }],
    }
}
fn stage() -> GentooStage3 {
    let timestamp = "20261025T170057Z";
    let filename = format!("stage3-amd64-systemd-{timestamp}.tar.xz");
    let url = format!("{GENTOO_ARCHIVE}/{timestamp}/{filename}");
    GentooStage3 {
        timestamp: timestamp.into(),
        filename,
        size: 280774272,
        digests_url: format!("{url}.DIGESTS"),
        signature_url: format!("{url}.asc"),
        url,
    }
}
fn plan(target: Target) -> InstallPlan {
    let mut plan = InstallPlan::new(target);
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.username = Some("root".into());
    plan.ssh_keys = vec!["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA test".into()];
    plan
}

#[test]
fn deployment_roundtrips_bounded_identity_network_and_gentoo_source_without_fake_pgp_armor() {
    for boot in [BootMode::Bios, BootMode::Efi] {
        let mut value = manifest();
        value.boot_mode = boot;
        value.gentoo_stage3 = Some(stage());
        assert_eq!(
            decode_manifest(&encode_manifest(&value).unwrap()).unwrap(),
            value
        );
    }
    let mut value = manifest();
    value.gentoo_stage3 = Some(stage());
    value.gentoo_stage3.as_mut().unwrap().url = "https://attacker.invalid/stage3.tar.xz".into();
    assert!(encode_manifest(&value).is_err());
    let mut bytes = encode_manifest(&manifest()).unwrap();
    bytes.push(0);
    assert!(decode_manifest(&bytes).is_err());
    for bytes in [b"".as_slice(), b"RDEPLOY\0\x02\0".as_slice()] {
        assert!(decode_manifest(bytes).is_err());
    }
}

#[test]
fn physical_identity_survives_device_renaming_and_requires_all_recorded_identifiers() {
    let expected = manifest().disk;
    let mut found = expected.clone();
    found.device = "/dev/sda".into();
    assert!(same_physical_disk(&expected, &found));
    found.size_bytes += 512;
    assert!(!same_physical_disk(&expected, &found));
    found = expected.clone();
    found.serial = "different".into();
    assert!(!same_physical_disk(&expected, &found));
    found = expected.clone();
    found.partition_table_id = "deadbeef".into();
    assert!(!same_physical_disk(&expected, &found));
    let mut unbound = expected.clone();
    unbound.serial.clear();
    unbound.partition_table_id.clear();
    assert!(!same_physical_disk(&unbound, &expected));
}

#[test]
fn running_root_and_target_inventory_guards_reject_host_mounts_swap_lvm_and_ambiguity() {
    for root in ["rootfs", "tmpfs", "ramfs"] {
        validate_live_root(root).unwrap();
    }
    for root in ["ext4", "overlay", "btrfs", "xfs", ""] {
        assert!(validate_live_root(root).is_err());
    }
    let descendants = parse_unmounted_descendants(
        "/dev/vda disk\n/dev/vda1 part\n/dev/vda2 part\n",
        "/dev/vda",
    )
    .unwrap();
    reject_target_swaps(&descendants, "Filename Type Size Used Priority\n").unwrap();
    assert!(
        reject_target_swaps(
            &descendants,
            "Filename Type Size Used Priority\n/dev/vda2 partition 2048 0 -2\n"
        )
        .is_err()
    );
    assert!(reject_target_swaps(&descendants, "").is_err());
    for text in [
        "/dev/vda disk /\n",
        "/dev/vda disk\n/dev/vg/root lvm\n",
        "/dev/vda disk\n/dev/mapper/root crypt\n",
        "/dev/vda1 part\n",
        "/dev/vda disk\n/dev/vda disk\n",
    ] {
        assert!(parse_unmounted_descendants(text, "/dev/vda").is_err());
    }
}

#[test]
fn all_four_targets_validate_before_mutation_and_reject_unresolved_public_key_sources() {
    for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
        let mut metadata = manifest();
        if target == Target::Gentoo {
            metadata.gentoo_stage3 = Some(stage());
        }
        let selected = validate_live_plan_with_uid(
            plan(target),
            "x86_64",
            4096,
            reinstall_rs::RUNTIME_VERSION,
            0,
        )
        .unwrap();
        validate_install_options(&selected, &metadata).unwrap();
    }
    let mut invalid = plan(Target::Debian);
    invalid.ssh_keys = vec!["/root/client.pub".into()];
    let selected =
        validate_live_plan_with_uid(invalid, "x86_64", 4096, reinstall_rs::RUNTIME_VERSION, 0)
            .unwrap();
    assert!(validate_install_options(&selected, &manifest()).is_err());
    let mut invalid = plan(Target::Alpine);
    invalid.username = Some("option;injection".into());
    assert!(invalid.validate().is_err());
}

#[test]
fn alpine_dual_stack_static_configuration_survives_mac_based_interface_rename() {
    let directory = Scratch::new();
    let metadata = manifest();
    write_network(&directory.0, &metadata, Target::Alpine).unwrap();
    let text = fs::read_to_string(directory.0.join("etc/network/interfaces")).unwrap();
    assert!(text.contains("# mac 52:54:00:11:22:33"));
    assert!(text.contains("iface ens3 inet static"));
    assert!(text.contains("iface ens3 inet6 static"));
    assert!(text.contains("via fe80::1 onlink dev ens3"));
    let rewritten = reinstall_rs::worker::firstboot::rewrite_ifupdown(&text, |mac| {
        assert_eq!(mac, "52:54:00:11:22:33");
        Some("eth0".into())
    });
    let rewritten = String::from_utf8(rewritten).unwrap();
    assert!(rewritten.contains("iface eth0 inet6 static"));
    assert!(rewritten.contains("via 192.0.2.1 onlink dev eth0"));
    assert!(!rewritten.contains("ens3"));
    let dhcpcd = fs::read_to_string(directory.0.join("etc/dhcpcd.conf")).unwrap();
    assert!(!dhcpcd.contains("interface ens3"));
    assert!(!dhcpcd.contains("static routers"));
}

#[test]
fn networkd_uses_mac_match_and_onlink_gateways_but_does_not_pin_dynamic_leases() {
    let directory = Scratch::new();
    let mut metadata = manifest();
    metadata.network[0].dhcp4 = true;
    metadata.network[0].accept_ra = true;
    write_network(&directory.0, &metadata, Target::Debian).unwrap();
    let text = fs::read_to_string(directory.0.join("etc/systemd/network/10-ens3.network")).unwrap();
    assert!(text.contains("MACAddress=52:54:00:11:22:33"));
    assert!(text.contains("DHCP=ipv4"));
    assert!(text.contains("GatewayOnLink=yes"));
    assert!(!text.contains("Address=192.0.2.7"));
    assert!(!text.contains("Address=2001:db8::7"));
}

#[test]
fn gentoo_accepts_verified_subkey_primary_binding_and_rejects_expired_bad_or_multiple_signatures() {
    let good = "[GNUPG:] GOODSIG 2C44695DB9F6043D Gentoo\n[GNUPG:] VALIDSIG 534E4209AB49EEE1C19D96162C44695DB9F6043D 2026-10-26 1792974065 0 4 0 1 8 01 13EBBDBEDE7A12775DFDB1BABB572E0E2D182910\n";
    assert!(valid_gentoo_signature(good));
    assert!(!valid_gentoo_signature(&good.replace(
        "13EBBDBEDE7A12775DFDB1BABB572E0E2D182910",
        "D99EAC7379A850BCE47DA5F29E6438C817072058"
    )));
    assert!(!valid_gentoo_signature(&format!("{good}{good}")));
    assert!(!valid_gentoo_signature(&format!(
        "{good}[GNUPG:] EXPKEYSIG 2C44695DB9F6043D Gentoo\n"
    )));
    let selected = stage();
    let digest = "a".repeat(128);
    let body = format!("# SHA512 HASH\n{digest}  {}\n", selected.filename);
    assert_eq!(gentoo_sha512(&selected, &body).unwrap(), digest);
    assert!(gentoo_sha512(&selected, &format!("{body}{body}")).is_err());
    assert!(gentoo_sha512(&selected, &body.replace(&selected.filename, "other.tar.xz")).is_err());
}
