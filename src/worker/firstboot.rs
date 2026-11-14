use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use crate::fsutil::replace_config as atomic_replace;
use crate::shared::configure::{
    validate_directory_tree, validate_planned_file, validate_planned_parent, write_config,
};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitManager {
    OpenRc,
    Systemd,
}

#[derive(Debug)]
pub enum FirstbootError {
    Command(io::Error),
    CommandFailed {
        name: &'static str,
        status: Option<i32>,
    },
    NoInterfaces,
    WorkerVersionMismatch,
    Filesystem(io::Error),
}

impl fmt::Display for FirstbootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(error) => write!(f, "Firstboot command failed to start: {error}"),
            Self::CommandFailed { name, status } => {
                write!(f, "Firstboot command {name} failed with status {status:?}")
            }
            Self::NoInterfaces => f.write_str("No network interface became available"),
            Self::WorkerVersionMismatch => {
                f.write_str("Refusing to install a worker with an incompatible runtime version")
            }
            Self::Filesystem(error) => write!(f, "Firstboot network configuration failed: {error}"),
        }
    }
}

impl std::error::Error for FirstbootError {}

impl From<io::Error> for FirstbootError {
    fn from(error: io::Error) -> Self {
        Self::Filesystem(error)
    }
}
impl From<crate::shared::configure::ConfigurationError> for FirstbootError {
    fn from(error: crate::shared::configure::ConfigurationError) -> Self {
        Self::Filesystem(io::Error::other(error))
    }
}

pub fn non_loopback_links(ip_link_output: &str) -> String {
    let mut state = String::new();
    for line in ip_link_output
        .lines()
        .filter(|line| line.split_whitespace().nth(1) != Some("lo:"))
    {
        state.push_str(line);
        state.push('\n');
    }
    state
}

pub fn wait_for_stable_links<R: CommandRunner>(runner: &mut R) -> Result<String, FirstbootError> {
    let mut check_count = 0u32;
    let mut stable_count = 0u32;
    let mut has_interface = false;
    let mut old_state = String::new();

    loop {
        check_count += 1;
        let command = CommandSpec::new("ip", ["-o", "link"])
            .phase("firstboot-wait-for-nics")
            .stdin(CommandInput::Null)
            .stdout_mode(OutputMode::Capture);
        let output = runner.run(&command).map_err(FirstbootError::Command)?;
        let status = output.status;
        if status != Some(0) {
            return Err(FirstbootError::CommandFailed { name: "ip", status });
        }
        let current_state = non_loopback_links(&String::from_utf8_lossy(&output.stdout))
            .trim_end_matches(char::from(10))
            .to_owned();
        if !current_state.is_empty() {
            has_interface = true;
        }
        if has_interface && old_state == current_state {
            stable_count += 1;
        } else {
            stable_count = 0;
        }
        old_state.clone_from(&current_state);
        println!("Waiting for the NICs to be stable ({stable_count}s/10s)...");

        if has_interface && stable_count >= 10 {
            return Ok(current_state);
        }
        if !has_interface && check_count >= 60 {
            return Err(FirstbootError::NoInterfaces);
        }

        let sleep = CommandSpec::new("sleep", ["1"])
            .phase("firstboot-wait-for-nics")
            .stdin(CommandInput::Null)
            .stdout_mode(OutputMode::Discard)
            .stderr_mode(OutputMode::Discard);
        let output = runner.run(&sleep).map_err(FirstbootError::Command)?;
        if output.status != Some(0) {
            return Err(FirstbootError::CommandFailed {
                name: "sleep",
                status: output.status,
            });
        }
    }
}

pub fn run_firstboot<R: CommandRunner>(runner: &mut R, root: &Path) -> Result<(), FirstbootError> {
    wait_for_stable_links(runner)?;
    apply_network_fixes(runner, root).map_err(FirstbootError::Filesystem)
}

pub fn render_firstboot_service(manager: InitManager) -> &'static str {
    match manager {
        InitManager::Systemd => SYSTEMD_FIRSTBOOT_SERVICE,
        InitManager::OpenRc => OPENRC_FIRSTBOOT_SERVICE,
    }
}

