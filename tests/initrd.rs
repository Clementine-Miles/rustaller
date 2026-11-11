use reinstall_rs::codec::decode_plan;
use reinstall_rs::host::initrd::{
    InitrdError, inject_network_bootstrap, install_worker_and_plan, repack_initrd,
    stage_alpine_initrd, unpack_initrd,
};
use reinstall_rs::model::{InstallPlan, SecretString, Target};
use reinstall_rs::worker::initrd_network::InitrdNetworkConfig;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "reinstall-initrd-stage-{}-{nonce}",
            std::process::id()
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

fn automatic_plan() -> InstallPlan {
    let mut plan = InstallPlan::new(Target::Alpine);
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.password = Some(SecretString::new("initrd-secret-sentinel"));
    plan
}

fn worker_elf_header() -> Vec<u8> {
    let mut worker = vec![0; 64];
    worker[..4].copy_from_slice(b"\x7fELF");
    worker[4] = 2;
    worker[5] = 1;
    worker[6] = 1;
    worker[18..20].copy_from_slice(&[0x3e, 0]);
    worker
}

fn worker_bytes() -> Vec<u8> {
    let mut worker = worker_elf_header();
    worker.extend_from_slice(reinstall_rs::RUNTIME_VERSION.as_bytes());
    worker
}

