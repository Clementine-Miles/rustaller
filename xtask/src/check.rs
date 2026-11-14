use std::env;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

pub fn run(root: &Path) -> Result<(), String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    for args in [
        vec!["fmt", "--all", "--", "--check"],
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
        vec!["test", "--locked", "--workspace", "--all-features"],
    ] {
        run_command(root, &cargo, args)?;
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
