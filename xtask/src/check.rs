use std::env;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

pub fn run(root: &Path, skip_shellcheck: bool) -> Result<(), String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    run_command(
        root,
        "git",
        [
            "cat-file",
            "-e",
            "98f1e21013233d42750cc2e7a298a0818461d686^{commit}",
        ],
    )?;
    for args in [
        vec!["fmt", "--all", "--", "--check"],
        vec![
            "fmt",
            "--manifest-path",
            "tests/verify/Cargo.toml",
            "--",
            "--check",
        ],
        vec![
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
        vec![
            "clippy",
            "--locked",
            "--manifest-path",
            "tests/verify/Cargo.toml",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        vec!["test", "--locked", "--workspace", "--all-features"],
        vec![
            "test",
            "--locked",
            "--manifest-path",
            "tests/verify/Cargo.toml",
        ],
    ] {
        run_command(root, &cargo, args)?;
    }
    let bash = env::var_os("TEST_BASH").unwrap_or_else(|| "bash".into());
    for script in [
        "reinstall.sh",
        "trans.sh",
        "initrd-network.sh",
        "fix-eth-name.sh",
    ] {
        run_command(root, &bash, ["-n", script])?;
    }
    let busybox = env::var_os("TEST_BUSYBOX").unwrap_or_else(|| "busybox".into());
    for script in ["trans.sh", "initrd-network.sh", "fix-eth-name.sh"] {
        run_command(root, &busybox, ["ash", "-n", script])?;
    }
    if !skip_shellcheck {
        let shellcheck = env::var_os("TEST_SHELLCHECK").unwrap_or_else(|| "shellcheck".into());
        run_command(
            root,
            &shellcheck,
            [
                "--severity=warning",
                "reinstall.sh",
                "trans.sh",
                "initrd-network.sh",
                "fix-eth-name.sh",
            ],
        )?;
        run_command(
            root,
            &shellcheck,
            [
                "--severity=warning",
                "--shell=sh",
                "tests/fixtures/fix-eth-name-openrc.initd",
            ],
        )?;
    }
    run_command(root, "git", ["diff", "--check"])
}

fn run_command<I, S>(root: &Path, program: impl AsRef<OsStr>, args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.current_dir(root).args(args);
    println!("Checking {command:?}");
    let status = command.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "check failed: {command:?} (status {:?})",
            status.code()
        ))
    }
}
