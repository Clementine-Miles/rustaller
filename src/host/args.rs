use crate::config::{DnsMode, InstallProfile};
use crate::model::{BootMode, InstallPlan, IsoOptions, PlanError, Target};
use std::fmt;

pub const HELP: &str = r#"Usage: rustaller [OPTIONS] TARGET

Targets:
  alpine       Latest stable Alpine Linux
  debian       Latest stable Debian, installed from Alpine Live
  arch         Arch Linux
  gentoo       Gentoo Linux
  netboot.xyz  Start the netboot.xyz installer
  iso          Boot a local Linux ISO without formatting disks
  reset        Remove the previously installed rustaller boot entry

Options:
  --username USERNAME           Target account name
  --password PASSWORD           Target account password
  --ssh-key KEY                 SSH public key (repeatable)
  --ssh-port PORT               SSH port
  --web-port PORT               Web log viewer port
  --frpc-config PATH            frpc configuration
  --target-disk /dev/DISK       Target disk for automatic installation
  --no-cloud-kernel             Select the non-cloud kernel
  --kernel CHOICE               Official kernel: auto/cloud/generic/lts/standard
  --hostname NAME               Set target hostname (one ASCII label)
  --timezone ZONE               Set target timezone, e.g. Etc/UTC
  --profile standard|minimal    Select target installation footprint
  --minimal                    Alias for --profile minimal
  --with-tools                 Add curl and less (Gentoo equivalents)
  --keep-docs                  Keep documentation in the minimal profile
  --keep-package-cache         Keep downloaded packages in minimal profile
  --dns-mode auto|static       Target DNS policy (default: auto)
  --dns IP[,IP]                Static upstream DNS, repeatable, maximum 3
  --dns-search DOMAIN[,DOMAIN] DNS search domains, repeatable, maximum 6
  --tcp-congestion CHOICE      default|bbr|cubic (bbr requires --qdisc fq)
  --qdisc CHOICE               default|fq|fq_codel
  --ipv6 POLICY               inherit|enable|disable (target only)
  --address-preference CHOICE  default|ipv4 (glibc targets only)
  --mtu BYTES                 Target non-loopback NIC MTU, 1280-9000
  --tcp-buffer-mib MIB        Maximum TCP/socket buffer, 1-64 MiB
  --somaxconn N               Listen backlog, 128-262144
  --syn-backlog N             SYN backlog, 128-262144
  --netdev-backlog N          Network device backlog, 128-262144
  --tcp-mtu-probing N         0=off, 1=on blackhole, 2=always
  --tcp-fastopen N            0=off, 1=client, 2=server, 3=both
  --tcp-syncookies N          0=off, 1=on
  --swapfile-mib MIB          Opt-in target /swapfile, 64-65536 MiB
  --swappiness N              Target vm.swappiness, 0-200
  --hold 0|1|2                  Select the existing hold behavior
  --force-boot-mode bios|efi    Override detected boot mode
  --confhome HTTPS_URL          Root URL for matching installer resources
  --iso PATH                    Local ISO image
  --iso-config PATH             GRUB loopback configuration in the ISO
  --iso-kernel PATH             Kernel path in the ISO
  --iso-initrd PATH             Initrd path in the ISO
  --iso-cmdline TEXT            Kernel command line for custom ISO boot
  --debug, -x                   Enable debug diagnostics
  --inspect                    Read-only disk identity and handoff inspection
  --confirm-erase TOKEN        Exact identity token from --inspect (no prompt)
  --confirm-handoff TOKEN      Explicit handoff token (also supports Live rescue)
  --help, -h                    Show this help

Linux x86_64 hosts only. Automatic installation erases the selected disk.
Rust embeds its matching worker and verifies official installation sources.
Development installer: BIOS/UEFI acceptance is required before production use.
"#;