#[test]
fn unpacks_initrd_with_zcat_and_cpio_in_the_isolated_staging_directory() {
    let scratch = Scratch::new();
    let archive = scratch.0.join("source-initrd");
    let staging = scratch.0.join("stage");
    fs::write(&archive, b"compressed fixture").unwrap();
    let mut runner = reinstall_rs::command::MockRunner::new([
        reinstall_rs::command::CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        reinstall_rs::command::CommandOutput::new(Some(0), Vec::new(), Vec::new()),
    ]);
    unpack_initrd(&mut runner, &archive, &staging).unwrap();
    assert!(staging.is_dir());
    assert_eq!(
        fs::metadata(&staging).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(runner.calls()[0].program, "zcat");
    assert_eq!(runner.calls()[0].args, [archive.as_os_str()]);
    assert_eq!(runner.calls()[0].cwd.as_deref(), Some(staging.as_path()));
    assert_eq!(runner.calls()[1].program, "cpio");
    assert_eq!(runner.calls()[1].args, ["-idm"]);
    assert_eq!(runner.calls()[1].cwd.as_deref(), Some(staging.as_path()));
}

#[test]
fn initrd_unpack_uses_the_last_pipeline_command_status_like_the_shell_baseline() {
    let scratch = Scratch::new();
    let archive = scratch.0.join("source-initrd");
    let staging = scratch.0.join("stage");
    fs::write(&archive, b"compressed fixture").unwrap();
    let mut runner = reinstall_rs::command::MockRunner::new([
        reinstall_rs::command::CommandOutput::new(Some(141), Vec::new(), Vec::new()),
        reinstall_rs::command::CommandOutput::new(Some(0), Vec::new(), Vec::new()),
    ]);
    assert!(unpack_initrd(&mut runner, &archive, &staging).is_ok());
    assert!(staging.is_dir());
}

#[test]
fn failed_unpack_removes_only_its_new_staging_directory() {
    let scratch = Scratch::new();
    let archive = scratch.0.join("source-initrd");
    let staging = scratch.0.join("stage");
    fs::write(&archive, b"compressed fixture").unwrap();
    let mut runner = reinstall_rs::command::MockRunner::new([
        reinstall_rs::command::CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        reinstall_rs::command::CommandOutput::new(Some(2), Vec::new(), b"bad archive".to_vec()),
    ]);
    assert!(unpack_initrd(&mut runner, &archive, &staging).is_err());
    assert!(!staging.exists());
}

#[test]
fn repacks_initrd_with_the_legacy_find_cpio_gzip_pipeline_and_atomic_output() {
    let scratch = Scratch::new();
    let staging = scratch.0.join("stage");
    fs::create_dir(&staging).unwrap();
    let archive = scratch.0.join("repacked-initrd");
    let mut runner = reinstall_rs::command::MockRunner::new([
        reinstall_rs::command::CommandOutput::new(Some(0), b"./init\n".to_vec(), Vec::new()),
        reinstall_rs::command::CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        reinstall_rs::command::CommandOutput::new(Some(0), b"gzip-output".to_vec(), Vec::new()),
    ]);
    repack_initrd(&mut runner, &staging, &archive).unwrap();
    assert_eq!(fs::read(&archive).unwrap(), b"gzip-output");
    assert_eq!(runner.calls()[0].program, "find");
    assert_eq!(runner.calls()[0].args, ["."]);
    assert_eq!(runner.calls()[1].program, "cpio");
    assert_eq!(
        runner.calls()[1].args,
        ["--quiet", "-o", "-H", "newc", "-R", "0:0"]
    );
    assert_eq!(runner.calls()[2].program, "gzip");
    assert_eq!(runner.calls()[2].args, ["-1"]);
    assert!(
        runner
            .calls()
            .iter()
            .all(|call| call.cwd.as_deref() == Some(staging.as_path()))
    );
}

#[test]
fn failed_repack_preserves_the_previous_initrd_archive() {
    let scratch = Scratch::new();
    let staging = scratch.0.join("stage");
    fs::create_dir(&staging).unwrap();
    let archive = scratch.0.join("reinstall-initrd");
    fs::write(&archive, b"previous archive").unwrap();
    let mut runner = reinstall_rs::command::MockRunner::new([
        reinstall_rs::command::CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        reinstall_rs::command::CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        reinstall_rs::command::CommandOutput::new(Some(1), b"partial output".to_vec(), Vec::new()),
    ]);
    assert!(repack_initrd(&mut runner, &staging, &archive).is_err());
    assert_eq!(fs::read(&archive).unwrap(), b"previous archive");
}

#[test]
fn stages_network_hook_worker_and_private_plan_as_one_alpine_initrd_payload() {
    let scratch = Scratch::new();
    fs::write(
        scratch.0.join("init"),
        "#!/bin/sh\nconfigure_ip() {\n    setup_dhcp\n}\nexec switch_root\n",
    )
    .unwrap();
    let config = InitrdNetworkConfig::new(
        "00:11:22:33:44:55",
        "192.0.2.20/24",
        "192.0.2.1",
        "",
        "",
        "",
    )
    .unwrap();

    let paths =
        stage_alpine_initrd(&scratch.0, &automatic_plan(), &worker_bytes(), &[config]).unwrap();
    assert!(
        fs::read_to_string(scratch.0.join("init"))
            .unwrap()
            .contains("/reinstall-worker initrd-network '00:11:22:33:44:55'")
    );
    assert_eq!(fs::read(paths.worker).unwrap(), worker_bytes());
    assert!(paths.plan.is_file());
}

#[test]
fn invalid_init_script_prevents_any_payload_from_being_staged() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("init"), "no configure function\n").unwrap();
    assert!(matches!(
        stage_alpine_initrd(&scratch.0, &automatic_plan(), &worker_bytes(), &[]),
        Err(InitrdError::ConfigureIpAnchorNotUnique)
    ));
    assert!(!scratch.0.join("reinstall-worker").exists());
    assert!(!scratch.0.join("configs/install.plan").exists());
}

#[test]
fn initrd_payload_installs_matching_worker_and_private_versioned_plan() {
    let scratch = Scratch::new();
    let paths = install_worker_and_plan(&scratch.0, &automatic_plan(), &worker_bytes()).unwrap();
    assert_eq!(paths.worker, scratch.0.join("reinstall-worker"));
    assert_eq!(paths.plan, scratch.0.join("configs/install.plan"));
    assert_eq!(fs::read(&paths.worker).unwrap(), worker_bytes());
    assert_eq!(
        fs::metadata(&paths.worker).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&paths.plan).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let decoded = decode_plan(&fs::read(&paths.plan).unwrap()).unwrap();
    assert_eq!(decoded.target, Target::Alpine);
    assert_eq!(
        decoded.password.as_ref().map(SecretString::expose),
        Some("initrd-secret-sentinel")
    );
}

#[test]
fn initrd_payload_rejects_worker_version_mismatch_before_creating_any_files() {
    let scratch = Scratch::new();
    let error =
        install_worker_and_plan(&scratch.0, &automatic_plan(), &worker_elf_header()).unwrap_err();
    assert_eq!(error, InitrdError::WorkerVersionMismatch);
    assert!(!scratch.0.join("reinstall-worker").exists());
    assert!(!scratch.0.join("configs/install.plan").exists());
}

