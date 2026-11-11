use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=REINSTALL_WORKER_ELF");
    println!("cargo:rerun-if-env-changed=REINSTALL_XTASK_BUNDLE");
    println!("cargo:rerun-if-changed=tests/fixtures/embedded-worker-version.txt");
    println!("cargo:rustc-check-cfg=cfg(has_embedded_worker)");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let profile = env::var("PROFILE").unwrap_or_default();
    let bundle_requested = env::var("REINSTALL_XTASK_BUNDLE").as_deref() == Ok("1");
    let worker = match env::var_os("REINSTALL_WORKER_ELF") {
        Some(path) if profile == "release" && bundle_requested => {
            let path = PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            fs::read(&path).unwrap_or_else(|error| {
                panic!("cannot read bundled worker {}: {error}", path.display())
            })
        }
        _ if profile != "release" => make_test_worker(),
        _ => Vec::new(),
    };

    if !worker.is_empty() {
        assert!(
            is_x86_64_elf(&worker),
            "embedded worker must be an x86_64 ELF"
        );
        let version = fs::read_to_string("tests/fixtures/embedded-worker-version.txt")
            .expect("read embedded worker version fixture");
        assert!(
            contains_bytes(&worker, version.trim().as_bytes()),
            "embedded worker does not contain the current runtime version"
        );
        if profile == "release" {
            println!("cargo:rustc-cfg=has_embedded_worker");
        }
    }

    let worker_path = out_dir.join("embedded-worker.bin");
    fs::write(&worker_path, &worker).expect("write embedded worker artifact");
    let source = format!(
        "pub static EMBEDDED_WORKER: &[u8] = include_bytes!({:?});\n",
        worker_path
    );
    fs::write(out_dir.join("embedded_worker.rs"), source).expect("write generated worker include");
}

fn make_test_worker() -> Vec<u8> {
    let version = fs::read_to_string("tests/fixtures/embedded-worker-version.txt")
        .expect("read embedded worker version fixture");
    let mut bytes = vec![0; 64];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[18..20].copy_from_slice(&[0x3e, 0]);
    bytes.extend_from_slice(version.trim().as_bytes());
    bytes
}

fn is_x86_64_elf(bytes: &[u8]) -> bool {
    bytes.len() >= 20
        && &bytes[..4] == b"\x7fELF"
        && bytes[4] == 2
        && bytes[5] == 1
        && bytes[18..20] == [0x3e, 0]
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}
