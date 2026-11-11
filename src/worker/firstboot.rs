use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use crate::fsutil::replace_config as atomic_replace;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, Permissions};
use std::io;
use std::os::unix::fs::PermissionsExt;
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
        let pipeline = [
            CommandSpec::new("ip", ["-o", "link"])
                .phase("firstboot-wait-for-nics")
                .stdin(CommandInput::Null),
            CommandSpec::new("awk", [format!("$2 != {quote}lo:{quote}", quote = '\"')])
                .phase("firstboot-wait-for-nics")
                .stdout_mode(OutputMode::Capture),
        ];
        let output = runner
            .run_pipeline(&pipeline)
            .map_err(FirstbootError::Command)?;
        let status = output.statuses.last().copied().flatten();
        if status != Some(0) {
            return Err(FirstbootError::CommandFailed {
                name: "awk",
                status,
            });
        }
        let current_state = String::from_utf8_lossy(&output.stdout)
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
    let worker = root.join("usr/local/bin/reinstall-worker");
    fs::create_dir_all(worker.parent().expect("worker path has parent"))
        .map_err(FirstbootError::Filesystem)?;
    atomic_replace(&worker, worker_elf).map_err(FirstbootError::Filesystem)?;
    fs::set_permissions(&worker, Permissions::from_mode(0o755))
        .map_err(FirstbootError::Filesystem)?;

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
    fs::create_dir_all(service_path.parent().expect("service path has parent"))
        .map_err(FirstbootError::Filesystem)?;
    atomic_replace(&service_path, render_firstboot_service(manager).as_bytes())
        .map_err(FirstbootError::Filesystem)?;
    fs::set_permissions(&service_path, Permissions::from_mode(service_mode))
        .map_err(FirstbootError::Filesystem)?;

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
        atomic_replace(&preset, b"enable fix-eth-name.service\n")
            .map_err(FirstbootError::Filesystem)?;
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
        remove_if_exists(&root.join("etc/init.d/fix-eth-name"))
            .map_err(FirstbootError::Filesystem)?;
    } else {
        for path in [
            "etc/systemd/system/fix-eth-name.service",
            "etc/systemd/system/multi-user.target.wants/fix-eth-name.service",
            "lib/systemd/system-preset/01-fix-eth-name.preset",
            "usr/lib/systemd/system-preset/01-fix-eth-name.preset",
        ] {
            remove_if_exists(&root.join(path)).map_err(FirstbootError::Filesystem)?;
        }
    }
    remove_if_exists(&root.join("usr/local/bin/reinstall-worker"))
        .map_err(FirstbootError::Filesystem)
}

pub fn apply_network_fixes<R: CommandRunner>(runner: &mut R, root: &Path) -> io::Result<()> {
    apply_network_fixes_with_lookup(root, |mac| lookup_ethx_by_mac(runner, mac, false))
}

pub fn apply_network_fixes_from_links(root: &Path, ip_link_output: &str) -> io::Result<()> {
    apply_network_fixes_with_lookup(root, |mac| {
        Ok(eth_names_by_mac(ip_link_output, mac, false)
            .into_iter()
            .next())
    })
}

