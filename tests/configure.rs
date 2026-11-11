use rustaller::model::{InstallPlan, Target};
use rustaller::postinstall::PostInstallConfig;
use rustaller::shared::configure::{ConfigurationError, apply_basic_configuration};
use rustaller::worker::live::{ValidatedLivePlan, validate_live_plan_with_uid};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustaller-configure-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("etc")).unwrap();
        Self(path)
    }

    fn zonefile(&self, name: &str, bytes: &[u8]) {
        let path = self.0.join("usr/share/zoneinfo").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn utc_zonefile() -> Vec<u8> {
    // A minimal TZif v1 UTC record: one ttinfo and a four-byte UTC name.
    let mut bytes = vec![0; 44];
    bytes[..4].copy_from_slice(b"TZif");
    bytes[36..40].copy_from_slice(&1_u32.to_be_bytes());
    bytes[40..44].copy_from_slice(&4_u32.to_be_bytes());
    bytes.extend_from_slice(&[0; 6]);
    bytes.extend_from_slice(b"UTC\0");
    bytes
}

#[test]
fn installed_iana_zonefiles_with_real_transition_tables_are_accepted() {
    for (name, bytes) in [
        (
            "Etc/UTC",
            include_bytes!("fixtures/zoneinfo/UTC").as_slice(),
        ),
        (
            "America/New_York",
            include_bytes!("fixtures/zoneinfo/New_York").as_slice(),
        ),
    ] {
        let scratch = Scratch::new();
        scratch.zonefile(name, bytes);
        let configuration = PostInstallConfig {
            timezone: Some(name.into()),
            ..Default::default()
        };
        let plan = live_plan(Target::Debian, configuration, None);
        apply_basic_configuration(&scratch.0, &plan, true).unwrap();
        assert_eq!(fs::read(scratch.0.join("etc/localtime")).unwrap(), bytes);
    }
}

fn utc_zonefile_v2(version: u8) -> Vec<u8> {
    // UTC has no transitions, so its 32-bit and 64-bit data blocks are equal.
    let mut block = utc_zonefile();
    block[4] = version;
    let mut bytes = block.clone();
    bytes.extend_from_slice(&block);
    bytes.extend_from_slice(b"\nUTC0\n");
    bytes
}

fn live_plan(
    target: Target,
    configuration: PostInstallConfig,
    hold: Option<u8>,
) -> ValidatedLivePlan {
    let mut plan = InstallPlan::new(target);
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.post_install = configuration;
    plan.hold = hold;
    validate_live_plan_with_uid(plan, "x86_64", 2048, rustaller::RUNTIME_VERSION, 0).unwrap()
}

fn requested_configuration() -> PostInstallConfig {
    PostInstallConfig {
        hostname: Some("my-vps".into()),
        timezone: Some("UTC".into()),
        ..Default::default()
    }
}

#[test]
fn requested_settings_replace_localtime_symlink_idempotently_without_following_it() {
    for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
        let scratch = Scratch::new();
        let bytes = utc_zonefile();
        scratch.zonefile("Etc/UTC", &bytes);
        symlink("Etc/UTC", scratch.0.join("usr/share/zoneinfo/UTC")).unwrap();
        let outside = scratch.0.join("outside-localtime");
        fs::write(&outside, b"do not overwrite").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&outside, scratch.0.join("etc/localtime")).unwrap();
        fs::write(scratch.0.join("etc/hosts"), b"preserve hosts\n").unwrap();
        let plan = live_plan(target, requested_configuration(), None);

        for _ in 0..2 {
            apply_basic_configuration(&scratch.0, &plan, true).unwrap();
            assert_eq!(
                fs::read(scratch.0.join("etc/hostname")).unwrap(),
                b"my-vps\n"
            );
            assert_eq!(fs::read(scratch.0.join("etc/timezone")).unwrap(), b"UTC\n");
            assert_eq!(fs::read(scratch.0.join("etc/localtime")).unwrap(), bytes);
            for name in ["hostname", "timezone", "localtime"] {
                let metadata = fs::symlink_metadata(scratch.0.join("etc").join(name)).unwrap();
                assert!(metadata.is_file());
                assert_eq!(metadata.permissions().mode() & 0o777, 0o644);
            }
            assert_eq!(fs::read(&outside).unwrap(), b"do not overwrite");
            assert_eq!(
                fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::read(scratch.0.join("etc/hosts")).unwrap(),
                b"preserve hosts\n"
            );
            assert_eq!(fs::read_dir(scratch.0.join("etc")).unwrap().count(), 4);
        }
    }
}

