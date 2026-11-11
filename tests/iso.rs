use rustaller::command::{CommandOutput, MockRunner};
use rustaller::host::iso::{
    IsoError, canonicalize_iso_source, prepare_local_iso, render_grub_entry, validate_iso_options,
};
use rustaller::model::IsoOptions;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "rustaller-iso-{label}-{}-{nonce}",
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

fn test_mount_dir(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let managed = scratch.0.join("rustaller-tmp");
    let check = managed.join("iso-check");
    fs::create_dir_all(&check).unwrap();
    (managed, check)
}

fn options(path: &str) -> IsoOptions {
    IsoOptions {
        path: path.to_owned(),
        config: None,
        kernel: None,
        initrd: None,
        cmdline: None,
    }
}

#[test]
fn iso_options_default_to_loopback_cfg_and_validate_custom_kernel_pair() {
    let default = validate_iso_options(&options("/images/live.iso")).unwrap();
    assert_eq!(default.config.as_deref(), Some("/boot/grub/loopback.cfg"));
    assert!(default.kernel.is_none());

    let custom = IsoOptions {
        path: "/images/live.iso".into(),
        config: None,
        kernel: Some("/live/vmlinuz".into()),
        initrd: Some("/live/initrd.img".into()),
        cmdline: Some("boot=live findiso=%ISO_PATH%".into()),
    };
    let custom = validate_iso_options(&custom).unwrap();
    assert_eq!(custom.kernel.as_deref(), Some("/live/vmlinuz"));
    assert_eq!(custom.initrd.as_deref(), Some("/live/initrd.img"));
}

#[test]
fn iso_options_reject_path_injection_incomplete_pairs_and_conflicts() {
    for path in [
        "relative.iso",
        "/images/../live.iso",
        "/images/a b.iso",
        "/images/a;reboot.iso",
    ] {
        assert!(validate_iso_options(&options(path)).is_err(), "{path}");
    }
    assert!(
        validate_iso_options(&IsoOptions {
            path: "/images/live.iso".into(),
            config: None,
            kernel: Some("/live/vmlinuz".into()),
            initrd: None,
            cmdline: None,
        })
        .is_err()
    );
    assert!(
        validate_iso_options(&IsoOptions {
            path: "/images/live.iso".into(),
            config: Some("/boot/grub/loopback.cfg".into()),
            kernel: Some("/live/vmlinuz".into()),
            initrd: Some("/live/initrd.img".into()),
            cmdline: None,
        })
        .is_err()
    );
    assert!(
        validate_iso_options(&IsoOptions {
            path: "/images/live.iso".into(),
            config: None,
            kernel: Some("/live/vmlinuz".into()),
            initrd: Some("/live/initrd.img".into()),
            cmdline: Some("boot=live; reboot".into()),
        })
        .is_err()
    );
}

#[test]
fn canonical_iso_source_must_be_readable_nonempty_and_outside_managed_paths() {
    let scratch = Scratch::new("source");
    let image = scratch.0.join("live.iso");
    fs::write(&image, b"not really an ISO, but a nonempty local source").unwrap();
    let resolved = canonicalize_iso_source(&image, &scratch.0.join("rustaller-tmp")).unwrap();
    assert_eq!(resolved, fs::canonicalize(image).unwrap());

    let empty = scratch.0.join("empty.iso");
    fs::write(&empty, []).unwrap();
    assert!(canonicalize_iso_source(&empty, &scratch.0.join("rustaller-tmp")).is_err());

    let managed = scratch.0.join("rustaller-tmp");
    fs::create_dir(&managed).unwrap();
    let protected = managed.join("live.iso");
    fs::write(&protected, b"iso").unwrap();
    assert!(canonicalize_iso_source(&protected, &managed).is_err());
}

#[test]
fn read_only_iso_preflight_preserves_probe_and_mount_call_order() {
    let scratch = Scratch::new("preflight");
    let image = scratch.0.join("live.iso");
    fs::write(&image, b"iso bytes").unwrap();
    let (managed_tmp, check_dir) = test_mount_dir(&scratch);
    fs::create_dir_all(check_dir.join("boot/grub")).unwrap();
    fs::write(
        check_dir.join("boot/grub/loopback.cfg"),
        b"menuentry test\n",
    )
    .unwrap();

    let mut runner = MockRunner::new([
        CommandOutput::new(Some(0), b"ext2\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), b"1111-2222\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), b"/images/live.iso\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
    ]);
    let prepared = prepare_local_iso(
        &mut runner,
        &options(&image.to_string_lossy()),
        &managed_tmp,
        &check_dir,
    )
    .unwrap();

    assert_eq!(prepared.uuid(), "1111-2222");
    assert_eq!(prepared.grub_path(), "/images/live.iso");
    assert_eq!(runner.calls().len(), 5);
    assert_eq!(runner.calls()[0].program, "grub-probe");
    assert_eq!(runner.calls()[0].args[0], "--target=abstraction");
    assert_eq!(runner.calls()[1].args[0], "--target=fs_uuid");
    assert_eq!(runner.calls()[2].program, "grub-mkrelpath");
    assert_eq!(runner.calls()[3].program, "mount");
    assert_eq!(
        runner.calls()[3].args,
        [
            "-o",
            "loop,ro",
            image.to_str().unwrap(),
            check_dir.to_str().unwrap()
        ]
    );
    assert_eq!(runner.calls()[4].program, "umount");
}

