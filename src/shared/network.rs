//! Stage native target configuration. Never change the running host's network.
use super::configure::{ConfigurationError, validate_root};
use super::dns::{
    ConfigFile, networkd_files, publish_files, read_config, replace_managed_block,
    validate_planned_file,
};
use crate::config::{AddressPreference, CongestionControl, NetworkConfig, QueueDiscipline};
use crate::model::Target;
use crate::worker::live::ValidatedLivePlan;
use std::path::Path;

const BEGIN: &str = "# BEGIN rust-aller target network";
const END: &str = "# END rust-aller target network";

/// The future target adapter must enable these native OpenRC boot services.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct NetworkServices {
    pub openrc_boot: Vec<&'static str>,
}

/// `installed_kernel_config` must belong to the kernel installed in `root`, not
/// uname's host kernel. Verify module payloads before invoking the final driver.
pub fn apply_network_configuration(
    root: &Path,
    plan: &ValidatedLivePlan,
    installed_kernel_config: &str,
) -> Result<NetworkServices, ConfigurationError> {
    let plan = plan.plan();
    if plan.hold == Some(1) {
        return Err(ConfigurationError::RescuePlan);
    }
    plan.system
        .validate(plan.target, plan.hold)
        .map_err(ConfigurationError::SystemConfig)?;
    validate_root(root)?;
    let net = &plan.system.network;
    let modules = required_modules(net, installed_kernel_config)?;
    let mut files = Vec::new();
    let sysctl = net.sysctl();
    if !sysctl.is_empty() {
        stage(
            root,
            "etc/sysctl.d/90-rust-aller-network.conf",
            format!("# rust-aller target-only network policy\n{sysctl}"),
            &mut files,
        )?;
    }
    if !modules.is_empty() {
        let path = if plan.target == Target::Alpine {
            "etc/modules"
        } else {
            "etc/modules-load.d/90-rust-aller-network.conf"
        };
        let full = root.join(path);
        validate_planned_file(&full)?;
        let body = format!("{}\n", modules.join("\n"));
        stage(root, path, block(&read_config(&full)?, &body)?, &mut files)?;
    }
    if net.address_preference == AddressPreference::Ipv4 {
        let path = root.join("etc/gai.conf");
        validate_planned_file(&path)?;
        let clean = block(&read_config(&path)?, "")?;
        if clean
            .lines()
            .any(|line| line.split_whitespace().next() == Some("precedence"))
        {
            return Err(ConfigurationError::InvalidConfigFile);
        }
        // glibc replaces its default precedence table once any rule is provided.
        let body = "precedence ::1/128 50\nprecedence ::/0 40\nprecedence 2002::/16 30\nprecedence ::/96 20\nprecedence ::ffff:0:0/96 100\n";
        stage(root, "etc/gai.conf", block(&clean, body)?, &mut files)?;
    }
    if let Some(mtu) = net.mtu {
        if plan.target == Target::Alpine {
            let path = root.join("etc/network/interfaces");
            validate_planned_file(&path)?;
            let contents = alpine_mtu(&read_config(&path)?, mtu)?;
            stage(root, "etc/network/interfaces", contents, &mut files)?;
        } else {
            for path in networkd_files(root)? {
                let existing = read_config(&path)?;
                if existing.lines().any(|line| {
                    line.split_once('=').is_some_and(|(key, value)| {
                        matches!(key.trim(), "Name" | "Type")
                            && value
                                .split_whitespace()
                                .any(|value| matches!(value.trim_matches('"'), "lo" | "loopback"))
                    })
                }) {
                    continue;
                }
                let contents = block(&existing, &format!("[Link]\nMTUBytes={mtu}\n"))?;
                files.push(ConfigFile {
                    path,
                    contents: contents.into_bytes(),
                });
            }
        }
    }
    let services = NetworkServices {
        openrc_boot: if plan.target == Target::Alpine && !sysctl.is_empty() {
            let mut services = vec!["sysctl"];
            if !modules.is_empty() {
                services.push("modules");
            }
            services
        } else {
            vec![]
        },
    };
    let links = super::configure::prepare_openrc(root, &services.openrc_boot)?;
    // Check every path/config/kernel choice/service before publishing any change.
    publish_files(files)?;
    super::configure::publish_openrc(links)?;
    Ok(services)
}

pub fn required_modules(
    net: &NetworkConfig,
    kernel_config: &str,
) -> Result<Vec<&'static str>, ConfigurationError> {
    let mut modules = Vec::new();
    let mut require = |symbol: &str, module| {
        let mut states = kernel_config
            .lines()
            .filter_map(|line| line.split_once('='))
            .filter(|(key, _)| *key == symbol)
            .map(|(_, value)| value);
        match (states.next(), states.next()) {
            (Some("y"), None) => Ok(()),
            (Some("m"), None) => {
                modules.push(module);
                Ok(())
            }
            _ => Err(ConfigurationError::UnsupportedNetworkKernel),
        }
    };
    match net.congestion {
        CongestionControl::Bbr => require("CONFIG_TCP_CONG_BBR", "tcp_bbr")?,
        CongestionControl::Cubic => require("CONFIG_TCP_CONG_CUBIC", "tcp_cubic")?,
        CongestionControl::Default => {}
    }
    match net.qdisc {
        QueueDiscipline::Fq => require("CONFIG_NET_SCH_FQ", "sch_fq")?,
        QueueDiscipline::FqCodel => require("CONFIG_NET_SCH_FQ_CODEL", "sch_fq_codel")?,
        QueueDiscipline::Default => {}
    }
    Ok(modules)
}

fn stage(
    root: &Path,
    name: &str,
    text: String,
    files: &mut Vec<ConfigFile>,
) -> Result<(), ConfigurationError> {
    let path = root.join(name);
    validate_planned_file(&path)?;
    files.push(ConfigFile {
        path,
        contents: text.into_bytes(),
    });
    Ok(())
}
fn block(existing: &str, body: &str) -> Result<String, ConfigurationError> {
    replace_managed_block(existing, body, BEGIN, END, false)
}

/// Only the generated plain ifupdown stanzas are supported, including dual stack.
pub fn alpine_mtu(existing: &str, mtu: u16) -> Result<String, ConfigurationError> {
    if !(1280..=9000).contains(&mtu) {
        return Err(ConfigurationError::InvalidConfigFile);
    }
    let clean = block(existing, "")?;
    let mut result = String::new();
    let mut active = false;
    let mut found = false;
    for line in clean.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.first() == Some(&"iface") {
            active =
                fields.len() == 4 && fields[1] != "lo" && matches!(fields[2], "inet" | "inet6");
            result.push_str(line);
            result.push('\n');
            if active {
                result.push_str(&format!("    mtu {mtu}\n"));
                found = true;
            }
            continue;
        }
        if !line.starts_with(char::is_whitespace)
            && !line.trim().is_empty()
            && !line.trim_start().starts_with('#')
        {
            active = false;
        }
        if active && fields.first() == Some(&"mtu") {
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    if !found {
        return Err(ConfigurationError::MissingNetworkConfig);
    }
    Ok(result)
}
