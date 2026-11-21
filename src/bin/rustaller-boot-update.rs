//! Installed native package hooks. This binary contains no handoff or download actions.
use rustaller::{command::SystemRunner, targets::boot};
use std::{env, path::Path};

fn main() {
    if let Err(error) = run() {
        eprintln!("rustaller-boot-update: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = env::args().collect();
    let name = Path::new(&argv[0]).file_name().and_then(|v| v.to_str());
    let action = match (name, &argv[1..]) {
        (Some("zz-rustaller-boot-check"), _) => "check",
        (Some("zz-rustaller-boot"), args) if !boot::hook_action(args) => "check",
        (Some("zz-rustaller-boot"), _) => "refresh",
        (_, [arg]) => arg.as_str(),
        _ => return Err("expected check, verify, refresh or --version".into()),
    };
    match action {
        "--version" => {
            println!("{}", rustaller::RUNTIME_VERSION);
            Ok(())
        }
        "verify" => {
            boot::check(&mut SystemRunner, Path::new("/"))?;
            boot::verify(Path::new("/"))
        }
        "check" => boot::check(&mut SystemRunner, Path::new("/")),
        "refresh" => boot::refresh(&mut SystemRunner, Path::new("/")),
        _ => Err("expected check, verify, refresh or --version".into()),
    }
}
