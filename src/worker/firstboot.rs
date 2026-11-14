use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use crate::fsutil::replace_config as atomic_replace;
use crate::shared::configure::{validate_planned_file, validate_planned_parent, write_config};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

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

pub fn render_firstboot_service() -> &'static str {
    OPENRC_FIRSTBOOT_SERVICE
}

pub fn install_firstboot_service<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
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
    validate_planned_file(&worker)?;
    validate_planned_file(&root.join("etc/init.d/fix-eth-name"))?;
    write_config::<FirstbootError>(&worker, worker_elf, 0o755)?;

    write_config::<FirstbootError>(
        &root.join("etc/init.d/fix-eth-name"),
        render_firstboot_service().as_bytes(),
        0o755,
    )?;
    let command_args = [
        root.as_os_str().to_owned(),
        OsString::from("rc-update"),
        OsString::from("add"),
        OsString::from("fix-eth-name"),
        OsString::from("boot"),
    ];
    let command = CommandSpec::new("chroot", command_args)
        .phase("install-firstboot-service")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Stream)
        .stderr_mode(OutputMode::Stream);
    let output = runner.run(&command).map_err(FirstbootError::Command)?;
    if output.status != Some(0) {
        return Err(FirstbootError::CommandFailed {
            name: "rc-update",
            status: output.status,
        });
    }

    Ok(())
}

pub fn cleanup_firstboot<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
) -> Result<(), FirstbootError> {
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

fn apply_network_fixes_with_lookup<F>(root: &Path, mut lookup: F) -> io::Result<()>
where
    F: FnMut(&str) -> io::Result<Option<String>>,
{
    let interfaces = root.join("etc/network/interfaces");
    validate_planned_file(&interfaces).map_err(io::Error::other)?;

    let input = fs::read_to_string(&interfaces)?;
    if input.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "refusing to rewrite empty interfaces file",
        ));
    }
    let output = rewrite_ifupdown_fallible(&input, |mac| {
        let name =
            lookup(mac)?.ok_or_else(|| io::Error::other("inherited interface MAC is missing"))?;
        Ok(Some(name))
    })?;
    atomic_replace(&interfaces, &output)?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    validate_planned_parent(path).map_err(io::Error::other)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

const OPENRC_FIRSTBOOT_SERVICE: &str = "#!/sbin/openrc-run\nDescription=\"Fix Eth Name\"\n\n# https://gitlab.alpinelinux.org/alpine/aports/-/blob/master/main/openrc/networking.initd\n# https://gitlab.alpinelinux.org/alpine/aports/-/blob/master/main/dhcpcd/dhcpcd.initd\ndepend() {\n    need localmount\n    want dev-settle\n\n    after bootmisc hwdrivers modules\n    before net networking dhcpcd\n}\n\nstart() {\n    ebegin \"Fix Eth Name\"\n    /usr/local/bin/rustaller-worker firstboot-fix\n    eend $?\n}\n\nstart_post() {\n    /usr/local/bin/rustaller-worker cleanup-openrc\n}\n";
