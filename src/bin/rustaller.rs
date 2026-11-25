use rustaller::command::SystemRunner;
use rustaller::host::args::{CliOutcome, parse_args};
use rustaller::host::handoff;
use rustaller::model::InstallPlan;
use std::io::{self, BufRead, IsTerminal, Read, Write};

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            #[cfg(feature = "tui")]
            return tui::run(preflight_plan, run_plan);
            #[cfg(not(feature = "tui"))]
            return Err("interactive TUI is not available in this development build".into());
        }
        println!("{}", rustaller::host::args::HELP);
        return Err("a target is required".into());
    }

    let outcome = parse_args(&args).map_err(|error| error.to_string())?;
    match outcome {
        CliOutcome::Help => {
            println!("{}", rustaller::host::args::HELP);
            Ok(())
        }
        CliOutcome::Reset => {
            println!(
                "{}",
                rustaller::host::lifecycle::reset().map_err(|error| error.to_string())?
            );
            Ok(())
        }
        CliOutcome::HostInspection => {
            println!(
                "{}",
                rustaller::host::inspect::collect(&mut SystemRunner, std::path::Path::new("/"))
                    .render_current()
            );
            Ok(())
        }
        CliOutcome::Plan(plan) => run_plan(&plan),
        CliOutcome::Inspect(plan) => {
            let (report, confirmation) = inspect_plan(&plan);
            println!("{}", report.text());
            if let Some(confirmation) = confirmation {
                println!("Confirmation token: {confirmation}");
            }
            Ok(())
        }
        CliOutcome::ConfirmedPlan { plan, confirmation } => run_install(&plan, Some(&confirmation)),
    }
}

#[cfg(feature = "tui")]
fn preflight_plan(plan: &InstallPlan) -> Result<rustaller::host::inspect::PreflightReport, String> {
    Ok(inspect_plan(plan).0)
}

fn inspect_plan(plan: &InstallPlan) -> (rustaller::host::inspect::PreflightReport, Option<String>) {
    let mut runner = SystemRunner;
    let root = std::path::Path::new("/");
    let (mut report, confirmation) = match handoff::inspect_inventory(&mut runner, plan) {
        Ok(manifest) => {
            let confirmation = handoff::confirmation_for_plan(plan, &manifest.disk);
            let mut report = rustaller::host::inspect::report(&mut runner, root, plan, &manifest);
            report.warning.push_str("\nOffline inventory checked. Source authentication, kexec readiness and package availability are checked during installation preparation.");
            (report, Some(confirmation))
        }
        Err(error) => {
            let current = rustaller::host::inspect::collect(&mut runner, root).render_current();
            let target = rustaller::host::inspect::planned_target(plan, None);
            (
                rustaller::host::inspect::PreflightReport {
                    current,
                    target,
                    warning: format!(
                        "Installation compatibility: blocked — {error}\nDiagnostic facts do not authorize installation."
                    ),
                    blocked: true,
                },
                None,
            )
        }
    };
    if let Ok(status) = rustaller::host::lifecycle::status() {
        report.warning.push('\n');
        report.warning.push_str(status);
    }
    (report, confirmation)
}
fn run_plan(plan: &InstallPlan) -> Result<(), String> {
    run_install(plan, None)
}

fn run_install(plan: &InstallPlan, provided_confirmation: Option<&str>) -> Result<(), String> {
    if !plan.target.is_automatic() {
        return Err("Select an installation or Alpine Live rescue route.".into());
    }
    plan.validate_install_credentials()
        .map_err(|error| error.to_string())?;
    if rustaller::worker_bytes().is_empty() {
        return Err("matching rustaller-worker is not embedded".into());
    }
    let mut owned_plan = plan.clone();
    let mut runner = SystemRunner;
    owned_plan.ssh_keys = handoff::resolve_public_keys(&mut runner, &owned_plan.ssh_keys)
        .map_err(|error| error.to_string())?;
    rustaller::host::hostname::resolve_install_hostname(&mut owned_plan, std::path::Path::new("/"));
    let plan = &owned_plan;
    let deployment = handoff::discover(&mut runner, plan).map_err(|error| error.to_string())?;
    println!(
        "{}",
        rustaller::host::inspect::report(
            &mut runner,
            std::path::Path::new("/"),
            plan,
            &deployment.manifest
        )
        .text()
    );
    println!("Target system: {}", plan.target);
    println!(
        "Target disk: {} ({} bytes)",
        deployment.manifest.disk.device, deployment.manifest.disk.size_bytes
    );
    println!("Firmware: {}", deployment.manifest.boot_mode.as_str());
    println!("Confirmation token: {}", deployment.confirmation);
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
    println!("Preparing authenticated Alpine Live and the private installation plan...");
    let prepared = handoff::prepare(
        &mut runner,
        plan,
        &deployment,
        &confirmation,
        rustaller::worker_bytes(),
    )
    .map_err(|error| error.to_string())?;
    if plan.hold == Some(0) {
        println!("Prepared files: {}", prepared.workspace().display());
        if !io::stdin().is_terminal() {
            println!(
                "Handoff paused by --hold 0. Use rustaller reset to cancel or rerun without --hold 0 to prepare a fresh handoff."
            );
            return Ok(());
        }
        println!(
            "Preparation is held for review. Type KEXEC to switch now or use rustaller reset to discard the staging files."
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
