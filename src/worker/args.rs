use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerAction {
    LiveModules,
    FirstbootFix,
    CachyosFirstboot,
    CleanupOpenRc,
    CleanupSystemd,
    InitrdNetwork(super::initrd_network::InitrdNetworkConfig),
    LogStream { path: String },
    LiveInstall { plan: String, manifest: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerArgsError(pub Vec<String>);

impl fmt::Display for WorkerArgsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid worker arguments: {}", self.0.join(" "))
    }
}

impl std::error::Error for WorkerArgsError {}

pub fn parse_worker_args<I, S>(args: I) -> Result<Option<WorkerAction>, WorkerArgsError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect();
    match args.as_slice() {
        [] => Ok(None),
        [command] if command == "--help" || command == "-h" => Ok(None),
        [command] if command == "live-modules" => Ok(Some(WorkerAction::LiveModules)),
        [command] if command == "firstboot-fix" => Ok(Some(WorkerAction::FirstbootFix)),
        [command] if command == "cachyos-firstboot" => Ok(Some(WorkerAction::CachyosFirstboot)),
        [command] if command == "cleanup-openrc" => Ok(Some(WorkerAction::CleanupOpenRc)),
        [command] if command == "cleanup-systemd" => Ok(Some(WorkerAction::CleanupSystemd)),
        [command, path] if command == "log-stream" && super::log_stream::valid_web_path(path) => {
            Ok(Some(WorkerAction::LogStream { path: path.clone() }))
        }
        [command, plan, manifest]
            if command == "live-install"
                && valid_private_path(plan)
                && valid_private_path(manifest)
                && plan != manifest =>
        {
            Ok(Some(WorkerAction::LiveInstall {
                plan: plan.clone(),
                manifest: manifest.clone(),
            }))
        }
        [
            command,
            mac,
            ipv4,
            ipv4_gateway,
            ipv6,
            ipv6_gateway,
            ipv6_extra,
        ] if command == "initrd-network" => {
            let config = super::initrd_network::InitrdNetworkConfig::new(
                mac,
                ipv4,
                ipv4_gateway,
                ipv6,
                ipv6_gateway,
                ipv6_extra,
            )
            .map_err(|_| WorkerArgsError(args.clone()))?;
            Ok(Some(WorkerAction::InitrdNetwork(config)))
        }
        _ => Err(WorkerArgsError(args)),
    }
}

fn valid_private_path(path: &str) -> bool {
    let path = std::path::Path::new(path);
    path.is_absolute()
        && path.file_name().is_some()
        && path.as_os_str().len() <= 4096
        && !path
            .as_os_str()
            .to_string_lossy()
            .chars()
            .any(char::is_control)
        && !path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
}
