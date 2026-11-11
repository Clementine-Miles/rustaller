use rustaller::command::SystemRunner;
use rustaller::host::args::{CliOutcome, parse_args};
#[cfg(feature = "tui")]
use rustaller::host::current::{CurrentConfig, CurrentReport, apply_current};
use rustaller::host::handoff;
use rustaller::model::InstallPlan;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "tui")]
#[path = "../tui/mod.rs"]
mod tui;

#[cfg(all(not(debug_assertions), not(has_embedded_worker)))]
compile_error!("release host builds must use `cargo xtask bundle` to embed rustaller-worker");

fn main() {
    if let Err(error) = run() {
        eprintln!("rustaller: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    if rustaller::worker_bytes().is_empty() {
        return Err("matching rustaller-worker is not embedded".into());
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let confhome = std::env::var("REINSTALL_CONFHOME").ok();
    if args.is_empty() {
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            #[cfg(feature = "tui")]
            return tui::run(confhome, run_plan, configure_current);
            #[cfg(not(feature = "tui"))]
            return Err("interactive TUI is not available in this development build".into());
        }
        println!("{}", rustaller::host::args::HELP);
        return Err("a target is required".into());
    }

    let outcome = parse_args(&args, confhome.as_deref()).map_err(|error| error.to_string())?;
    match outcome {
        CliOutcome::Help => {
            println!("{}", rustaller::host::args::HELP);
            Ok(())
        }
        CliOutcome::Plan(plan) => run_plan(&plan),
        CliOutcome::Inspect(plan) => run_install(&plan, None, true),
        CliOutcome::ConfirmedPlan { plan, confirmation } => {
            run_install(&plan, Some(&confirmation), false)
        }
    }
}

fn run_plan(plan: &InstallPlan) -> Result<(), String> {
    run_install(plan, None, false)
}

#[cfg(feature = "tui")]
fn configure_current(config: &CurrentConfig) -> Result<CurrentReport, String> {
    apply_current(&mut SystemRunner, config).map_err(|error| error.to_string())
}

fn run_install(
    plan: &InstallPlan,
    provided_confirmation: Option<&str>,
    inspect: bool,
) -> Result<(), String> {
    if !plan.target.is_automatic() {
        return Err("This Rust handoff entry supports Alpine, Debian, Arch, Gentoo and Alpine Live rescue. Interactive media handoff is not connected.".into());
    }
    let mut runner = SystemRunner;
    let deployment = handoff::discover(&mut runner, plan).map_err(|error| error.to_string())?;
    println!("Target system: {}", plan.target);
    println!(
        "Target disk: {} ({} bytes)",
        deployment.manifest.disk.device, deployment.manifest.disk.size_bytes
    );
    println!("Firmware: {}", deployment.manifest.boot_mode.as_str());
    println!("Confirmation token: {}", deployment.confirmation);
    if inspect {
        return Ok(());
    }
    if plan.password.is_none() && plan.ssh_keys.is_empty() {
        return Err("Configure an SSH public key or password before installation. Use the TUI to enter and confirm a password without command-line history.".into());
    }
    let confirmation = match provided_confirmation {
        Some(value) => value.to_owned(),
        None => {
            if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
                return Err("Non-interactive installation requires --confirm-erase with the exact token from --inspect".into());
            }
            if plan.hold == Some(1) {
                println!(
                    "Switching to Alpine Live rescue interrupts this session but does not automatically partition a disk. Type the complete confirmation token to continue; any other input cancels."
                );
            } else {
                println!(
                    "Installation erases every partition on the disk shown above. Type the complete confirmation token to continue; any other input cancels."
                );
            }
            print!("> ");
            io::stdout().flush().map_err(|error| error.to_string())?;
            read_confirmation(&mut io::stdin().lock()).map_err(|error| error.to_string())?
        }
    };
    if confirmation != deployment.confirmation {
        return Err("Installation cancelled: disk identity confirmation did not match".into());
    }
    let workspace = PathBuf::from(format!(
        "/var/tmp/rustaller-{}-{}",
        std::process::id(),
        NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed)
    ));
    println!("Preparing authenticated Alpine Live and the private installation plan...");
    let prepared = handoff::prepare(
        &mut runner,
        plan,
        &deployment,
        &confirmation,
        &workspace,
        rustaller::worker_bytes(),
    )
    .map_err(|error| error.to_string())?;
    if plan.hold == Some(0) {
        println!("Prepared files: {}", prepared.workspace().display());
        if !io::stdin().is_terminal() {
            println!(
                "Handoff paused by --hold 0; no kernel was loaded. Rerun without --hold 0 to perform the handoff after review."
            );
            return Ok(());
        }
        println!(
            "Preparation is held for review. Type KEXEC to switch now; any other input leaves the source system running."
        );
        print!("> ");
        io::stdout().flush().map_err(|error| error.to_string())?;
        if read_confirmation(&mut io::stdin().lock()).map_err(|error| error.to_string())? != "KEXEC"
        {
            return Ok(());
        }
    }
    if plan.hold == Some(1) {
        println!("Preparation complete. Switching through kexec into Alpine Live rescue.");
    } else {
        println!(
            "Preparation complete. Switching through kexec; the next full reboot enters the installed system."
        );
    }
    handoff::execute(&mut runner, &prepared).map_err(|error| error.to_string())
}

fn read_confirmation(reader: &mut impl BufRead) -> io::Result<String> {
    let mut line = String::new();
    reader.take(4098).read_line(&mut line)?;
    let value = line.trim_end_matches(['\n', '\r']);
    if value.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Confirmation is too long; installation cancelled",
        ));
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod confirmation_tests {
    #[test]
    fn confirmation_input_is_bounded_and_preserves_identity_spaces() {
        let mut reader = std::io::Cursor::new(b"ERASE /dev/vda SERIAL=VM 123\r\n");
        assert_eq!(
            super::read_confirmation(&mut reader).unwrap(),
            "ERASE /dev/vda SERIAL=VM 123"
        );
        let mut reader = std::io::Cursor::new(vec![b'a'; 8192]);
        assert!(super::read_confirmation(&mut reader).is_err());
    }
}