#[test]
fn missing_malformed_and_oversized_zonefiles_prevent_hostname_publication() {
    for bytes in [
        None,
        Some(b"not a TZif file".to_vec()),
        Some(vec![b'x'; 1024 * 1024 + 1]),
    ] {
        let scratch = Scratch::new();
        fs::write(scratch.0.join("etc/hostname"), b"existing\n").unwrap();
        if let Some(bytes) = bytes {
            scratch.zonefile("UTC", &bytes);
        }
        let plan = live_plan(Target::Debian, requested_configuration(), None);
        assert!(apply_basic_configuration(&scratch.0, &plan, false).is_err());
        assert_eq!(
            fs::read(scratch.0.join("etc/hostname")).unwrap(),
            b"existing\n"
        );
        assert!(!scratch.0.join("etc/timezone").exists());
        assert!(!scratch.0.join("etc/localtime").exists());
    }
}

#[test]
fn tzif_counts_versions_and_data_blocks_are_validated_before_any_write() {
    let valid = utc_zonefile();
    let mut bad_version = valid.clone();
    bad_version[4] = b'9';
    let mut zero_types = valid.clone();
    zero_types[36..40].copy_from_slice(&0_u32.to_be_bytes());
    let mut excessive_types = valid.clone();
    excessive_types[36..40].copy_from_slice(&257_u32.to_be_bytes());
    let mut zero_characters = valid.clone();
    zero_characters[40..44].copy_from_slice(&0_u32.to_be_bytes());
    let mut bad_standard_count = valid.clone();
    bad_standard_count[24..28].copy_from_slice(&2_u32.to_be_bytes());
    let mut bad_utc_count = valid.clone();
    bad_utc_count[20..24].copy_from_slice(&2_u32.to_be_bytes());
    let mut excessive_transitions = valid.clone();
    excessive_transitions[32..36].copy_from_slice(&u32::MAX.to_be_bytes());
    let mut excessive_leaps = valid.clone();
    excessive_leaps[28..32].copy_from_slice(&u32::MAX.to_be_bytes());
    let mut bad_dst = valid.clone();
    bad_dst[48] = 2;
    let mut bad_abbreviation = valid.clone();
    bad_abbreviation[49] = 4;
    let mut unterminated_abbreviation = valid.clone();
    *unterminated_abbreviation.last_mut().unwrap() = b'X';
    let mut bad_transition_type = valid.clone();
    bad_transition_type[32..36].copy_from_slice(&1_u32.to_be_bytes());
    let _ = bad_transition_type.splice(44..44, [0, 0, 0, 0, 1]);
    let mut missing_second_header = valid.clone();
    missing_second_header[4] = b'2';
    let mut mismatching_second_version = utc_zonefile_v2(b'2');
    mismatching_second_version[valid.len() + 4] = b'3';
    let v2 = utc_zonefile_v2(b'2');
    for bytes in [
        valid[..44].to_vec(),
        bad_version,
        zero_types,
        excessive_types,
        zero_characters,
        bad_standard_count,
        bad_utc_count,
        excessive_transitions,
        excessive_leaps,
        bad_dst,
        bad_abbreviation,
        unterminated_abbreviation,
        bad_transition_type,
        missing_second_header,
        mismatching_second_version,
        v2[..valid.len() * 2 - 1].to_vec(),
        v2[..valid.len() * 2].to_vec(),
    ] {
        let scratch = Scratch::new();
        scratch.zonefile("UTC", &bytes);
        let plan = live_plan(Target::Debian, requested_configuration(), None);
        assert!(matches!(
            apply_basic_configuration(&scratch.0, &plan, false),
            Err(ConfigurationError::InvalidZonefile)
        ));
        assert_eq!(fs::read_dir(scratch.0.join("etc")).unwrap().count(), 0);
    }
}

