use rustaller::command::{CommandOutput, MockRunner};
use rustaller::deployment::{DeploymentManifest, DiskIdentity, NetworkInterface};
use rustaller::host::handoff::{
    ALPINE_RELEASE_FINGERPRINT, HostDeployment, HostInit, confirmation_for, confirmation_for_plan,
    detect_init, parse_addresses, pinned_primary_key, prepare, read_disk_identity,
    resolve_public_keys, valid_alpine_signature,
};
use rustaller::host::sources::AlpineNetboot;
use rustaller::model::{BootMode, InstallPlan, SecretString, Target};
use rustaller::worker::initrd_network::AddressFamily;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustaller-handoff-{}-{}",
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
fn output(text: &str) -> CommandOutput {
    CommandOutput::new(Some(0), text.as_bytes().to_vec(), vec![])
}
fn disk() -> DiskIdentity {
    DiskIdentity {
        device: "/dev/vda".into(),
        size_bytes: 8 * 1024_u64.pow(3),
        serial: "acceptance-disk".into(),
        wwn: String::new(),
        partition_table_id: "1234abcd".into(),
    }
}
fn plan() -> InstallPlan {
    let mut plan = InstallPlan::new(Target::Debian);
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.password = Some(SecretString::new("private-password-sentinel"));
    plan
}
fn deployment(plan: &InstallPlan) -> HostDeployment {
    let source=AlpineNetboot {version:"3.24.3".into(),branch:"v3.24".into(),filename:"alpine-netboot-3.24.3-x86_64.tar.gz".into(),url:"https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-netboot-3.24.3-x86_64.tar.gz".into(),sha256:"a".repeat(64),checksum_url:"https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-netboot-3.24.3-x86_64.tar.gz.sha256".into(),signature_url:"https://dl-cdn.alpinelinux.org/alpine/v3.24/releases/x86_64/alpine-netboot-3.24.3-x86_64.tar.gz.asc".into()};
    HostDeployment {
        manifest: DeploymentManifest {
            disk: disk(),
            boot_mode: BootMode::Bios,
            alpine_branch: source.branch.clone(),
            debian_codename: "trixie".into(),
            gentoo_stage3: None,
            network: vec![NetworkInterface {
                name: "eth0".into(),
                mac: "52:54:00:12:34:56".into(),
                ipv4_addresses: vec!["192.0.2.4/24".into()],
                ipv4_gateway: Some("192.0.2.1".into()),
                ipv6_addresses: vec![],
                ipv6_gateway: None,
                dhcp4: true,
                dhcp6: false,
                accept_ra: false,
            }],
        },
        confirmation: confirmation_for_plan(plan, &disk()),
        init: HostInit::Systemd,
        alpine: source,
    }
}

#[test]
fn only_native_init_kexec_adapters_are_selected() {
    assert_eq!(
        detect_init(Path::new("/usr/lib/systemd/systemd")).unwrap(),
        HostInit::Systemd
    );
    assert_eq!(
        detect_init(Path::new("/sbin/openrc-init")).unwrap(),
        HostInit::OpenRcInit
    );
    for init in [
        "/bin/busybox",
        "/sbin/init",
        "/sbin/runit",
        "/usr/bin/python",
    ] {
        assert!(detect_init(Path::new(init)).is_err());
    }
}

#[test]
fn disk_inspection_uses_exact_columns_and_confirmation_binds_every_identifier() {
    let mut runner = MockRunner::new([
        output("8589934592\n"),
        output("acceptance-disk\n"),
        output("\n"),
        output("1234abcd\n"),
    ]);
    assert_eq!(read_disk_identity(&mut runner, "/dev/vda").unwrap(), disk());
    assert_eq!(
        runner
            .calls()
            .iter()
            .map(|call| call.args[1].to_str().unwrap())
            .collect::<Vec<_>>(),
        ["SIZE", "SERIAL", "WWN", "PTUUID"]
    );
    assert_eq!(
        confirmation_for(&disk()),
        "ERASE /dev/vda 8589934592 SERIAL=acceptance-disk WWN= PTUUID=1234abcd"
    );
    let mut rescue = plan();
    rescue.target = Target::Alpine;
    rescue.hold = Some(1);
    assert!(confirmation_for_plan(&rescue, &disk()).starts_with("RESCUE "));
    let mut empty = MockRunner::empty();
    assert!(read_disk_identity(&mut empty, "/dev/../sda").is_err());
    assert!(empty.calls().is_empty());
}

#[test]
fn confirmation_firmware_and_source_identity_fail_before_workspace_or_downloads() {
    let scratch = Scratch::new();
    let plan = plan();
    let inventory = deployment(&plan);
    let workspace = scratch.0.join("new-workspace");
    let mut runner = MockRunner::empty();
    assert!(
        prepare(
            &mut runner,
            &plan,
            &inventory,
            "yes",
            &workspace,
            b"not-a-worker"
        )
        .is_err()
    );
    assert!(!workspace.exists());
    assert!(runner.calls().is_empty());
    let mut wrong_source = deployment(&plan);
    wrong_source.alpine.filename = "../outside".into();
    assert!(
        prepare(
            &mut runner,
            &plan,
            &wrong_source,
            &wrong_source.confirmation,
            &workspace,
            b"not-a-worker"
        )
        .is_err()
    );
    assert!(!workspace.exists());
    assert!(runner.calls().is_empty());
    let mut forced = plan;
    forced.force_boot_mode = Some(BootMode::Efi);
    assert!(
        prepare(
            &mut runner,
            &forced,
            &inventory,
            &inventory.confirmation,
            &workspace,
            b"not-a-worker"
        )
        .is_err()
    );
    assert!(runner.calls().is_empty());
}

