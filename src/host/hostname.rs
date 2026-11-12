//! Resolve new-system hostnames before leaving the source environment.
use crate::model::{InstallPlan, Target};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Preserve a usable source hostname. FQDNs contribute their first label because
/// installation plans accept a single hostname label, not a DNS domain.
pub fn detect_hostname(root: &Path) -> Option<String> {
    for relative in ["etc/hostname", "proc/sys/kernel/hostname"] {
        let mut text = String::new();
        let Ok(file) = File::open(root.join(relative)) else {
            continue;
        };
        if !file.metadata().is_ok_and(|metadata| metadata.is_file())
            || file.take(257).read_to_string(&mut text).is_err()
            || text.len() > 256
        {
            continue;
        }
        let value = text.trim();
        if value.is_empty() || value.chars().any(char::is_whitespace) {
            continue;
        }
        // Validate the whole FQDN, rather than salvaging an unsafe first label.
        if !value.split('.').all(valid_label) {
            continue;
        }
        let label = value.split('.').next().unwrap_or_default();
        if ["localhost", "localhost6", "none"]
            .iter()
            .any(|placeholder| label.eq_ignore_ascii_case(placeholder))
        {
            continue;
        }
        return Some(label.to_owned());
    }
    None
}

fn valid_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

pub fn default_hostname(target: Target, root: &Path) -> String {
    detect_hostname(root).unwrap_or_else(|| target.to_string().to_ascii_lowercase())
}

pub fn resolve_install_hostname(plan: &mut InstallPlan, root: &Path) {
    if plan.target.is_automatic() && plan.hold != Some(1) && plan.post_install.hostname.is_none() {
        plan.post_install.hostname = Some(default_hostname(plan.target, root));
    }
}