pub fn install_firstboot_service<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    manager: InitManager,
    worker_elf: &[u8],
) -> Result<(), FirstbootError> {
    if worker_elf.is_empty() {
        return Err(FirstbootError::Filesystem(io::Error::new(
            io::ErrorKind::InvalidData,
            "refusing to install an empty worker binary",
        )));
    }
    if !worker_elf
        .windows(crate::RUNTIME_VERSION.len())
        .any(|window| window == crate::RUNTIME_VERSION.as_bytes())
    {
        return Err(FirstbootError::WorkerVersionMismatch);
    }
    let worker = root.join("usr/local/bin/rustaller-worker");
    write_config::<FirstbootError>(&worker, worker_elf, 0o755)?;

    let (service_path, command_args, service_mode) = match manager {
        InitManager::OpenRc => (
            root.join("etc/init.d/fix-eth-name"),
            vec![
                root.as_os_str().to_owned(),
                OsString::from("rc-update"),
                OsString::from("add"),
                OsString::from("fix-eth-name"),
                OsString::from("boot"),
            ],
            0o755,
        ),
        InitManager::Systemd => (
            root.join("etc/systemd/system/fix-eth-name.service"),
            vec![
                root.as_os_str().to_owned(),
                OsString::from("systemctl"),
                OsString::from("enable"),
                OsString::from("fix-eth-name.service"),
            ],
            0o644,
        ),
    };
    write_config::<FirstbootError>(
        &service_path,
        render_firstboot_service(manager).as_bytes(),
        service_mode,
    )?;

    let command_name = if manager == InitManager::OpenRc {
        "rc-update"
    } else {
        "systemctl"
    };
    let command = CommandSpec::new("chroot", command_args)
        .phase("install-firstboot-service")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream);
    let output = runner.run(&command).map_err(FirstbootError::Command)?;
    if output.status != Some(0) {
        return Err(FirstbootError::CommandFailed {
            name: command_name,
            status: output.status,
        });
    }

    if manager == InitManager::Systemd {
        let usr_preset = root.join("usr/lib/systemd/system-preset");
        let preset_dir = if usr_preset.is_dir() {
            usr_preset
        } else {
            root.join("lib/systemd/system-preset")
        };
        if !preset_dir.is_dir() {
            return Err(FirstbootError::Filesystem(io::Error::new(
                io::ErrorKind::NotFound,
                "systemd system-preset directory is missing",
            )));
        }
        let preset = preset_dir.join("01-fix-eth-name.preset");
        write_config::<FirstbootError>(&preset, b"enable fix-eth-name.service\n", 0o644)?;
    }
    Ok(())
}

pub fn cleanup_firstboot<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    manager: InitManager,
) -> Result<(), FirstbootError> {
    if manager == InitManager::OpenRc {
        let commands: [(&str, &[&str]); 2] = [
            ("rc-service", &["fix-eth-name", "zap"]),
            ("rc-update", &["del", "fix-eth-name", "boot"]),
        ];
        for (program, args) in commands {
            let command = CommandSpec::new(program, args.iter().copied())
                .phase("firstboot-cleanup-openrc")
                .stdin(CommandInput::Null)
                .stdout_mode(OutputMode::Stream)
                .stderr_mode(OutputMode::Stream);
            let output = runner.run(&command).map_err(FirstbootError::Command)?;
            if output.status != Some(0) {
                return Err(FirstbootError::CommandFailed {
                    name: if program == "rc-service" {
                        "rc-service"
                    } else {
                        "rc-update"
                    },
                    status: output.status,
                });
            }
        }
        remove_if_exists(&root.join("etc/init.d/fix-eth-name"))?;
    } else {
        for path in [
            "etc/systemd/system/fix-eth-name.service",
            "etc/systemd/system/multi-user.target.wants/fix-eth-name.service",
            "usr/lib/systemd/system-preset/01-fix-eth-name.preset",
        ] {
            remove_if_exists(&root.join(path))?;
        }
        // The installer selects usr/lib on merged-/usr targets. Only inspect
        // the legacy lib path when it is a real directory, never through a
        // link that could lead outside an isolated target root.
        if fs::symlink_metadata(root.join("lib")).is_ok_and(|metadata| metadata.is_dir()) {
            remove_if_exists(&root.join("lib/systemd/system-preset/01-fix-eth-name.preset"))?;
        }
    }
    remove_if_exists(&root.join("usr/local/bin/rustaller-worker"))
        .map_err(FirstbootError::Filesystem)
}

