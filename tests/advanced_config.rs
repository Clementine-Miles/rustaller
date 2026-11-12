use rustaller::codec::{decode_plan, encode_plan};
use rustaller::command::{
    CommandInput, CommandOutput, CommandRunner, CommandSpec, MockRunner, PipelineOutput,
    SwapSafeRunner,
};
use rustaller::config::{
    AddressPreference, CongestionControl, Ipv6Policy, NetworkConfig, QueueDiscipline,
};
use rustaller::host::args::{CliOutcome, parse_args};
use rustaller::host::preflight::{KexecError, validate_one_reboot_support};
use rustaller::host::ssh_keygen::{KeyType, KeygenOptions, generate};
use rustaller::model::{InstallPlan, Target};
use rustaller::shared::network::{alpine_mtu, apply_network_configuration};
use rustaller::shared::swapfile::prepare_swapfile;
use rustaller::worker::live::{ValidatedLivePlan, validate_live_plan_with_uid};
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
// Execute only the bounded temporary-file allocator; inventory and mkswap stay
// mocked. This checks real dd allocation without touching swap or block devices.
struct SwapIoRunner(MockRunner);
impl CommandRunner for SwapIoRunner {
    fn run(&mut self, spec: &CommandSpec) -> io::Result<CommandOutput> {
        if spec.program == "dd" {
            let output = std::process::Command::new("dd").args(&spec.args).output()?;
            assert!(output.status.success());
        }
        self.0.run(spec)
    }
    fn run_pipeline(&mut self, specs: &[CommandSpec]) -> io::Result<PipelineOutput> {
        self.0.run_pipeline(specs)
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustaller-advanced-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(path.join("etc")).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).unwrap()
    }
    fn service(&self, name: &str) {
        let relative = format!("etc/init.d/{name}");
        self.write(&relative, "test fixture; never executed\n");
        fs::set_permissions(self.0.join(relative), fs::Permissions::from_mode(0o755)).unwrap();
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
    validate_live_plan_with_uid(plan, "x86_64", 2048, rustaller::RUNTIME_VERSION, 0).unwrap()
}
fn out(status: i32, text: &str) -> CommandOutput {
    CommandOutput::new(Some(status), text.as_bytes().to_vec(), vec![])
}
fn tuned() -> NetworkConfig {
    NetworkConfig {
        congestion: CongestionControl::Bbr,
        qdisc: QueueDiscipline::Fq,
        mtu: Some(1450),
        buffer_mib: Some(16),
        mtu_probing: Some(1),
        fastopen: Some(3),
        syncookies: Some(true),
        ..Default::default()
    }
}
const KERNEL: &str = "CONFIG_TCP_CONG_BBR=m\nCONFIG_NET_SCH_FQ=y\n";

