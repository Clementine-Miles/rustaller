use rustaller::command::{SwapSafeRunner, SystemRunner};
use rustaller::worker::args::{WorkerAction, parse_worker_args};
use rustaller::worker::firstboot::{cleanup_firstboot, run_firstboot};
use std::path::Path;

fn main() {
    if let Err(error) = run() {
        eprintln!("rustaller-worker: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let action = parse_worker_args(std::env::args().skip(1))?;
    let Some(action) = action else {
        println!(
            "Usage: rustaller-worker <live-install PLAN MANIFEST|reset|firstboot-fix|cachyos-firstboot|cleanup-openrc|initrd-network>\n\nRuntime version: {}\nInternal helper for the Rust rustaller installer.",
            rustaller::RUNTIME_VERSION
        );
        return Ok(());
    };

    let mut runner = SwapSafeRunner::new(SystemRunner);
    match action {
        WorkerAction::Reset => {
            println!("{}", rustaller::worker::install::cancel_live(&mut runner)?)
        }
        WorkerAction::LiveModules => rustaller::worker::install::prepare_live_modules(&mut runner)?,
        WorkerAction::LiveInstall { plan, manifest } => {
            rustaller::worker::install::run_from_files(
                &mut runner,
                Path::new(&plan),
                Path::new(&manifest),
            )?;
        }
        WorkerAction::FirstbootFix => run_firstboot(&mut runner, Path::new("/"))?,
        WorkerAction::CachyosFirstboot => {
            rustaller::targets::cachyos::run_firstboot_update(&mut runner, Path::new("/"))?
        }
        WorkerAction::CleanupOpenRc => cleanup_firstboot(&mut runner, Path::new("/"))?,
        WorkerAction::InitrdNetwork => {
            rustaller::worker::initrd_network::run_from_files(&mut runner)?;
        }
    }
    Ok(())
}