pub fn apply_network_fixes<R: CommandRunner>(runner: &mut R, root: &Path) -> io::Result<()> {
    apply_network_fixes_with_lookup(root, |mac| lookup_ethx_by_mac(runner, mac, false))
}

pub fn apply_network_fixes_from_links(root: &Path, ip_link_output: &str) -> io::Result<()> {
    apply_network_fixes_with_lookup(root, |mac| {
        Ok(super::initrd_network::interface_for_mac(
            ip_link_output,
            mac,
        ))
    })
}

pub fn lookup_ethx_by_mac<R: CommandRunner>(
    runner: &mut R,
    mac: &str,
    include_master: bool,
) -> io::Result<Option<String>> {
    let command = CommandSpec::new("ip", ["-o", "link"])
        .phase("firstboot-nic-lookup")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Stream);
    let output = runner.run(&command)?;
    if output.status != Some(0) {
        return Err(io::Error::other(
            "ip failed while looking up NIC MAC address",
        ));
    }
    let links =
        String::from_utf8(output.stdout).map_err(|_| io::Error::other("invalid NIC inventory"))?;
    let names = eth_names_by_mac(&links, mac, include_master);
    if names.len() > 1 {
        return Err(io::Error::other("ambiguous NIC MAC identity"));
    }
    Ok(names.into_iter().next())
}

pub fn eth_names_by_mac(ip_link_output: &str, mac: &str, include_master: bool) -> Vec<String> {
    let mac = mac.to_ascii_lowercase();
    ip_link_output
        .lines()
        .filter(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields
                .windows(2)
                .any(|pair| pair[0] == "link/ether" && pair[1].eq_ignore_ascii_case(&mac))
        })
        .filter(|line| {
            let has_master = line.split_whitespace().any(|word| word == "master");
            if include_master {
                has_master
            } else {
                !has_master
            }
        })
        .filter_map(|line| {
            line.split_whitespace()
                .nth(1)
                .map(|field| {
                    field
                        .trim_end_matches(':')
                        .split('@')
                        .next()
                        .unwrap_or(field)
                })
                .filter(|name| crate::deployment::valid_interface_name(name))
                .map(str::to_owned)
        })
        .collect()
}

pub fn rewrite_ifupdown<F>(input: &str, mut get_ethx_by_mac: F) -> Vec<u8>
where
    F: FnMut(&str) -> Option<String>,
{
    rewrite_ifupdown_fallible(input, |mac| Ok(get_ethx_by_mac(mac)))
        .expect("the public ifupdown resolver is infallible")
}

fn rewrite_ifupdown_fallible<F>(input: &str, mut get_ethx_by_mac: F) -> io::Result<Vec<u8>>
where
    F: FnMut(&str) -> io::Result<Option<String>>,
{
    let mut current_eth = None;
    let mut output = String::new();

    for line in input.lines() {
        let mut line = line.to_owned();
        if let Some(mac_line) = line.strip_prefix("# mac ") {
            let mac = mac_line.split_whitespace().last().unwrap_or_default();
            current_eth = get_ethx_by_mac(mac)?;
            continue;
        }

        if let Some(eth) = current_eth.as_deref() {
            if line.starts_with("iface ")
                || line.starts_with("auto ")
                || line.starts_with("allow-hotplug ")
            {
                let mut fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() >= 2 {
                    fields[1] = eth;
                    line = fields.join(" ");
                }
            } else if line.contains(" dev ")
                && let Some(last_space) = line.rfind(' ')
            {
                line.replace_range(last_space + 1.., eth);
            }
        }

        output.push_str(&line);
        output.push('\n');
    }
    Ok(output.into_bytes())
}

pub fn rewrite_networkd_file<F>(
    filename: &str,
    input: &str,
    mut get_ethx_by_mac: F,
) -> Option<(String, String)>
where
    F: FnMut(&str) -> Option<String>,
{
    if !filename.starts_with("10-cloud-init-eth") || !filename.ends_with(".network") {
        return None;
    }
    rewrite_networkd_file_fallible(filename, input, |mac| Ok(get_ethx_by_mac(mac)))
        .ok()
        .flatten()
}