#[test]
fn advanced_cli_roundtrips_for_four_targets_and_rejects_incompatible_or_out_of_range_values() {
    for target in ["alpine", "debian", "arch", "gentoo"] {
        let CliOutcome::Plan(selected) = parse_args(
            [
                target,
                "--tcp-congestion=bbr",
                "--qdisc=fq",
                "--mtu=1450",
                "--tcp-buffer-mib=16",
                "--somaxconn=8192",
                "--syn-backlog=8192",
                "--netdev-backlog=16384",
                "--tcp-mtu-probing=1",
                "--tcp-fastopen=3",
                "--tcp-syncookies=1",
                "--ipv6=enable",
                "--swapfile-mib=2048",
                "--swappiness=10",
            ],
            Some("https://assets.example/reinstall"),
        )
        .unwrap() else {
            panic!("plan")
        };
        let decoded = decode_plan(&encode_plan(&selected).unwrap()).unwrap();
        assert_eq!(decoded.system, selected.system);
        assert_eq!(decoded.system.swap.size_mib, Some(2048));
        assert_eq!(decoded.system.network.mtu, Some(1450));
    }
    for options in [
        vec!["--mtu=1279"],
        vec!["--mtu=9001"],
        vec!["--tcp-buffer-mib=65"],
        vec!["--somaxconn=127"],
        vec!["--tcp-congestion=bbr"],
        vec!["--tcp-fastopen=4"],
        vec!["--tcp-mtu-probing=3"],
        vec!["--tcp-syncookies=2"],
        vec!["--swapfile-mib=0"],
        vec!["--swapfile-mib=65537"],
        vec!["--swappiness=10"],
        vec!["--swapfile-mib=1024", "--swappiness=201"],
        vec!["--ipv6=disable", "--dns=2606:4700:4700::1111"],
    ] {
        let mut args = vec!["debian"];
        args.extend(options);
        assert!(parse_args(args, Some("https://assets.example/reinstall")).is_err());
    }
    assert!(
        parse_args(
            ["alpine", "--address-preference=ipv4"],
            Some("https://assets.example/reinstall")
        )
        .is_err()
    );
    for target in ["iso", "reset", "netboot.xyz"] {
        assert!(parse_args([target, "--swapfile-mib=2048"], None).is_err());
    }
    assert!(
        parse_args(
            ["alpine", "--hold=1", "--ipv6=enable"],
            Some("https://assets.example/reinstall")
        )
        .is_err()
    );
    assert!(plan(Target::Debian).system.is_default());
}

#[test]
fn networkd_settings_survive_renaming_and_use_the_target_kernel_and_native_files() {
    for target in [Target::Debian, Target::Arch, Target::Gentoo] {
        let scratch = Scratch::new();
        scratch.write(
            "etc/systemd/network/10-eth0.network",
            "[Match]\nMACAddress=aa:bb:cc:dd:ee:ff\n[Network]\nDHCP=yes\n",
        );
        scratch.write("etc/gai.conf", "# existing comment\n");
        scratch.write(
            "etc/systemd/network/00-lo.network",
            "[Match]\nName = lo\n[Network]\n",
        );
        scratch.write("etc/resolv.conf", "unchanged\n");
        let mut selected = plan(target);
        selected.system.network = tuned();
        selected.system.network.address_preference = AddressPreference::Ipv4;
        let selected = live(selected);
        let services = apply_network_configuration(&scratch.0, &selected, KERNEL).unwrap();
        assert!(services.openrc_boot.is_empty());
        let text = scratch.read("etc/sysctl.d/90-rustaller-network.conf");
        assert!(text.contains("tcp_congestion_control = bbr"));
        assert!(text.contains("16777216"));
        assert!(!text.contains("accept_ra"));
        assert!(!text.contains("kernel.panic"));
        assert!(!text.contains("tcp_adv_win_scale"));
        assert!(
            scratch
                .read("etc/modules-load.d/90-rustaller-network.conf")
                .contains("tcp_bbr")
        );
        assert!(
            scratch
                .read("etc/gai.conf")
                .contains("precedence ::1/128 50")
        );
        assert_eq!(scratch.read("etc/resolv.conf"), "unchanged\n");
        assert!(
            !scratch
                .read("etc/systemd/network/00-lo.network")
                .contains("MTUBytes")
        );
        let before = scratch.read("etc/systemd/network/10-eth0.network");
        assert!(before.contains("MTUBytes=1450"));
        fs::rename(
            scratch.0.join("etc/systemd/network/10-eth0.network"),
            scratch.0.join("etc/systemd/network/10-ens3.network"),
        )
        .unwrap();
        apply_network_configuration(&scratch.0, &selected, KERNEL).unwrap();
        assert_eq!(scratch.read("etc/systemd/network/10-ens3.network"), before);
    }
}

