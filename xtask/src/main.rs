use rustaller_xtask::{bundle, check, dist, sources, vm};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}
fn run(args: Vec<String>) -> Result<(), String> {
    if args.first().is_some_and(|arg| arg == "local-tool") {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_owned();
        return rustaller_xtask::local_tool::run(&root, &args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "vm-acceptance") {
        return vm::run(&args[1..]);
    }
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage: cargo xtask <COMMAND>\n\n  bundle [--no-tui] [--target x86_64-unknown-linux-musl]\n  dist --tag vVERSION[-preview] [--output dist] [--no-tui] [--allow-dirty]\n  check [--skip-shellcheck]\n  audit-sources\n  local-tool [build|check]\n\nBundle defaults to a TUI host and a worker with no TUI dependencies.\nDist makes a development-preview package; it never publishes to GitHub.\nAudit-sources checks official metadata and endpoint reachability without downloading images."
        );
        return Ok(());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let mut target = bundle::TARGET.to_owned();
    let mut tui = true;
    let mut output = root.join("dist");
    let mut tag = None;
    let mut allow_dirty = false;
    let mut skip_shellcheck = false;
    let action = args[0].as_str();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--no-tui" if action == "bundle" || action == "dist" => tui = false,
            "--allow-dirty" if action == "dist" => allow_dirty = true,
            "--skip-shellcheck" if action == "check" => skip_shellcheck = true,
            flag @ ("--target" | "--output" | "--tag") if index + 1 < args.len() => {
                index += 1;
                match flag {
                    "--target" if action == "bundle" => target = args[index].clone(),
                    "--output" if action == "dist" => output = PathBuf::from(&args[index]),
                    "--tag" if action == "dist" => tag = Some(args[index].clone()),
                    _ => return Err(format!("option {flag} is not valid for {action}")),
                }
            }
            flag => return Err(format!("unknown or incomplete option: {flag}")),
        }
        index += 1;
    }
    match action {
        "bundle" => {
            bundle::build(&root, &target, tui)?;
        }
        "check" => check::run(&root, skip_shellcheck)?,
        "audit-sources" => sources::audit()?,
        "dist" => {
            let tag = tag.ok_or("dist requires --tag")?;
            dist::validate_tag(&tag)?;
            if output.exists() {
                return Err("output already exists; choose a fresh --output directory".into());
            }
            let source = dist::source_revision(&root, allow_dirty)?;
            let artifacts = bundle::build(&root, bundle::TARGET, tui)?;
            dist::package(&root, &artifacts, &output, &tag, &source)?;
            println!("Preview package: {}", output.display());
        }
        _ => return Err(format!("unknown command: {action}")),
    }
    Ok(())
}