pub enum CliOutcome {
    Help,
    Plan(Box<InstallPlan>),
    Inspect(Box<InstallPlan>),
    ConfirmedPlan {
        plan: Box<InstallPlan>,
        confirmation: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliError {
    MissingTarget,
    MultipleTargets,
    UnknownOption(String),
    MissingValue(String),
    InvalidValue(String),
    InvalidTarget(String),
    InvalidPlan(PlanError),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTarget => f.write_str("Specify a supported target"),
            Self::MultipleTargets => f.write_str("Specify exactly one target"),
            Self::UnknownOption(option) => write!(f, "Unexpected option: {option}"),
            Self::MissingValue(option) => write!(f, "Need value for {option}"),
            Self::InvalidValue(option) => write!(f, "Invalid value for {option}"),
            Self::InvalidTarget(target) => write!(f, "Unsupported target: {target}"),
            Self::InvalidPlan(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CliError {}

pub fn parse_args<I, S>(args: I, env_confhome: Option<&str>) -> Result<CliOutcome, CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(CliOutcome::Help);
    }

    let mut target = None;
    let mut inspect = false;
    let mut confirmation = None;
    let mut dns_mode = None;
    let mut options = InstallPlan::new(Target::Reset);
    let mut confhome = env_confhome
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let mut iso_fields = IsoOptions {
        path: String::new(),
        config: None,
        kernel: None,
        initrd: None,
        cmdline: None,
    };
    let mut has_iso_field = false;
    let mut options_ended = false;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if options_ended {
            set_target(&mut target, arg)?;
            index += 1;
            continue;
        }
        if arg == "--" {
            options_ended = true;
            index += 1;
            continue;
        }

        let (name, inline_value) = if let Some(value) = arg.strip_prefix("--") {
            match value.split_once('=') {
                Some((name, value)) => (format!("--{name}"), Some(value.to_owned())),
                None => (arg.clone(), None),
            }
        } else {
            (arg.clone(), None)
        };

        match name.as_str() {
            "--inspect" => {
                reject_inline(&name, inline_value.as_deref())?;
                inspect = true;
                index += 1;
            }
            "--confirm-erase" | "--confirm-handoff" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                if confirmation.is_some()
                    || value.len() > 4096
                    || value.chars().any(char::is_control)
                {
                    return Err(CliError::InvalidValue(name));
                }
                confirmation = Some(value);
                index += consumed;
            }
            "--tcp-congestion"
            | "--qdisc"
            | "--ipv6"
            | "--address-preference"
            | "--mtu"
            | "--tcp-buffer-mib"
            | "--somaxconn"
            | "--syn-backlog"
            | "--netdev-backlog"
            | "--tcp-mtu-probing"
            | "--tcp-fastopen"
            | "--tcp-syncookies"
            | "--swapfile-mib"
            | "--swappiness" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                let invalid = || CliError::InvalidValue(name.clone());
                let net = &mut options.system.network;
                match name.as_str() {
                    "--tcp-congestion" => net.congestion = value.parse().map_err(|_| invalid())?,
                    "--qdisc" => net.qdisc = value.parse().map_err(|_| invalid())?,
                    "--ipv6" => net.ipv6 = value.parse().map_err(|_| invalid())?,
                    "--address-preference" => {
                        net.address_preference = value.parse().map_err(|_| invalid())?
                    }
                    "--mtu" => net.mtu = Some(value.parse().map_err(|_| invalid())?),
                    "--tcp-buffer-mib" => {
                        net.buffer_mib = Some(value.parse().map_err(|_| invalid())?)
                    }
                    "--somaxconn" => net.somaxconn = Some(value.parse().map_err(|_| invalid())?),
                    "--syn-backlog" => {
                        net.syn_backlog = Some(value.parse().map_err(|_| invalid())?)
                    }
                    "--netdev-backlog" => {
                        net.netdev_backlog = Some(value.parse().map_err(|_| invalid())?)
                    }
                    "--tcp-mtu-probing" => {
                        net.mtu_probing = Some(value.parse().map_err(|_| invalid())?)
                    }
                    "--tcp-fastopen" => net.fastopen = Some(value.parse().map_err(|_| invalid())?),
                    "--tcp-syncookies" => {
                        net.syncookies = Some(match value.as_str() {
                            "0" => false,
                            "1" => true,
                            _ => return Err(invalid()),
                        })
                    }
                    "--swapfile-mib" => {
                        options.system.swap.size_mib = Some(value.parse().map_err(|_| invalid())?)
                    }
                    "--swappiness" => {
                        options.system.swap.swappiness = Some(value.parse().map_err(|_| invalid())?)
                    }
                    _ => unreachable!(),
                }
                index += consumed;
            }
            "--minimal" | "--with-tools" | "--keep-docs" | "--keep-package-cache" => {
                reject_inline(&name, inline_value.as_deref())?;
                match name.as_str() {
                    "--minimal" => options.system.profile = InstallProfile::Minimal,
                    "--with-tools" => options.system.with_tools = true,
                    "--keep-docs" => options.system.keep_docs = true,
                    "--keep-package-cache" => options.system.keep_package_cache = true,
                    _ => unreachable!(),
                }
                index += 1;
            }
            "--profile" | "--dns-mode" | "--dns" | "--dns-search" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                let invalid = || CliError::InvalidValue(name.clone());
                match name.as_str() {
                    "--profile" => options.system.profile = value.parse().map_err(|_| invalid())?,
                    "--dns-mode" => {
                        dns_mode = Some(value.parse::<DnsMode>().map_err(|_| invalid())?)
                    }
                    "--dns" => {
                        for address in value.split(',') {
                            options
                                .system
                                .dns
                                .servers
                                .push(address.trim().parse().map_err(|_| invalid())?);
                        }
                    }
                    "--dns-search" => {
                        for domain in value.split(',') {
                            let domain = domain.trim();
                            if domain.is_empty() {
                                return Err(invalid());
                            }
                            options.system.dns.search_domains.push(domain.to_owned());
                        }
                    }
                    _ => unreachable!(),
                }
                index += consumed;
            }
            "-x" | "--debug" => {
                reject_inline(&name, inline_value.as_deref())?;
                options.debug = true;
                index += 1;
            }
            "--no-cloud-kernel" => {
                reject_inline(&name, inline_value.as_deref())?;
                options.no_cloud_kernel = true;
                index += 1;
            }
            "--kernel" | "--hostname" | "--timezone" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                match name.as_str() {
                    "--kernel" => {
                        options.post_install.kernel = value
                            .parse()
                            .map_err(|_| CliError::InvalidValue(name.clone()))?
                    }
                    "--hostname" => options.post_install.hostname = Some(value),
                    "--timezone" => options.post_install.timezone = Some(value),
                    _ => unreachable!(),
                }
                index += consumed;
            }
            "--confhome" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                confhome = Some(value);
                index += consumed;
            }
            "--hold" | "--sleep" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                options.hold = Some(
                    value
                        .parse::<u8>()
                        .map_err(|_| CliError::InvalidValue(name.clone()))?,
                );
                index += consumed;
            }
            "--user" | "--username" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                options.username = Some(value.trim().to_owned());
                index += consumed;
            }
            "--passwd" | "--password" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                options.password = Some(crate::model::SecretString::new(value));
                index += consumed;
            }
            "--ssh-key" | "--public-key" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                options.ssh_keys.push(value);
                index += consumed;
            }
            "--ssh-port" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                options.ssh_port = Some(parse_port(&value, &name)?);
                index += consumed;
            }
            "--web-port" | "--http-port" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                options.web_port = Some(parse_port(&value, &name)?);
                index += consumed;
            }
            "--frpc-conf" | "--frpc-config" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                options.frpc_config = Some(value);
                index += consumed;
            }
            "--target-disk" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                options.target_disk = Some(value);
                index += consumed;
            }
            "--force-boot-mode" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                options.force_boot_mode = Some(match value.as_str() {
                    "bios" => BootMode::Bios,
                    "efi" => BootMode::Efi,
                    _ => return Err(CliError::InvalidValue(name)),
                });
                index += consumed;
            }
            "--iso" | "--iso-config" | "--iso-kernel" | "--iso-initrd" | "--iso-cmdline" => {
                let (value, consumed) = take_value(&args, index, &name, inline_value)?;
                has_iso_field = true;
                match name.as_str() {
                    "--iso" => iso_fields.path = value,
                    "--iso-config" => iso_fields.config = Some(value),
                    "--iso-kernel" => iso_fields.kernel = Some(value),
                    "--iso-initrd" => iso_fields.initrd = Some(value),
                    "--iso-cmdline" => iso_fields.cmdline = Some(value),
                    _ => unreachable!(),
                }
                index += consumed;
            }
            "-h" | "--help" => return Ok(CliOutcome::Help),
            _ if arg.starts_with('-') => return Err(CliError::UnknownOption(arg.clone())),
            _ => {
                set_target(&mut target, arg)?;
                index += 1;
            }
        }
    }

    let target = target.ok_or(CliError::MissingTarget)?;
    let mut plan = InstallPlan::new(target);
    plan.username = options.username;
    plan.password = options.password;
    plan.ssh_keys = options.ssh_keys;
    plan.ssh_port = options.ssh_port.or(plan.ssh_port);
    plan.web_port = options.web_port.or(plan.web_port);
    plan.frpc_config = options.frpc_config;
    plan.target_disk = options.target_disk;
    plan.no_cloud_kernel = options.no_cloud_kernel;
    plan.post_install = options.post_install;
    plan.system = options.system;
    plan.system.dns.mode = dns_mode.unwrap_or({
        if plan.system.dns.servers.is_empty() && plan.system.dns.search_domains.is_empty() {
            DnsMode::Auto
        } else {
            DnsMode::Static
        }
    });
    plan.hold = options.hold;
    plan.force_boot_mode = options.force_boot_mode;
    plan.confhome = confhome;
    plan.debug = options.debug;
    if has_iso_field {
        plan.iso = Some(iso_fields);
    }
    if plan.target == Target::LocalIso
        && let Some(iso) = plan.iso.as_mut()
        && iso.config.is_none()
        && iso.kernel.is_none()
        && iso.initrd.is_none()
    {
        iso.config = Some("/boot/grub/loopback.cfg".to_owned());
    }
    plan.validate().map_err(CliError::InvalidPlan)?;
    if (inspect || confirmation.is_some()) && !plan.target.is_automatic() {
        return Err(CliError::InvalidValue(
            "--inspect/--confirm-erase require automatic installation".into(),
        ));
    }
    if inspect && confirmation.is_some() {
        return Err(CliError::InvalidValue(
            "--inspect and --confirm-erase are mutually exclusive".into(),
        ));
    }
    if inspect {
        return Ok(CliOutcome::Inspect(Box::new(plan)));
    }
    if let Some(confirmation) = confirmation {
        return Ok(CliOutcome::ConfirmedPlan {
            plan: Box::new(plan),
            confirmation,
        });
    }
    Ok(CliOutcome::Plan(Box::new(plan)))
}