#[test]
fn network_defaults_are_noop_and_unsupported_kernel_or_symlinks_fail_before_publication() {
    let scratch = Scratch::new();
    apply_network_configuration(&scratch.0, &live(plan(Target::Debian)), "").unwrap();
    assert!(!scratch.0.join("etc/sysctl.d").exists());
    let mut selected = plan(Target::Debian);
    selected.system.network = tuned();
    let selected = live(selected);
    assert!(apply_network_configuration(&scratch.0, &selected, "").is_err());
    assert!(!scratch.0.join("etc/sysctl.d").exists());
    scratch.write("outside", "unchanged");
    fs::create_dir_all(scratch.0.join("etc/systemd/network")).unwrap();
    symlink(
        scratch.0.join("outside"),
        scratch.0.join("etc/systemd/network/10-eth0.network"),
    )
    .unwrap();
    assert!(apply_network_configuration(&scratch.0, &selected, KERNEL).is_err());
    assert_eq!(scratch.read("outside"), "unchanged");
    assert!(!scratch.0.join("etc/sysctl.d").exists());
}

#[test]
fn alpine_native_mtu_and_ipv6_policy_preserve_dhcp_and_loopback_without_forcing_ra() {
    let original = "auto lo\niface lo inet loopback\n\nauto eth0\niface eth0 inet dhcp\n    mtu 1500\niface eth0 inet6 auto\n    accept_ra 1\n";
    let text = alpine_mtu(original, 1450).unwrap();
    assert_eq!(text.matches("mtu 1450").count(), 2);
    assert!(!text.contains("mtu 1500"));
    assert!(text.contains("iface lo inet loopback\n\n"));
    assert_eq!(alpine_mtu(&text, 1450).unwrap(), text);
    let scratch = Scratch::new();
    scratch.service("sysctl");
    scratch.service("modules");
    scratch.write("etc/network/interfaces", original);
    let mut selected = plan(Target::Alpine);
    selected.system.network = tuned();
    selected.system.network.ipv6 = Ipv6Policy::Enable;
    let services = apply_network_configuration(&scratch.0, &live(selected), KERNEL).unwrap();
    assert_eq!(services.openrc_boot, ["sysctl", "modules"]);
    assert!(scratch.read("etc/modules").contains("tcp_bbr"));
    assert_eq!(
        fs::read_link(scratch.0.join("etc/runlevels/boot/sysctl")).unwrap(),
        PathBuf::from("/etc/init.d/sysctl")
    );
    let sysctl = scratch.read("etc/sysctl.d/90-rustaller-network.conf");
    assert!(sysctl.contains("all.disable_ipv6 = 0"));
    assert!(!sysctl.contains("accept_ra"));
}

#[test]
fn opt_in_swapfile_is_fully_allocated_private_and_persistent_but_never_activated_on_host() {
    let scratch = Scratch::new();
    scratch.write("etc/fstab", "UUID=original / xfs defaults 0 1\n");
    let mut selected = plan(Target::Debian);
    selected.system.swap.size_mib = Some(64);
    selected.system.swap.swappiness = Some(10);
    let mut runner = SwapSafeRunner::new(SwapIoRunner(MockRunner::new([
        out(0, "xfs\n"),
        out(
            0,
            "Filesystem 1048576-blocks Used Available Capacity Mounted on\n/dev/mock 4096 512 3584 13% /os\n",
        ),
        out(0, ""),
        out(0, ""),
    ])));
    let mut progress = 0;
    assert!(
        prepare_swapfile(&mut runner, &scratch.0, &live(selected), |n, total| {
            assert_eq!(total, 64);
            progress = n;
        })
        .unwrap()
    );
    assert_eq!(progress, 64);
    let metadata = fs::metadata(scratch.0.join("swapfile")).unwrap();
    assert_eq!(metadata.len(), 64 * 1024 * 1024);
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        scratch
            .read("etc/fstab")
            .matches("/swapfile none swap defaults 0 0")
            .count(),
        1
    );
    assert!(scratch.read("etc/fstab").contains("UUID=original"));
    assert!(
        scratch
            .read("etc/sysctl.d/90-rustaller-swap.conf")
            .contains("vm.swappiness = 10")
    );
    let calls = runner.into_inner();
    assert_eq!(calls.0.calls().len(), 4);
    assert_eq!(calls.0.calls()[3].program, "mkswap");
    assert!(
        !calls
            .0
            .calls()
            .iter()
            .any(|c| c.program == "swapon" || c.program == "swapoff")
    );
}