fn rewrite_networkd_file_fallible<F>(
    filename: &str,
    input: &str,
    mut get_ethx_by_mac: F,
) -> io::Result<Option<(String, String)>>
where
    F: FnMut(&str) -> io::Result<Option<String>>,
{
    if !filename.starts_with("10-cloud-init-eth") || !filename.ends_with(".network") {
        return Ok(None);
    }
    let Some(mac) = input
        .lines()
        .find_map(|line| line.strip_prefix("MACAddress="))
        .filter(|mac| !mac.is_empty())
    else {
        return Ok(None);
    };
    let Some(eth) = get_ethx_by_mac(mac)? else {
        return Ok(None);
    };

    let mut output = String::new();
    for line in input.lines() {
        output.push_str(&replace_eth_name(line, &eth));
        output.push('\n');
    }
    Ok(Some((format!("10-{eth}.network"), output)))
}

fn apply_network_fixes_with_lookup<F>(root: &Path, mut lookup: F) -> io::Result<()>
where
    F: FnMut(&str) -> io::Result<Option<String>>,
{
    let interfaces = root.join("etc/network/interfaces");
    validate_planned_file(&interfaces).map_err(io::Error::other)?;
    if interfaces.is_file() {
        let input = fs::read_to_string(&interfaces)?;
        if input.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "refusing to rewrite empty interfaces file",
            ));
        }
        let output = rewrite_ifupdown_fallible(&input, |mac| {
            let name = lookup(mac)?
                .ok_or_else(|| io::Error::other("inherited interface MAC is missing"))?;
            Ok(Some(name))
        })?;
        atomic_replace(&interfaces, &output)?;
    }

    let networkd = root.join("etc/systemd/network");
    let entries = match fs::read_dir(&networkd) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    validate_directory_tree(&networkd).map_err(io::Error::other)?;
    let mut files = entries
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("10-cloud-init-eth") && name.ends_with(".network")
                })
        })
        .collect::<Vec<_>>();
    files.sort();

    for path in files {
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        validate_planned_file(&path).map_err(io::Error::other)?;
        let input = fs::read_to_string(&path)?;
        let Some((new_name, output)) =
            rewrite_networkd_file_fallible(filename, &input, |mac| lookup(mac))?
        else {
            continue;
        };
        let destination = path.with_file_name(new_name);
        if destination != path && fs::symlink_metadata(&destination).is_ok() {
            return Err(io::Error::other("firstboot destination already exists"));
        }
        atomic_replace(&path, output.as_bytes())?;
        fs::rename(path, destination)?;
    }
    Ok(())
}

fn replace_eth_name(line: &str, eth: &str) -> String {
    let Some(rest) = line.strip_prefix("Name=eth") else {
        return line.to_owned();
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return line.to_owned();
    }
    format!("Name={eth}{}", &rest[digits..])
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    validate_planned_parent(path).map_err(io::Error::other)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

const SYSTEMD_FIRSTBOOT_SERVICE: &str = "[Unit]\nDescription=Fix Eth Name\nConditionPathExists=/usr/local/bin/rustaller-worker\n\nAfter=dbus.service\n\nBefore=networking.service\nBefore=systemd-networkd.service\n\nBefore=network.target\n\n[Service]\nType=oneshot\n\nStandardOutput=journal+console\nStandardError=journal+console\n\nExecStart=/usr/local/bin/rustaller-worker firstboot-fix\nExecStartPost=/usr/local/bin/rustaller-worker cleanup-systemd\n\n[Install]\nWantedBy=multi-user.target\n";

const OPENRC_FIRSTBOOT_SERVICE: &str = "#!/sbin/openrc-run\n# shellcheck shell=sh\n\n# shellcheck disable=SC2034 # OpenRC reads this variable.\nDescription=\"Fix Eth Name\"\n\n# https://gitlab.alpinelinux.org/alpine/aports/-/blob/master/main/openrc/networking.initd\n# https://gitlab.alpinelinux.org/alpine/aports/-/blob/master/main/dhcpcd/dhcpcd.initd\ndepend() {\n    need localmount\n    want dev-settle\n\n    after bootmisc hwdrivers modules\n    before net networking dhcpcd\n}\n\nstart() {\n    ebegin \"Fix Eth Name\"\n    /usr/local/bin/rustaller-worker firstboot-fix\n    eend $?\n}\n\nstart_post() {\n    /usr/local/bin/rustaller-worker cleanup-openrc\n}\n";
