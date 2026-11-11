use rustaller::command::{CommandOutput, CommandRunner, CommandSpec, MockRunner, PipelineOutput};
use rustaller::config::{CongestionControl, Ipv6Policy, NetworkConfig, QueueDiscipline};
use rustaller::config::{DnsConfig, DnsMode};
use rustaller::host::current::{
    CurrentConfig, CurrentError, CurrentFacts, apply_current_at, detect_target_at,
};
use rustaller::model::Target;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFj2vob6MoYTTn+NqUXOVKXyI7B+qnZbJ4soW6uF4ecl test";

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustaller-current-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(path.join("etc")).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).unwrap()
    }
    fn account(&self, name: &str) {
        let metadata = fs::metadata(&self.0).unwrap();
        self.write(
            "etc/passwd",
            format!(
                "{name}:x:{}:{}::/home/{name}:/bin/sh\n",
                metadata.uid(),
                metadata.gid()
            ),
        );
        fs::create_dir_all(self.0.join(format!("home/{name}"))).unwrap();
        fs::set_permissions(
            self.0.join(format!("home/{name}")),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    fn backups_absent(&self) -> bool {
        !self.0.join("var/backups/rustaller-current").exists()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn facts() -> CurrentFacts {
    CurrentFacts {
        target: Target::Debian,
        architecture: "x86_64".into(),
        effective_uid: 0,
        kernel_config: String::new(),
        ipv4_available: true,
    }
}
fn out(status: i32, text: &str) -> CommandOutput {
    CommandOutput::new(Some(status), text.as_bytes().to_vec(), vec![])
}
fn ssh_outputs(port: u16, key_count: usize) -> MockRunner {
    let mut outputs = vec![
        out(0, ""),
        out(
            0,
            &format!("port {port}\nlistenaddress 0.0.0.0:{port}\nlistenaddress [::]:{port}\n"),
        ),
    ];
    if key_count > 0 {
        outputs.push(out(0,"pubkeyauthentication yes\npermitrootlogin prohibit-password\nauthorizedkeysfile .ssh/authorized_keys .ssh/authorized_keys2\n"));
        outputs.extend((0..key_count).map(|_| out(0, "256 SHA256:test test (ED25519)\n")));
    }
    MockRunner::new(outputs)
}

#[test]
fn distribution_detection_follows_only_an_in_root_os_release_link() {
    let root = Scratch::new();
    root.write("usr/lib/os-release", "ID=ubuntu\nID_LIKE=debian\n");
    symlink("../usr/lib/os-release", root.0.join("etc/os-release")).unwrap();
    assert_eq!(detect_target_at(&root.0).unwrap(), Target::Debian);
    fs::remove_file(root.0.join("etc/os-release")).unwrap();
    symlink("../../../outside", root.0.join("etc/os-release")).unwrap();
    assert!(detect_target_at(&root.0).is_err());
    fs::remove_file(root.0.join("etc/os-release")).unwrap();
    root.write("etc/os-release", "ID=unsupported\n");
    assert!(matches!(
        detect_target_at(&root.0),
        Err(CurrentError::UnsupportedDistribution)
    ));
}

#[test]
fn empty_draft_is_noop_and_current_test_seam_rejects_the_real_root() {
    let root = Scratch::new();
    let mut runner = MockRunner::empty();
    let report =
        apply_current_at(&mut runner, &root.0, &CurrentConfig::default(), &facts()).unwrap();
    assert!(report.changed_files.is_empty());
    assert!(report.backup_dir.is_none());
    assert!(runner.calls().is_empty());
    assert!(root.backups_absent());
    assert!(matches!(
        apply_current_at(
            &mut runner,
            Path::new("/"),
            &CurrentConfig::default(),
            &facts()
        ),
        Err(CurrentError::InvalidRoot)
    ));
    let mut invalid = facts();
    invalid.effective_uid = 1000;
    assert!(matches!(
        apply_current_at(&mut runner, &root.0, &CurrentConfig::default(), &invalid),
        Err(CurrentError::RootRequired)
    ));
    invalid = facts();
    invalid.architecture = "aarch64".into();
    assert!(matches!(
        apply_current_at(&mut runner, &root.0, &CurrentConfig::default(), &invalid),
        Err(CurrentError::UnsupportedArchitecture)
    ));
}

#[test]
fn hostname_timezone_backups_preserve_original_owner_mode_and_links_and_apply_idempotently() {
    let root = Scratch::new();
    root.write("etc/hostname", "old-vps\n");
    fs::set_permissions(
        root.0.join("etc/hostname"),
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let before = fs::metadata(root.0.join("etc/hostname")).unwrap();
    root.write(
        "usr/share/zoneinfo/Etc/UTC",
        include_bytes!("fixtures/zoneinfo/UTC"),
    );
    root.write("outside", "do not follow\n");
    symlink(root.0.join("outside"), root.0.join("etc/localtime")).unwrap();
    let config = CurrentConfig {
        hostname: Some("new-vps".into()),
        timezone: Some("Etc/UTC".into()),
        ..Default::default()
    };
    let mut runner = MockRunner::empty();
    let report = apply_current_at(&mut runner, &root.0, &config, &facts()).unwrap();
    let backup = report.backup_dir.unwrap();
    assert_eq!(report.changed_files.len(), 3);
    assert_eq!(root.read("etc/hostname"), "new-vps\n");
    assert_eq!(root.read("etc/timezone"), "Etc/UTC\n");
    assert_eq!(
        fs::read(root.0.join("etc/localtime")).unwrap(),
        include_bytes!("fixtures/zoneinfo/UTC")
    );
    assert_eq!(root.read("outside"), "do not follow\n");
    assert_eq!(
        fs::read_link(backup.join("etc/localtime")).unwrap(),
        root.0.join("outside")
    );
    assert_eq!(
        fs::read_to_string(backup.join("etc/hostname")).unwrap(),
        "old-vps\n"
    );
    let after = fs::metadata(root.0.join("etc/hostname")).unwrap();
    assert_eq!(after.uid(), before.uid());
    assert_eq!(after.gid(), before.gid());
    assert_eq!(after.permissions().mode() & 0o777, 0o640);
    assert_eq!(
        fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(backup.join("etc/hostname"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let second = apply_current_at(&mut runner, &root.0, &config, &facts()).unwrap();
    assert!(second.changed_files.is_empty());
    assert!(second.backup_dir.is_none());
    assert_eq!(
        fs::read_dir(root.0.join("var/backups/rustaller-current"))
            .unwrap()
            .count(),
        1
    );
    assert!(runner.calls().is_empty());
}

#[test]
fn dns_and_mtu_edits_compose_without_changing_host_resolver_or_running_services() {
    let root = Scratch::new();
    root.write(
        "etc/systemd/network/10-eth0.network",
        "[Match]\nMACAddress=aa:bb:cc:dd:ee:ff\n[Network]\nDHCP=yes\n",
    );
    root.write("outside", "resolver outside\n");
    symlink(root.0.join("outside"), root.0.join("etc/resolv.conf")).unwrap();
    let config = CurrentConfig {
        dns: DnsConfig {
            mode: DnsMode::Static,
            servers: vec![
                "1.1.1.1".parse().unwrap(),
                "2606:4700:4700::1111".parse().unwrap(),
            ],
            search_domains: vec!["example.internal".into()],
        },
        network: NetworkConfig {
            congestion: CongestionControl::Bbr,
            qdisc: QueueDiscipline::Fq,
            mtu: Some(1450),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut selected = facts();
    selected.kernel_config = "CONFIG_TCP_CONG_BBR=m\nCONFIG_NET_SCH_FQ=y\n".into();
    let mut runner = MockRunner::empty();
    let report = apply_current_at(&mut runner, &root.0, &config, &selected).unwrap();
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains("lookups immediately"))
    );
    assert_eq!(root.read("outside"), "resolver outside\n");
    assert!(root.read("etc/resolv.conf").contains("nameserver 1.1.1.1"));
    let network = root.read("etc/systemd/network/10-eth0.network");
    assert!(network.contains("DNS=1.1.1.1"));
    assert!(network.contains("MTUBytes=1450"));
    assert!(network.contains("UseDNS=no"));
    assert!(
        root.read("etc/modules-load.d/90-rustaller-current-network.conf")
            .contains("tcp_bbr")
    );
    let repeated = apply_current_at(&mut runner, &root.0, &config, &selected).unwrap();
    assert!(repeated.changed_files.is_empty());
    assert_eq!(root.read("etc/systemd/network/10-eth0.network"), network);
    assert!(runner.calls().is_empty());
}

#[test]
fn unsupported_or_unsafe_network_configuration_fails_before_hostname_publication() {
    let root = Scratch::new();
    root.write("etc/hostname", "original\n");
    let mut runner = MockRunner::empty();
    let mut config = CurrentConfig {
        hostname: Some("new-host".into()),
        dns: DnsConfig {
            mode: DnsMode::Static,
            servers: vec!["1.1.1.1".parse().unwrap()],
            search_domains: vec![],
        },
        ..Default::default()
    };
    assert!(matches!(
        apply_current_at(&mut runner, &root.0, &config, &facts()),
        Err(CurrentError::UnsupportedNetworkManager)
    ));
    assert_eq!(root.read("etc/hostname"), "original\n");
    assert!(root.backups_absent());
    config.dns = DnsConfig::default();
    config.network.ipv6 = Ipv6Policy::Disable;
    let mut selected = facts();
    selected.ipv4_available = false;
    assert!(matches!(
        apply_current_at(&mut runner, &root.0, &config, &selected),
        Err(CurrentError::Ipv4Required)
    ));
    assert_eq!(root.read("etc/hostname"), "original\n");
    config.network = NetworkConfig {
        congestion: CongestionControl::Bbr,
        qdisc: QueueDiscipline::Fq,
        ..Default::default()
    };
    assert!(apply_current_at(&mut runner, &root.0, &config, &facts()).is_err());
    assert_eq!(root.read("etc/hostname"), "original\n");
}

#[test]
fn unsafe_parent_hardlinked_destination_and_missing_ssh_do_not_publish_other_fields() {
    let root = Scratch::new();
    root.write("etc/hostname", "old\n");
    root.write("original", "do not edit\n");
    fs::remove_file(root.0.join("etc/hostname")).unwrap();
    fs::hard_link(root.0.join("original"), root.0.join("etc/hostname")).unwrap();
    let mut runner = MockRunner::empty();
    let config = CurrentConfig {
        hostname: Some("new-host".into()),
        ..Default::default()
    };
    assert!(apply_current_at(&mut runner, &root.0, &config, &facts()).is_err());
    assert_eq!(root.read("original"), "do not edit\n");
    assert!(root.backups_absent());
    fs::remove_file(root.0.join("etc/hostname")).unwrap();
    root.write("etc/hostname", "old\n");
    let config = CurrentConfig {
        hostname: Some("new-host".into()),
        ssh_port: Some(2222),
        ..Default::default()
    };
    assert!(matches!(
        apply_current_at(&mut runner, &root.0, &config, &facts()),
        Err(CurrentError::SshValidationFailed)
    ));
    assert_eq!(root.read("etc/hostname"), "old\n");
    assert!(runner.calls().is_empty());
    assert!(root.backups_absent());
    let other = Scratch::new();
    fs::rename(root.0.join("etc"), root.0.join("old-etc")).unwrap();
    symlink(&other.0, root.0.join("etc")).unwrap();
    assert!(
        apply_current_at(
            &mut runner,
            &root.0,
            &CurrentConfig {
                hostname: Some("new-host".into()),
                ..Default::default()
            },
            &facts()
        )
        .is_err()
    );
    assert!(!other.0.join("hostname").exists());
}

#[test]
fn ssh_port_and_existing_account_keys_are_validated_then_backed_up_and_persisted_once() {
    let root = Scratch::new();
    root.account("alice");
    root.write("etc/ssh/sshd_config","# original\nPort 22\nInclude /etc/ssh/sshd_config.d/*.conf\nMatch User backup\nPasswordAuthentication no\n");
    root.write(
        "home/alice/.ssh/authorized_keys",
        "ssh-ed25519 AAAA existing\n",
    );
    fs::set_permissions(
        root.0.join("home/alice/.ssh"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    root.write("keys/client.pub", format!("{KEY}\n"));
    let config = CurrentConfig {
        username: Some("alice".into()),
        ssh_port: Some(2222),
        ssh_keys: vec![KEY.into(), "/keys/client.pub".into()],
        ..Default::default()
    };
    let mut runner = ssh_outputs(2222, 2);
    let report = apply_current_at(&mut runner, &root.0, &config, &facts()).unwrap();
    let backup = report.backup_dir.unwrap();
    assert!(
        fs::read_to_string(backup.join("etc/ssh/sshd_config"))
            .unwrap()
            .contains("Port 22")
    );
    let sshd = root.read("etc/ssh/sshd_config");
    assert!(sshd.starts_with("# BEGIN rustaller SSH\nPort 2222\n"));
    assert!(!sshd.contains("Port 22\n"));
    assert!(sshd.contains("Match User backup"));
    let keys = root.read("home/alice/.ssh/authorized_keys");
    assert_eq!(keys.lines().filter(|key| *key == KEY).count(), 1);
    assert!(keys.contains("existing"));
    let metadata = fs::metadata(root.0.join("home/alice/.ssh/authorized_keys")).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        metadata.uid(),
        fs::metadata(root.0.join("home/alice")).unwrap().uid()
    );
    assert!(
        runner
            .calls()
            .iter()
            .all(|call| call.program == "sshd" || call.program == "ssh-keygen")
    );
    let mut repeated = ssh_outputs(2222, 2);
    let repeated = apply_current_at(&mut repeated, &root.0, &config, &facts()).unwrap();
    assert!(repeated.changed_files.is_empty());
    assert!(repeated.backup_dir.is_none());
}

#[test]
fn native_ssh_rejection_conflicting_port_and_unsafe_authorized_keys_fail_before_any_publication() {
    let root = Scratch::new();
    root.account("alice");
    root.write("etc/hostname", "old\n");
    root.write("etc/ssh/sshd_config", "Port 22\n");
    let mut config = CurrentConfig {
        hostname: Some("new-host".into()),
        ssh_port: Some(2222),
        ..Default::default()
    };
    let mut rejected = MockRunner::new([out(1, "")]);
    assert!(matches!(
        apply_current_at(&mut rejected, &root.0, &config, &facts()),
        Err(CurrentError::SshValidationFailed)
    ));
    assert_eq!(root.read("etc/hostname"), "old\n");
    assert!(root.backups_absent());
    let mut conflict = MockRunner::new([out(0, ""), out(0, "port 22\nlistenaddress 0.0.0.0:22\n")]);
    assert!(matches!(
        apply_current_at(&mut conflict, &root.0, &config, &facts()),
        Err(CurrentError::ConflictingSshConfiguration)
    ));
    assert_eq!(root.read("etc/hostname"), "old\n");
    config.username = Some("alice".into());
    config.ssh_keys = vec![KEY.into()];
    fs::create_dir_all(root.0.join("home/alice/.ssh")).unwrap();
    fs::set_permissions(
        root.0.join("home/alice/.ssh"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    root.write("outside", "keep\n");
    symlink(
        root.0.join("outside"),
        root.0.join("home/alice/.ssh/authorized_keys"),
    )
    .unwrap();
    let mut runner = ssh_outputs(2222, 1);
    assert!(apply_current_at(&mut runner, &root.0, &config, &facts()).is_err());
    assert_eq!(root.read("outside"), "keep\n");
    assert_eq!(root.read("etc/hostname"), "old\n");
    assert!(root.backups_absent());
}

#[test]
fn root_forced_commands_policy_is_rejected_without_modifying_login_configuration() {
    let root = Scratch::new();
    root.write("etc/passwd", "root:x:0:0:root:/root:/bin/sh\n");
    root.write("etc/ssh/sshd_config", "Port 22\n");
    let config = CurrentConfig {
        username: Some("root".into()),
        ssh_keys: vec![KEY.into()],
        ..Default::default()
    };
    let mut runner = MockRunner::new([
        out(0, ""),
        out(0, "port 22\n"),
        out(
            0,
            "pubkeyauthentication yes\npermitrootlogin forced-commands-only\nauthorizedkeysfile .ssh/authorized_keys\n",
        ),
    ]);
    assert!(matches!(
        apply_current_at(&mut runner, &root.0, &config, &facts()),
        Err(CurrentError::ConflictingSshConfiguration)
    ));
    assert_eq!(root.read("etc/ssh/sshd_config"), "Port 22\n");
    assert!(root.backups_absent());
}

struct ConcurrentRunner {
    inner: MockRunner,
    path: PathBuf,
    changed: bool,
}
impl CommandRunner for ConcurrentRunner {
    fn run(&mut self, spec: &CommandSpec) -> io::Result<CommandOutput> {
        if !self.changed {
            fs::write(&self.path, "concurrent edit\n")?;
            self.changed = true;
        }
        self.inner.run(spec)
    }
    fn run_pipeline(&mut self, _: &[CommandSpec]) -> io::Result<PipelineOutput> {
        panic!("no pipelines")
    }
}

#[test]
fn concurrent_external_edit_is_detected_before_backups_or_configuration_publication() {
    let root = Scratch::new();
    root.write("etc/hostname", "old\n");
    root.write("etc/ssh/sshd_config", "Port 22\n");
    let config = CurrentConfig {
        hostname: Some("new-host".into()),
        ssh_port: Some(2222),
        ..Default::default()
    };
    let mut runner = ConcurrentRunner {
        inner: ssh_outputs(2222, 0),
        path: root.0.join("etc/hostname"),
        changed: false,
    };
    assert!(matches!(
        apply_current_at(&mut runner, &root.0, &config, &facts()),
        Err(CurrentError::ConcurrentChange)
    ));
    assert_eq!(root.read("etc/hostname"), "concurrent edit\n");
    assert_eq!(root.read("etc/ssh/sshd_config"), "Port 22\n");
    assert!(root.backups_absent());
}

#[test]
fn backup_directory_must_not_be_writable_by_other_users() {
    let root = Scratch::new();
    root.write("etc/hostname", "old\n");
    fs::create_dir_all(root.0.join("var/backups/rustaller-current")).unwrap();
    fs::set_permissions(
        root.0.join("var/backups/rustaller-current"),
        fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    let mut runner = MockRunner::empty();
    let config = CurrentConfig {
        hostname: Some("new-host".into()),
        ..Default::default()
    };
    assert!(matches!(
        apply_current_at(&mut runner, &root.0, &config, &facts()),
        Err(CurrentError::UnsafePath)
    ));
    assert_eq!(root.read("etc/hostname"), "old\n");
    assert_eq!(
        fs::read_dir(root.0.join("var/backups/rustaller-current"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn current_config_validation_and_debug_never_expose_public_key_sources_or_accept_private_data() {
    let config = CurrentConfig {
        ssh_keys: vec![KEY.into()],
        ..Default::default()
    };
    assert!(!format!("{config:?}").contains(KEY));
    for source in [
        "-----BEGIN OPENSSH PRIVATE KEY-----",
        "https://example.org/key.pub",
        "github:user",
        "relative.pub",
    ] {
        assert!(
            CurrentConfig {
                ssh_keys: vec![source.into()],
                ..Default::default()
            }
            .validate(Target::Debian)
            .is_err()
        );
    }
    assert!(
        CurrentConfig {
            username: Some("_service".into()),
            ..Default::default()
        }
        .validate(Target::Debian)
        .is_ok()
    );
    assert!(
        CurrentConfig {
            ssh_port: Some(0),
            ..Default::default()
        }
        .validate(Target::Debian)
        .is_err()
    );
}