#[test]
fn swapfile_default_collisions_unsafe_filesystems_and_low_space_never_allocate_or_format() {
    let scratch = Scratch::new();
    scratch.service("swap");
    let mut empty = MockRunner::empty();
    assert!(
        !prepare_swapfile(
            &mut empty,
            &scratch.0,
            &live(plan(Target::Alpine)),
            |_, _| {}
        )
        .unwrap()
    );
    assert!(empty.calls().is_empty());
    let mut selected = plan(Target::Alpine);
    selected.system.swap.size_mib = Some(64);
    let selected = live(selected);
    scratch.write("swapfile", "original");
    assert!(prepare_swapfile(&mut empty, &scratch.0, &selected, |_, _| {}).is_err());
    assert_eq!(scratch.read("swapfile"), "original");
    fs::remove_file(scratch.0.join("swapfile")).unwrap();
    scratch.write("etc/fstab", "/swapfile none swap defaults 0 0\n");
    assert!(prepare_swapfile(&mut empty, &scratch.0, &selected, |_, _| {}).is_err());
    assert!(empty.calls().is_empty());
    scratch.write("etc/fstab", "");
    let mut runner = MockRunner::new([out(0, "btrfs\n")]);
    assert!(prepare_swapfile(&mut runner, &scratch.0, &selected, |_, _| {}).is_err());
    assert_eq!(runner.calls().len(), 1);
    assert!(!scratch.0.join("swapfile").exists());
    let mut runner = MockRunner::new([
        out(0, "xfs\n"),
        out(0, "header\n/dev/mock 1024 1000 24 98% /os\n"),
    ]);
    assert!(prepare_swapfile(&mut runner, &scratch.0, &selected, |_, _| {}).is_err());
    assert!(!scratch.0.join("swapfile").exists());
    symlink(scratch.0.join("not-created"), scratch.0.join("swapfile")).unwrap();
    assert!(prepare_swapfile(&mut empty, &scratch.0, &selected, |_, _| {}).is_err());
    assert!(!scratch.0.join("not-created").exists());
}

#[test]
fn mkswap_failure_leaves_fstab_untouched_and_removes_only_its_temporary_file() {
    let scratch = Scratch::new();
    scratch.write("etc/fstab", "original\n");
    scratch.write("keep", "unrelated");
    let mut selected = plan(Target::Debian);
    selected.system.swap.size_mib = Some(64);
    let mut runner = SwapSafeRunner::new(SwapIoRunner(MockRunner::new([
        out(0, "xfs\n"),
        out(0, "header\n/dev/mock 4096 512 3584 13% /os\n"),
        out(0, ""),
        out(1, "failure"),
    ])));
    assert!(prepare_swapfile(&mut runner, &scratch.0, &live(selected), |_, _| {}).is_err());
    assert_eq!(scratch.read("etc/fstab"), "original\n");
    assert_eq!(scratch.read("keep"), "unrelated");
    assert!(!scratch.0.join("swapfile").exists());
    assert!(!fs::read_dir(&scratch.0).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rustaller-swap")
    }));
}