#[test]
fn generated_loopback_config_contains_only_validated_iso_parameters() {
    let scratch = Scratch::new("render");
    let image = scratch.0.join("live.iso");
    fs::write(&image, b"iso").unwrap();
    let (managed_tmp, check_dir) = test_mount_dir(&scratch);
    fs::create_dir_all(check_dir.join("boot/grub")).unwrap();
    fs::write(check_dir.join("boot/grub/loopback.cfg"), b"cfg").unwrap();
    let mut runner = MockRunner::new([
        CommandOutput::new(Some(0), b"ext2\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), b"1111-2222\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), b"/images/live.iso\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
    ]);
    let prepared = prepare_local_iso(
        &mut runner,
        &options(&image.to_string_lossy()),
        &managed_tmp,
        &check_dir,
    )
    .unwrap();
    let output = render_grub_entry(&prepared);
    assert!(
        output.starts_with("### BEGIN reinstall.sh ###\nset timeout_style=menu\nset timeout=5\n")
    );
    assert!(output.contains("search --no-floppy --fs-uuid --set=iso_device '1111-2222'\n"));
    assert!(output.contains("loopback loop ($iso_device)$iso_path\n"));
    assert!(output.contains("configfile (loop)/boot/grub/loopback.cfg\n"));
    assert!(output.ends_with("### END reinstall.sh ###\n"));
}

#[test]
fn read_only_preflight_rejects_encryption_and_windows_media_without_skipping_unmount() {
    let encrypted = Scratch::new("encrypted");
    let encrypted_image = encrypted.0.join("live.iso");
    fs::write(&encrypted_image, b"iso").unwrap();
    let mut encrypted_runner = MockRunner::new([CommandOutput::new(
        Some(0),
        b"partmap cryptodisk\n".to_vec(),
        Vec::new(),
    )]);
    assert!(matches!(
        prepare_local_iso(
            &mut encrypted_runner,
            &options(&encrypted_image.to_string_lossy()),
            &encrypted.0.join("rustaller-tmp"),
            &encrypted.0.join("rustaller-tmp/iso-check"),
        ),
        Err(IsoError::EncryptedStorage)
    ));
    assert_eq!(encrypted_runner.calls().len(), 1);

    let windows = Scratch::new("windows-media");
    let windows_image = windows.0.join("live.iso");
    fs::write(&windows_image, b"iso").unwrap();
    let (managed_tmp, check_dir) = test_mount_dir(&windows);
    fs::create_dir_all(check_dir.join("SOURCES")).unwrap();
    fs::write(check_dir.join("SOURCES/BOOT.WIM"), b"windows installer").unwrap();
    let mut windows_runner = MockRunner::new([
        CommandOutput::new(Some(0), b"ext2\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), b"1111-2222\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), b"/images/live.iso\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
    ]);
    assert!(matches!(
        prepare_local_iso(
            &mut windows_runner,
            &options(&windows_image.to_string_lossy()),
            &managed_tmp,
            &check_dir,
        ),
        Err(IsoError::InvalidFilesystemLayout)
    ));
    assert_eq!(windows_runner.calls().last().unwrap().program, "umount");
}

#[test]
fn custom_kernel_parameters_substitute_only_the_supported_iso_tokens() {
    let scratch = Scratch::new("custom");
    let image = scratch.0.join("live.iso");
    fs::write(&image, b"iso").unwrap();
    let (managed_tmp, check_dir) = test_mount_dir(&scratch);
    fs::create_dir_all(check_dir.join("live")).unwrap();
    fs::write(check_dir.join("live/vmlinuz"), b"kernel").unwrap();
    fs::write(check_dir.join("live/initrd.img"), b"initrd").unwrap();
    let config = IsoOptions {
        path: image.to_string_lossy().into_owned(),
        config: None,
        kernel: Some("/live/vmlinuz".into()),
        initrd: Some("/live/initrd.img".into()),
        cmdline: Some("findiso=%ISO_PATH% uuid=%ISO_UUID%".into()),
    };
    let mut runner = MockRunner::new([
        CommandOutput::new(Some(0), b"ext2\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), b"1111-2222\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), b"/images/live.iso\n".to_vec(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
        CommandOutput::new(Some(0), Vec::new(), Vec::new()),
    ]);
    let prepared = prepare_local_iso(&mut runner, &config, &managed_tmp, &check_dir).unwrap();
    let output = render_grub_entry(&prepared);
    assert!(output.contains("linux (loop)/live/vmlinuz findiso=/images/live.iso uuid=1111-2222\n"));
    assert!(output.contains("initrd (loop)/live/initrd.img\n"));
    assert!(!output.contains("%ISO_"));
}