#[test]
fn initrd_payload_rejects_a_symlinked_configs_directory() {
    let scratch = Scratch::new();
    let outside = scratch.0.join("outside");
    let staging = scratch.0.join("staging");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(&staging).unwrap();
    std::os::unix::fs::symlink(&outside, staging.join("configs")).unwrap();
    assert!(install_worker_and_plan(&staging, &automatic_plan(), &worker_bytes()).is_err());
    assert!(!outside.join("install.plan").exists());
    assert!(!staging.join("reinstall-worker").exists());
}

#[test]
fn initrd_payload_refuses_to_replace_an_existing_worker_file() {
    let scratch = Scratch::new();
    let worker = scratch.0.join("reinstall-worker");
    fs::write(&worker, b"preserve existing worker").unwrap();
    assert!(install_worker_and_plan(&scratch.0, &automatic_plan(), &worker_bytes()).is_err());
    assert_eq!(fs::read(&worker).unwrap(), b"preserve existing worker");
    assert!(!scratch.0.join("configs/install.plan").exists());
}

#[test]
fn alpine_init_network_hook_runs_worker_with_validated_single_nic_arguments() {
    let source = "#!/bin/sh\nconfigure_ip() {\n    setup_dhcp\n}\nexec switch_root\n";
    let config = InitrdNetworkConfig::new(
        "00:11:22:33:44:55",
        "192.0.2.20/24",
        "192.0.2.1",
        "2001:db8::20/64",
        "2001:db8::1",
        "2001:db8::21/64",
    )
    .unwrap();
    assert_eq!(
        inject_network_bootstrap(source, &[config]).unwrap(),
        concat!(
            "#!/bin/sh\n",
            "configure_ip() {\n",
            "        # BEGIN PERSONAL-LINUX initrd network worker\n",
            "        depmod\n",
            "        [ -d /sys/module/ipv6 ] || modprobe ipv6\n",
            "        /reinstall-worker initrd-network '00:11:22:33:44:55' '192.0.2.20/24' '192.0.2.1' '2001:db8::20/64' '2001:db8::1' '2001:db8::21/64'\n",
            "        MAC_ADDRESS=1\n",
            "        return\n",
            "        # END PERSONAL-LINUX initrd network worker\n",
            "    setup_dhcp\n",
            "}\n",
            "exec switch_root\n"
        )
    );
}

#[test]
fn alpine_init_network_hook_supports_multiple_nics_and_no_static_config() {
    let source = "configure_ip() {\n  upstream\n}\n";
    let configs = [
        InitrdNetworkConfig::new("00:11:22:33:44:55", "192.0.2.20/24", "", "", "", "").unwrap(),
        InitrdNetworkConfig::new("00:11:22:33:44:66", "", "", "2001:db8::20/64", "", "").unwrap(),
    ];
    let patched = inject_network_bootstrap(source, &configs).unwrap();
    assert!(patched.contains(
        "/reinstall-worker initrd-network '00:11:22:33:44:55' '192.0.2.20/24' '' '' '' ''"
    ));
    assert!(patched.contains(
        "/reinstall-worker initrd-network '00:11:22:33:44:66' '' '' '2001:db8::20/64' '' ''"
    ));
    assert!(patched.contains("        MAC_ADDRESS=1\n        return\n"));
    assert_eq!(
        inject_network_bootstrap(source, &[])
            .unwrap()
            .matches("depmod")
            .count(),
        1
    );
}

#[test]
fn alpine_init_patch_fails_closed_if_anchor_is_missing_ambiguous_or_already_patched() {
    assert!(matches!(
        inject_network_bootstrap("echo no network function\n", &[]),
        Err(InitrdError::ConfigureIpAnchorNotUnique)
    ));
    assert!(matches!(
        inject_network_bootstrap("configure_ip() {\n}\nconfigure_ip() {\n}\n", &[]),
        Err(InitrdError::ConfigureIpAnchorNotUnique)
    ));
    let patched = inject_network_bootstrap("configure_ip() {\n}\n", &[]).unwrap();
    assert!(matches!(
        inject_network_bootstrap(&patched, &[]),
        Err(InitrdError::AlreadyPatched)
    ));
}
