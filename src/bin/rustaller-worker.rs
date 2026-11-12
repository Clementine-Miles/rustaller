use rustaller::command::{SwapSafeRunner, SystemRunner};
use rustaller::worker::args::{WorkerAction, parse_worker_args};
use rustaller::worker::firstboot::{InitManager, cleanup_firstboot, run_firstboot};
use rustaller::worker::initrd_network::run_initrd_network_with_proxy;
use rustaller::worker::log_stream::run_log_stream;
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
            "Usage: rustaller-worker <live-install PLAN MANIFEST|firstboot-fix|cachyos-firstboot|cleanup-openrc|cleanup-systemd|initrd-network|log-stream>\n\nRuntime version: {}\nInternal helper for the Rust rustaller installer.",
            rustaller::RUNTIME_VERSION
        );
        return Ok(());
    };

    let mut runner = SwapSafeRunner::new(SystemRunner);
    match action {
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
        WorkerAction::CleanupOpenRc => {
            cleanup_firstboot(&mut runner, Path::new("/"), InitManager::OpenRc)?
        }
        WorkerAction::CleanupSystemd => {
            cleanup_firstboot(&mut runner, Path::new("/"), InitManager::Systemd)?
        }
        WorkerAction::InitrdNetwork(config) => {
            let proxy = ["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY"]
                .into_iter()
                .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
            match run_initrd_network_with_proxy(
                &mut runner,
                Path::new("/"),
                &config,
                proxy.as_deref(),
            )? {
                Some(result) => println!(
                    "Configured {} (IPv4 internet: {}, IPv6 internet: {})",
                    result.interface, result.ipv4_has_internet, result.ipv6_has_internet
                ),
                None => println!("Not found network card: {}", config.mac_addr()),
            }
        }
        WorkerAction::LogStream { path } => {
            let request_path = std::env::var("PATH_INFO").unwrap_or_default();
            run_log_stream(&mut runner, &path, &request_path)?;
        }
    }
    Ok(())
}