#[test]
fn dd_failure_does_not_format_or_publish_swap() {
    let scratch = Scratch::new();
    scratch.write("etc/fstab", "original\n");
    let mut selected = plan(Target::Debian);
    selected.system.swap.size_mib = Some(64);
    let mut runner = MockRunner::new([
        out(0, "xfs\n"),
        out(0, "header\n/dev/mock 4096 512 3584 13% /os\n"),
        out(1, "No space left on device"),
    ]);
    assert!(prepare_swapfile(&mut runner, &scratch.0, &live(selected), |_, _| {}).is_err());
    assert_eq!(runner.calls().last().unwrap().program, "dd");
    assert_eq!(scratch.read("etc/fstab"), "original\n");
    assert!(!scratch.0.join("swapfile").exists());
    assert!(!fs::read_dir(&scratch.0).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rustaller-swap")
    }));
}

#[test]
fn alpine_swap_enables_native_openrc_boot_service() {
    let scratch = Scratch::new();
    scratch.service("swap");
    scratch.write("etc/fstab", "UUID=root / xfs defaults 0 0\n");
    let mut selected = plan(Target::Alpine);
    selected.system.swap.size_mib = Some(64);
    let mut runner = SwapSafeRunner::new(SwapIoRunner(MockRunner::new([
        out(0, "xfs\n"),
        out(0, "header\n/dev/mock 4096 512 3584 13% /os\n"),
        out(0, ""),
        out(0, ""),
    ])));
    assert!(prepare_swapfile(&mut runner, &scratch.0, &live(selected), |_, _| {}).unwrap());
    assert_eq!(
        fs::read_link(scratch.0.join("etc/runlevels/boot/swap")).unwrap(),
        PathBuf::from("/etc/init.d/swap")
    );
}

#[test]
fn ext4_swapfile_requires_matching_mount_and_stays_inactive_until_target_boot() {
    let scratch = Scratch::new();
    scratch.write("etc/fstab", "UUID=root / ext4 defaults 0 1\n");
    let mut selected = plan(Target::Debian);
    selected.system.root_filesystem = rustaller::config::RootFilesystem::Ext4;
    selected.system.swap.size_mib = Some(64);
    let selected = live(selected);
    let mut mismatch = MockRunner::new([out(0, "xfs\n")]);
    assert!(prepare_swapfile(&mut mismatch, &scratch.0, &selected, |_, _| {}).is_err());
    assert!(!scratch.0.join("swapfile").exists());
    let mut runner = SwapSafeRunner::new(SwapIoRunner(MockRunner::new([
        out(0, "ext4\n"),
        out(0, "header\n/dev/mock 4096 512 3584 13% /os\n"),
        out(0, ""),
        out(0, ""),
    ])));
    assert!(prepare_swapfile(&mut runner, &scratch.0, &selected, |_, _| {}).unwrap());
    assert_eq!(
        fs::metadata(scratch.0.join("swapfile")).unwrap().len(),
        64 * 1024 * 1024
    );
    assert_eq!(
        scratch
            .read("etc/fstab")
            .matches("/swapfile none swap defaults 0 0")
            .count(),
        1
    );
    assert!(
        !runner
            .into_inner()
            .0
            .calls()
            .iter()
            .any(|c| c.program == "swapon" || c.program == "swapoff")
    );
}

struct FakeKeygen {
    calls: Vec<CommandSpec>,
    status: i32,
}
impl CommandRunner for FakeKeygen {
    fn run(&mut self, spec: &CommandSpec) -> io::Result<CommandOutput> {
        self.calls.push(spec.clone());
        assert_eq!(spec.program, "ssh-keygen");
        let output = PathBuf::from(spec.args.last().unwrap());
        fs::write(&output, "PRIVATE-TEST-SENTINEL")?;
        let mut public = output.into_os_string();
        public.push(".pub");
        fs::write(
            PathBuf::from(public),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA test\n",
        )?;
        Ok(out(self.status, ""))
    }
    fn run_pipeline(&mut self, _: &[CommandSpec]) -> io::Result<PipelineOutput> {
        panic!("no pipeline")
    }
}
fn key_options(scratch: &Scratch) -> KeygenOptions {
    KeygenOptions {
        key_type: KeyType::Ed25519,
        rounds: 512,
        cipher: "aes256-ctr".into(),
        rsa_bits: 4096,
        comment: "20261026 $(not-a-shell)".into(),
        output: scratch.0.join("client-key"),
    }
}