fn set_target(target: &mut Option<Target>, input: &str) -> Result<(), CliError> {
    if target.is_some() {
        return Err(CliError::MultipleTargets);
    }
    *target = Some(match input.to_ascii_lowercase().as_str() {
        "alpine" => Target::Alpine,
        "debian" => Target::Debian,
        "arch" => Target::Arch,
        "gentoo" => Target::Gentoo,
        "iso" => Target::LocalIso,
        "netboot.xyz" => Target::NetbootXyz,
        "reset" => Target::Reset,
        _ => return Err(CliError::InvalidTarget(input.to_owned())),
    });
    Ok(())
}

fn reject_inline(name: &str, inline: Option<&str>) -> Result<(), CliError> {
    if inline.is_some() {
        Err(CliError::InvalidValue(name.to_owned()))
    } else {
        Ok(())
    }
}

fn take_value(
    args: &[String],
    index: usize,
    name: &str,
    inline: Option<String>,
) -> Result<(String, usize), CliError> {
    match inline {
        Some(value) if !value.is_empty() => Ok((value, 1)),
        Some(_) => Err(CliError::MissingValue(name.to_owned())),
        None => args
            .get(index + 1)
            .filter(|value| !value.is_empty())
            .map(|value| (value.clone(), 2))
            .ok_or_else(|| CliError::MissingValue(name.to_owned())),
    }
}

fn parse_port(value: &str, name: &str) -> Result<u16, CliError> {
    let port = value
        .parse::<u16>()
        .map_err(|_| CliError::InvalidValue(name.to_owned()))?;
    if port == 0 {
        return Err(CliError::InvalidValue(name.to_owned()));
    }
    Ok(port)
}