#[test]
fn changed_disk_identity_stops_before_any_download_or_staging() {
    let scratch = Scratch::new();
    let plan = plan();
    let inventory = deployment(&plan);
    let workspace = scratch.0.join("new-workspace");
    let mut runner = MockRunner::new([
        output("8589934592\n"),
        output("different-disk\n"),
        output("\n"),
        output("1234abcd\n"),
    ]);
    assert!(
        prepare(
            &mut runner,
            &plan,
            &inventory,
            &inventory.confirmation,
            &workspace,
            b"not-a-worker"
        )
        .is_err()
    );
    assert_eq!(runner.calls().len(), 4);
    assert!(!workspace.exists());
}

#[test]
fn release_key_parsing_rejects_unpinned_or_ambiguous_primary_keys_and_bad_status() {
    let colons = format!(
        "pub:-:4096:1:key:0:0::::::\nfpr:::::::::{ALPINE_RELEASE_FINGERPRINT}:\nsub:-:4096:1:subkey:0:0::::::\nfpr:::::::::{}:\n",
        "b".repeat(40)
    );
    assert!(pinned_primary_key(&colons));
    assert!(!pinned_primary_key(&format!("{colons}{colons}")));
    assert!(!pinned_primary_key(
        &colons.replace(ALPINE_RELEASE_FINGERPRINT, &"c".repeat(40))
    ));
    let valid = format!(
        "[GNUPG:] GOODSIG key signer\n[GNUPG:] VALIDSIG {} 2026-10-26 0 0 4 0 1 8 00 {ALPINE_RELEASE_FINGERPRINT}\n",
        "b".repeat(40)
    );
    assert!(valid_alpine_signature(&valid));
    for status in [
        "BADSIG",
        "ERRSIG",
        "EXPSIG",
        "EXPKEYSIG",
        "REVKEYSIG",
        "NO_PUBKEY",
    ] {
        assert!(!valid_alpine_signature(&format!(
            "{valid}[GNUPG:] {status} key\n"
        )));
    }
    assert!(!valid_alpine_signature(&format!("{valid}{valid}")));
    assert!(!valid_alpine_signature(
        &valid.replace(ALPINE_RELEASE_FINGERPRINT, &"c".repeat(40))
    ));
}

#[test]
fn key_resolution_reads_sources_on_host_and_validates_every_unique_key_without_echoing() {
    let scratch = Scratch::new();
    let file = scratch.0.join("client.pub");
    fs::write(
        &file,
        "# ignored\nssh-ed25519 AAAA first\nssh-rsa BBBB second\n",
    )
    .unwrap();
    let mut runner = MockRunner::new([output("fingerprint1"), output("fingerprint2")]);
    let result = resolve_public_keys(
        &mut runner,
        &scratch.0,
        &[
            file.to_str().unwrap().into(),
            "ssh-ed25519 AAAA first".into(),
        ],
    )
    .unwrap();
    assert_eq!(result, ["ssh-ed25519 AAAA first", "ssh-rsa BBBB second"]);
    assert_eq!(runner.calls().len(), 2);
    assert!(
        runner
            .calls()
            .iter()
            .all(|call| call.program == "ssh-keygen")
    );
    let debug = format!("{:?}", runner.calls());
    assert!(!debug.contains("AAAA"));
    assert!(!debug.contains("BBBB"));
    let other = Scratch::new();
    let private = other.0.join("accidental-private");
    fs::write(
        &private,
        "-----BEGIN OPENSSH PRIVATE KEY-----\nprivate-sentinel\n",
    )
    .unwrap();
    let mut empty = MockRunner::empty();
    let error = resolve_public_keys(&mut empty, &other.0, &[private.to_str().unwrap().into()])
        .err()
        .unwrap()
        .to_string();
    assert!(!error.contains("private-sentinel"));
    assert!(empty.calls().is_empty());
}

#[test]
fn network_address_parser_preserves_multiple_cidrs_but_rejects_missing_family_fields() {
    let text = "2: eth0 inet 192.0.2.4/24 brd 192.0.2.255 scope global dynamic eth0\n2: eth0 inet 192.0.2.5/24 scope global eth0\n";
    assert_eq!(
        parse_addresses(text, AddressFamily::Ipv4).unwrap(),
        ["192.0.2.4/24", "192.0.2.5/24"]
    );
    assert!(parse_addresses("2: eth0 missing-address\n", AddressFamily::Ipv4).is_err());
    assert!(parse_addresses("2: eth0 inet\n", AddressFamily::Ipv4).is_err());
}