#[test]
fn supported_tzif_versions_with_complete_data_can_be_applied() {
    for bytes in [
        utc_zonefile(),
        utc_zonefile_v2(b'2'),
        utc_zonefile_v2(b'3'),
        utc_zonefile_v2(b'4'),
    ] {
        let scratch = Scratch::new();
        scratch.zonefile("UTC", &bytes);
        let plan = live_plan(Target::Debian, requested_configuration(), None);
        apply_basic_configuration(&scratch.0, &plan, false).unwrap();
        assert_eq!(fs::read(scratch.0.join("etc/localtime")).unwrap(), bytes);
    }
}

#[test]
fn escaped_timezone_symlink_is_rejected_before_any_configuration_write() {
    let scratch = Scratch::new();
    fs::create_dir_all(scratch.0.join("usr/share/zoneinfo")).unwrap();
    let outside = scratch.0.join("outside-zone");
    fs::write(&outside, utc_zonefile()).unwrap();
    symlink(&outside, scratch.0.join("usr/share/zoneinfo/UTC")).unwrap();
    let plan = live_plan(Target::Debian, requested_configuration(), None);
    assert!(matches!(
        apply_basic_configuration(&scratch.0, &plan, false),
        Err(ConfigurationError::ZonefileOutsideTarget)
    ));
    assert_eq!(fs::read_dir(scratch.0.join("etc")).unwrap().count(), 0);
}

#[test]
fn root_and_etc_symlinks_and_root_ancestors_are_rejected() {
    let scratch = Scratch::new();
    let plan = live_plan(
        Target::Debian,
        PostInstallConfig {
            hostname: Some("my-vps".into()),
            ..Default::default()
        },
        None,
    );
    let real_root = scratch.0.join("real-root");
    fs::create_dir_all(real_root.join("etc")).unwrap();
    let linked_root = scratch.0.join("linked-root");
    symlink(&real_root, &linked_root).unwrap();
    assert!(matches!(
        apply_basic_configuration(&linked_root, &plan, false),
        Err(ConfigurationError::UnsafePath)
    ));
    fs::create_dir_all(real_root.join("child/etc")).unwrap();
    assert!(matches!(
        apply_basic_configuration(&linked_root.join("child"), &plan, false),
        Err(ConfigurationError::UnsafePath)
    ));
    fs::remove_dir(scratch.0.join("etc")).unwrap();
    symlink(real_root.join("etc"), scratch.0.join("etc")).unwrap();
    assert!(matches!(
        apply_basic_configuration(&scratch.0, &plan, false),
        Err(ConfigurationError::UnsafePath)
    ));
    assert!(!real_root.join("etc/hostname").exists());
}

#[test]
fn invalid_destination_is_detected_before_hostname_write() {
    let scratch = Scratch::new();
    scratch.zonefile("UTC", &utc_zonefile());
    fs::create_dir(scratch.0.join("etc/localtime")).unwrap();
    let plan = live_plan(Target::Debian, requested_configuration(), None);
    assert!(matches!(
        apply_basic_configuration(&scratch.0, &plan, false),
        Err(ConfigurationError::UnsafePath)
    ));
    assert!(!scratch.0.join("etc/hostname").exists());
    assert!(!scratch.0.join("etc/timezone").exists());
}

#[test]
fn host_root_relative_paths_and_rescue_are_rejected_and_defaults_write_nothing() {
    let plan = live_plan(Target::Alpine, PostInstallConfig::default(), None);
    for root in [
        Path::new("/"),
        Path::new("relative/root"),
        Path::new("/tmp/../etc"),
    ] {
        assert!(matches!(
            apply_basic_configuration(root, &plan, true),
            Err(ConfigurationError::InvalidRoot)
        ));
    }
    let scratch = Scratch::new();
    let rescue = live_plan(Target::Alpine, PostInstallConfig::default(), Some(1));
    assert!(matches!(
        apply_basic_configuration(&scratch.0, &rescue, true),
        Err(ConfigurationError::RescuePlan)
    ));
    apply_basic_configuration(&scratch.0, &plan, true).unwrap();
    assert_eq!(fs::read_dir(scratch.0.join("etc")).unwrap().count(), 0);
}