#[test]
fn ssh_keygen_calls_system_tool_with_separate_arguments_and_never_serializes_private_key_or_passphrase()
 {
    let scratch = Scratch::new();
    let options = key_options(&scratch);
    let mut runner = FakeKeygen {
        calls: vec![],
        status: 0,
    };
    let key = generate(&mut runner, &options).unwrap();
    let args = runner.calls[0]
        .args
        .iter()
        .map(|a| a.to_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        &args[..9],
        [
            "-o",
            "-a",
            "512",
            "-Z",
            "aes256-ctr",
            "-t",
            "ed25519",
            "-C",
            "20261026 $(not-a-shell)"
        ]
    );
    assert!(!args.contains(&"-N"));
    assert_eq!(runner.calls[0].stdin, CommandInput::Inherit);
    assert_eq!(
        fs::metadata(&key.private_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&key.public_path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    let mut selected = plan(Target::Debian);
    selected.ssh_keys.push(key.public_key);
    let encoded = encode_plan(&selected).unwrap();
    assert!(
        !encoded
            .windows(b"PRIVATE-TEST-SENTINEL".len())
            .any(|s| s == b"PRIVATE-TEST-SENTINEL")
    );
    assert!(
        !encoded
            .windows(b"client-key".len())
            .any(|s| s == b"client-key")
    );
    assert!(!fs::read_dir(&scratch.0).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rustaller-keygen")
    }));
    assert!(generate(&mut runner, &options).is_err());
    assert_eq!(runner.calls.len(), 1);
    assert_eq!(scratch.read("client-key"), "PRIVATE-TEST-SENTINEL");
}

#[test]
fn ssh_keygen_failure_and_invalid_options_do_not_publish_or_overwrite_keys() {
    let scratch = Scratch::new();
    let mut runner = FakeKeygen {
        calls: vec![],
        status: 1,
    };
    let mut options = key_options(&scratch);
    assert!(generate(&mut runner, &options).is_err());
    assert!(!options.output.exists());
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 1);
    options.rounds = 0;
    assert!(generate(&mut runner, &options).is_err());
    options.rounds = 512;
    options.cipher = "arbitrary -N secret".into();
    assert!(generate(&mut runner, &options).is_err());
    options.cipher = "aes256-ctr".into();
    options.output = "relative".into();
    assert!(generate(&mut runner, &options).is_err());
    options = key_options(&scratch);
    symlink(scratch.0.join("not-created"), &options.output).unwrap();
    assert!(generate(&mut runner, &options).is_err());
    assert!(!scratch.0.join("not-created").exists());
    assert_eq!(runner.calls.len(), 1);
}

#[test]
fn single_reboot_contract_fails_closed_without_kexec_and_never_downgrades_to_grub_restart() {
    let config = "CONFIG_KEXEC_FILE=y\nCONFIG_KEXEC_CORE=y\n";
    validate_one_reboot_support(config, "0", Some("[none] integrity confidentiality"), true)
        .unwrap();
    for (disabled, lockdown, tool, expected) in [
        ("1", None, true, KexecError::AdministrativelyDisabled),
        ("", None, true, KexecError::UnknownPolicy),
        (
            "0",
            Some("none [integrity] confidentiality"),
            true,
            KexecError::Lockdown,
        ),
        ("0", Some("unknown"), true, KexecError::UnknownPolicy),
        ("0", None, false, KexecError::ToolMissing),
    ] {
        assert_eq!(
            validate_one_reboot_support(config, disabled, lockdown, tool),
            Err(expected)
        );
    }
    assert_eq!(
        validate_one_reboot_support("CONFIG_KEXEC=y\n", "0", None, true),
        Err(KexecError::KernelUnsupported)
    );
}