pub fn lookup_ethx_by_mac<R: CommandRunner>(
    runner: &mut R,
    mac: &str,
    include_master: bool,
) -> io::Result<Option<String>> {
    let mut input = mac.as_bytes().to_vec();
    input.push(10);
    let lower = CommandSpec::new("tr", ["[:upper:]", "[:lower:]"])
        .phase("firstboot-nic-lookup")
        .stdin(CommandInput::Bytes(input))
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Stream);
    let output = runner.run(&lower)?;
    if output.status != Some(0) {
        return Err(io::Error::other(
            "tr failed while normalizing NIC MAC address",
        ));
    }
    let mac = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(char::from(10))
        .to_owned();
    let mut commands = vec![
        CommandSpec::new("ip", ["-o", "link"])
            .phase("firstboot-nic-lookup")
            .stdin(CommandInput::Null),
        CommandSpec::new("grep", ["-i", mac.as_str()]).phase("firstboot-nic-lookup"),
    ];
    if include_master {
        commands.push(CommandSpec::new("grep", ["-w", "master"]).phase("firstboot-nic-lookup"));
    } else {
        commands.push(CommandSpec::new("grep", ["-v", "master"]).phase("firstboot-nic-lookup"));
    }
    commands.push(CommandSpec::new("awk", ["{print $2}"]).phase("firstboot-nic-lookup"));
    commands.push(CommandSpec::new("cut", ["-d:", "-f1"]).phase("firstboot-nic-lookup"));
    commands.push(
        CommandSpec::new("grep", ["."])
            .phase("firstboot-nic-lookup")
            .stdout_mode(OutputMode::Capture),
    );
    let output = runner.run_pipeline(&commands)?;
    let names = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(char::from(10))
        .to_owned();
    Ok((!names.is_empty()).then_some(names))
}

pub fn eth_names_by_mac(ip_link_output: &str, mac: &str, include_master: bool) -> Vec<String> {
    let mac = mac.to_ascii_lowercase();
    ip_link_output
        .lines()
        .filter(|line| line.to_ascii_lowercase().contains(&mac))
        .filter(|line| {
            let has_master = line.contains("master");
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
                        .split(':')
                        .next()
                        .unwrap_or(field)
                })
                .filter(|name| !name.is_empty())
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
            if line.starts_with("iface e")
                || line.starts_with("auto e")
                || line.starts_with("allow-hotplug e")
            {
                let mut fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() >= 2 {
                    fields[1] = eth;
                    line = fields.join(" ");
                }
            } else if line.contains(" dev e")
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
    if interfaces.is_file() {
        let input = fs::read_to_string(&interfaces)?;
        if input.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "refusing to rewrite empty interfaces file",
            ));
        }
        let output = rewrite_ifupdown_fallible(&input, |mac| lookup(mac))?;
        atomic_replace(&interfaces, &output)?;
    }

    let networkd = root.join("etc/systemd/network");
    let entries = match fs::read_dir(&networkd) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut files = entries
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();

    for path in files {
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let input = fs::read_to_string(&path)?;
        let Some((new_name, output)) =
            rewrite_networkd_file_fallible(filename, &input, |mac| lookup(mac))?
        else {
            continue;
        };
        atomic_replace(&path, output.as_bytes())?;
        let destination = path.with_file_name(new_name);
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
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

const SYSTEMD_FIRSTBOOT_SERVICE: &str = "[Unit]\nDescription=Fix Eth Name\nConditionPathExists=/usr/local/bin/reinstall-worker\n\nAfter=dbus.service\n\nBefore=networking.service\nBefore=systemd-networkd.service\n\nBefore=network.target\n\n[Service]\nType=oneshot\n\nStandardOutput=journal+console\nStandardError=journal+console\n\nExecStart=/usr/local/bin/reinstall-worker firstboot-fix\nExecStartPost=/usr/local/bin/reinstall-worker cleanup-systemd\n\n[Install]\nWantedBy=multi-user.target\n";

const OPENRC_FIRSTBOOT_SERVICE: &str = "#!/sbin/openrc-run\n# shellcheck shell=sh\n\n# shellcheck disable=SC2034 # OpenRC reads this variable.\nDescription=\"Fix Eth Name\"\n\n# https://gitlab.alpinelinux.org/alpine/aports/-/blob/master/main/openrc/networking.initd\n# https://gitlab.alpinelinux.org/alpine/aports/-/blob/master/main/dhcpcd/dhcpcd.initd\ndepend() {\n    need localmount\n    want dev-settle\n\n    after bootmisc hwdrivers modules\n    before net networking dhcpcd\n}\n\nstart() {\n    ebegin \"Fix Eth Name\"\n    /usr/local/bin/reinstall-worker firstboot-fix\n    eend $?\n}\n\nstart_post() {\n    /usr/local/bin/reinstall-worker cleanup-openrc\n}\n";
