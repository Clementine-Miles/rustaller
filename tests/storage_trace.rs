use rustaller::command::{CommandOutput, CommandSpec, MockRunner};
use rustaller::model::{BootMode, InstallPlan, Target};
use rustaller::shared::storage::mount_target;
use rustaller::shared::storage::{DiskLayout, PartitionEnvironment, create_partitions};
use rustaller::worker::live::validate_live_plan_with_uid;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "rustaller-storage-trace-{}-{nonce}",
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
fn definition(source: &str, name: &str) -> String {
    let start = source.find(&format!("\n{name}() {{")).unwrap() + 1;
    let rest = &source[start..];
    rest[..rest.find("\n}\n").unwrap() + 2].into()
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn trace_record(spec: &CommandSpec, target: Target) -> String {
    let args = spec
        .args
        .iter()
        .map(|s| format!("{:?}", s.to_str().unwrap()))
        .collect::<Vec<_>>()
        .join(",");
    let cwd = spec
        .cwd
        .as_deref()
        .expect("explicit Live working directory");
    let env = spec
        .env
        .iter()
        .map(|(key, value)| format!("{:?}:{:?}", key.to_str().unwrap(), value.to_str().unwrap()))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(spec.stdin, rustaller::command::CommandInput::Null);
    format!(
        "{{\"phase\":\"live-storage\",\"target\":\"{target}\",\"program\":{:?},\"argv\":[{args}],\"cwd\":{:?},\"env_delta\":{{{env}}},\"stdin_fixture\":\"\",\"expected_status\":0}}\n",
        spec.program.to_str().unwrap(),
        cwd.to_str().unwrap()
    )
}

#[test]
fn all_targets_partition_mount_and_refresh_match_two_isolated_shell_runs() {
    let scratch = Scratch::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = scratch.0.join("trace-mock");
    assert!(
        Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg("--edition=2024")
            .arg(root.join("tests/support/mock-command.rs"))
            .arg("-o")
            .arg(&binary)
            .status()
            .unwrap()
            .success()
    );
    for utility in [
        "apk",
        "wipefs",
        "parted",
        "mkfs.fat",
        "mkfs.ext4",
        "sleep",
        "sync",
        "partprobe",
        "partx",
        "rc-service",
        "mdev",
        "rm",
        "mkdir",
        "mount",
    ] {
        std::os::unix::fs::symlink(&binary, scratch.0.join(utility)).unwrap();
    }
    let source = fs::read_to_string(root.join("trans.sh")).unwrap();
    let definitions = [
        "create_part",
        "update_part",
        "xda",
        "is_ends_with_digit",
        "mount_part_basic_layout",
    ]
    .map(|name| definition(&source, name))
    .join("\n");
    let path = format!(
        "{}:{}",
        scratch.0.display(),
        std::env::var_os("PATH").unwrap().to_string_lossy()
    );
    for target in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo] {
        for (mode, large, has_partprobe, has_partx) in [
            (BootMode::Bios, false, false, false),
            (BootMode::Bios, true, true, false),
            (BootMode::Efi, false, true, true),
        ] {
            let mut plan = InstallPlan::new(target);
            plan.confhome = Some("https://assets.example/reinstall".into());
            let plan =
                validate_live_plan_with_uid(plan, "x86_64", 4096, rustaller::RUNTIME_VERSION, 0)
                    .unwrap();
            let layout = DiskLayout::new(
                &plan,
                "nvme0n1",
                mode,
                if large {
                    3 * 1024_u64.pow(4)
                } else {
                    1024_u64.pow(4)
                },
            )
            .unwrap();
            let os_root = scratch.0.join("os");
            let efi_root = os_root.join(if matches!(target, Target::Alpine | Target::Debian) {
                "boot/efi"
            } else {
                "efi"
            });
            let definitions = format!(
                r#"
set -e
{definitions}
info() {{ :; }}
apk() {{ command apk "$@"; }}
is_efi() {{ [ "$TRACE_EFI" = 1 ]; }}
is_xda_gt_2t() {{ [ "$TRACE_LARGE" = 1 ]; }}
is_have_cmd() {{
    case "$1" in
    partprobe) [ "$TRACE_PARTPROBE" = 1 ] ;;
    partx) [ "$TRACE_PARTX" = 1 ] ;;
    *) exit 98 ;;
    esac
}}
ensure_service_stopped() {{ command rc-service -q "$1" stop; }}
ensure_service_started() {{ command rc-service -q "$1" start; }}
retry() {{ shift; "$@"; }}
distro={distro}
xda=nvme0n1
create_part
mount_part_basic_layout {os_root} {efi_root}
"#,
                distro = target.to_string().to_ascii_lowercase(),
                os_root = quote(os_root.to_str().unwrap()),
                efi_root = quote(efi_root.to_str().unwrap())
            );
            let baseline = |name: &str| {
                let trace = scratch.0.join(name);
                // Even filesystem-only utilities are mocked. Never source the
                // entry point, or let an unmocked disk/mount tool execute.
                let output =
                    Command::new(std::env::var_os("TEST_BASH").unwrap_or_else(|| "bash".into()))
                        .args(["--noprofile", "--norc", "-c", &definitions])
                        .current_dir("/")
                        .env("PATH", &path)
                        .env("TRACE_EFI", if mode == BootMode::Efi { "1" } else { "0" })
                        .env("TRACE_LARGE", if large { "1" } else { "0" })
                        .env("TRACE_PARTPROBE", if has_partprobe { "1" } else { "0" })
                        .env("TRACE_PARTX", if has_partx { "1" } else { "0" })
                        .env("MOCK_TRACE_PATH", &trace)
                        .env("MOCK_TRACE_PHASE", "live-storage")
                        .env("MOCK_TRACE_TARGET", target.to_string())
                        .env_remove("MOCK_EXIT_CODE")
                        .env_remove("MOCK_TRACE_ENV_KEYS")
                        .output()
                        .unwrap();
                assert!(
                    output.status.success(),
                    "baseline: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let contents = fs::read_to_string(&trace).unwrap();
                fs::remove_file(&trace).unwrap();
                // Shell file operations are Rust std::fs operations in the port.
                contents
                    .lines()
                    .filter(|s| {
                        !s.contains("\"program\":\"rm\"") && !s.contains("\"program\":\"mkdir\"")
                    })
                    .map(|s| format!("{s}\n"))
                    .collect::<String>()
            };
            let first = baseline("first.jsonl");
            assert_eq!(
                first,
                baseline("second.jsonl"),
                "deterministic {target} {mode:?}"
            );
            let mut runner =
                MockRunner::new((0..200).map(|_| CommandOutput::new(Some(0), vec![], vec![])));
            create_partitions(
                &mut runner,
                &layout,
                &PartitionEnvironment {
                    has_partprobe,
                    has_partx,
                    device_links: None,
                },
            )
            .unwrap();
            mount_target(&mut runner, &layout, &os_root).unwrap();
            let actual = runner
                .calls()
                .iter()
                .map(|c| trace_record(c, target))
                .collect::<String>();
            assert_eq!(
                actual.lines().count(),
                first.lines().count(),
                "trace length for {target} {mode:?}"
            );
            // Intentional firmware-correctness deviations from retained Shell:
            // a 512 MiB FAT32 ESP, including on 4Kn, with explicit GPT ESP type.
            // Preserve the upstream trace unchanged and normalize only these
            // reviewed differences; live_storage.rs checks the new arguments.
            let corrected = if mode == BootMode::Efi {
                first
                    .replace("101MiB", "513MiB")
                    .replace("\"1\",\"boot\",\"on\"", "\"1\",\"esp\",\"on\"")
                    .replace(
                        "\"program\":\"mkfs.fat\",\"argv\":[",
                        "\"program\":\"mkfs.fat\",\"argv\":[\"-F\",\"32\",\"-s\",\"1\",",
                    )
            } else {
                first
            };
            // Rust now intentionally installs XFS; retain the Shell source and
            // normalize only the authorized filesystem/tool differences.
            let corrected = corrected.replace("e2fsprogs", "xfsprogs")
                .replace("ext4", "xfs")
                .replace("\"-F\",\"-O\",\"^64bit\"", "\"-f\",\"-K\",\"-m\",\"reflink=0,bigtime=0,inobtcount=0\",\"-i\",\"nrext64=0\"")
                .replace("\"program\":\"mkfs.xfs\",\"argv\":[\"-F\"", "\"program\":\"mkfs.xfs\",\"argv\":[\"-f\",\"-K\",\"-m\",\"reflink=0,bigtime=0,inobtcount=0\",\"-i\",\"nrext64=0\"");
            for (index, (actual, baseline)) in actual.lines().zip(corrected.lines()).enumerate() {
                assert_eq!(
                    actual, baseline,
                    "command {index} for {target} {mode:?} large={large}"
                );
            }
            assert!(!runner.calls().iter().any(|c| {
                ["swapon", "swapoff", "mkswap"]
                    .iter()
                    .any(|forbidden| c.program == *forbidden)
            }));
        }
    }
}
