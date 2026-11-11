use crate::{contains_bytes, verify_x86_64_elf};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const TARGET: &str = "x86_64-unknown-linux-musl";

pub struct BundleArtifacts {
    pub host: PathBuf,
    pub worker: PathBuf,
    pub tui: bool,
}

pub fn build(root: &Path, target: &str, tui: bool) -> Result<BundleArtifacts, String> {
    if target != TARGET {
        return Err(format!("only the release target {TARGET} is supported"));
    }

    let target_dir = target_dir(root);
    let manifest = root.join("Cargo.toml");
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));

    run_cargo_build(
        &cargo,
        root,
        &manifest,
        target,
        "rustaller-worker",
        None,
        false,
    )?;
    let worker_path = artifact_path(&target_dir, target, "rustaller-worker");
    let worker = fs::read(&worker_path)
        .map_err(|error| format!("cannot read worker {}: {error}", worker_path.display()))?;
    verify_x86_64_elf(&worker, true, rustaller::RUNTIME_VERSION)
        .map_err(|error| format!("worker validation failed: {error}"))?;

    run_cargo_build(
        &cargo,
        root,
        &manifest,
        target,
        "rustaller",
        Some(&worker_path),
        tui,
    )?;
    let host_path = artifact_path(&target_dir, target, "rustaller");
    let host = fs::read(&host_path)
        .map_err(|error| format!("cannot read host {}: {error}", host_path.display()))?;
    verify_x86_64_elf(&host, true, rustaller::RUNTIME_VERSION)
        .map_err(|error| format!("host validation failed: {error}"))?;
    if host.len() <= worker.len() || !contains_bytes(&host, &worker) {
        return Err("host does not contain the exact worker ELF that was built".into());
    }

    println!("Bundled matching static host and worker:");
    println!("  {}", host_path.display());
    println!("  worker bytes embedded and version-checked");
    Ok(BundleArtifacts {
        host: host_path,
        worker: worker_path,
        tui,
    })
}

fn target_dir(root: &Path) -> PathBuf {
    match env::var_os("CARGO_TARGET_DIR") {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        }
        None => root.join("target"),
    }
}

fn artifact_path(target_dir: &Path, target: &str, name: &str) -> PathBuf {
    target_dir.join(target).join("release").join(name)
}

fn run_cargo_build(
    cargo: &OsString,
    root: &Path,
    manifest: &Path,
    target: &str,
    binary: &str,
    worker: Option<&Path>,
    tui: bool,
) -> Result<(), String> {
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .arg("build")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--release")
        .arg("--no-default-features")
        .arg("--target")
        .arg(target)
        .arg("--bin")
        .arg(binary)
        .env_remove("REINSTALL_WORKER_ELF")
        .env_remove("REINSTALL_XTASK_BUNDLE");
    if tui {
        command.args(["--features", "tui"]);
    }
    if let Some(worker) = worker {
        command
            .env("REINSTALL_WORKER_ELF", worker)
            .env("REINSTALL_XTASK_BUNDLE", "1");
    }
    let status = command
        .status()
        .map_err(|error| format!("cannot start Cargo for {binary}: {error}"))?;
    if !status.success() {
        return Err(format!(
            "Cargo build for {binary} failed with status {:?}",
            status.code()
        ));
    }
    Ok(())
}
