use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "rustaller-command-trace-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create unique trace directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn compile_mock(scratch: &Path) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/mock-command.rs");
    let binary = scratch.join("trace-mock");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = Command::new(rustc)
        .arg("--edition=2024")
        .arg(source)
        .arg("-o")
        .arg(&binary)
        .status()
        .expect("compile Rust trace mock");
    assert!(status.success(), "mock compilation failed: {status}");
    binary
}

fn extract_function(source: &str, signature: &str) -> String {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("missing function {signature}"));
    let function = &source[start..];
    let end = function
        .find("\n}\n")
        .unwrap_or_else(|| panic!("unterminated function {signature}"))
        + 2;
    function[..end].to_owned()
}

#[test]
fn rust_mock_emits_deterministic_json_command_trace_without_changing_process_output() {
    let scratch = Scratch::new();
    let binary = compile_mock(&scratch.0);
    let trace = scratch.0.join("trace.jsonl");
    let mut child = Command::new(binary)
        .arg("first")
        .arg("argument \"quoted\"")
        .current_dir(&scratch.0)
        .env("MOCK_EXIT_CODE", "7")
        .env("MOCK_TRACE_PATH", &trace)
        .env("MOCK_TRACE_PHASE", "debian-bootstrap")
        .env("MOCK_TRACE_TARGET", "debian")
        .env("MOCK_TRACE_ENV_KEYS", "TRACE_TEST")
        .env("TRACE_TEST", "controlled")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run Rust trace mock");
    child
        .stdin
        .take()
        .expect("mock stdin")
        .write_all(b"input line\n")
        .expect("write mock stdin");
    let output = child.wait_with_output().expect("wait for mock");

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"out:first:input line\n\n");
    assert_eq!(output.stderr, b"diagnostic\n");
    let trace = fs::read_to_string(trace).expect("read JSON trace");
    assert_eq!(trace.lines().count(), 1);
    let record = trace.trim_end();
    assert!(record.starts_with(
        "{\"phase\":\"debian-bootstrap\",\"target\":\"debian\",\"program\":\"trace-mock\","
    ));
    assert!(record.contains("\"argv\":[\"first\",\"argument \\\"quoted\\\"\"]"));
    assert!(record.contains(&format!("\"cwd\":\"{}\"", scratch.0.display())));
    assert!(record.contains("\"env_delta\":{\"TRACE_TEST\":\"controlled\"}"));
    assert!(record.contains("\"stdin_fixture\":\"input line\\n\""));
    assert!(record.ends_with("\"expected_status\":7}"));
}

#[test]
fn isolated_debootstrap_shell_baseline_matches_a_stable_json_trace_fixture() {
    let scratch = Scratch::new();
    let binary = compile_mock(&scratch.0);
    std::os::unix::fs::symlink(&binary, scratch.0.join("apk")).expect("link mocked apk");
    std::os::unix::fs::symlink(&binary, scratch.0.join("debootstrap"))
        .expect("link mocked debootstrap");

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("trans.sh")).unwrap();
    let function = extract_function(&source, "bootstrap_debian_rootfs() {");
    let fixture =
        fs::read_to_string(root.join("tests/fixtures/command-traces/debian-bootstrap.jsonl"))
            .expect("read baseline trace fixture");
    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let test_bash = std::env::var_os("TEST_BASH").unwrap_or_else(|| "bash".into());
    let run = |trace_name: &str| {
        let trace = scratch.0.join(trace_name);
        let path = format!(
            "{}:{}",
            scratch.0.display(),
            original_path.to_string_lossy()
        );
        let script =
            format!("set -e; {function}\ncodename=trixie; mirror=; bootstrap_debian_rootfs /os");
        let output = Command::new(&test_bash)
            .args(["--noprofile", "--norc", "-c", &script])
            .current_dir("/")
            .env("PATH", path)
            .env("MOCK_TRACE_PATH", &trace)
            .env("MOCK_TRACE_PHASE", "debian-bootstrap")
            .env("MOCK_TRACE_TARGET", "debian")
            .output()
            .expect("run isolated Shell function with mocked utilities");
        assert!(
            output.status.success(),
            "baseline failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::read_to_string(trace).unwrap()
    };
    let first = run("trace-first.jsonl");
    let second = run("trace-second.jsonl");
    assert_eq!(first, second, "baseline trace must be deterministic");
    assert_eq!(first, fixture);
}
