use crate::model::InstallPlan;
use std::collections::BTreeMap;
use std::fmt;

const TTY_CMDLINE: &str = "console=ttyS0,115200n8 console=tty0";
const GRUBENV_HELPER: &str = concat!(
    "if ! [ -s $prefix/grubenv ]; then\n",
    "    for dir in /boot/grub /boot/grub2 /grub /grub2; do\n",
    "        set grubenv=\"($root)$dir/grubenv\"\n",
    "        if [ -s $grubenv ]; then\n",
    "            load_env --file $grubenv\n",
    "            if [ \"${next_entry}\" ]; then\n",
    "                set default=\"${next_entry}\"\n",
    "                set next_entry=\n",
    "                save_env --file $grubenv next_entry\n",
    "            else\n",
    "                set default=\"0\"\n",
    "            fi\n",
    "            return\n",
    "        fi\n",
    "    done\n",
    "fi\n"
);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BootExtra {
    pub main_disk: Option<String>,
    pub web_path: Option<String>,
    pub finalos_mirrorlist: Option<String>,
    pub nextos_mirrorlist: Option<String>,
    pub link_grub_dir: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrubBootEntry {
    pub name: String,
    pub kernel_path: String,
    pub initrd_path: Option<String>,
    pub linux_command: String,
    pub initrd_command: String,
    pub cmdline: String,
    pub btrfs_root: bool,
    pub include_grubenv_helper: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootError {
    InvalidAlpineRepository,
    InvalidModloopPath,
    InvalidFinalOsKey,
    InvalidBootPath,
    InvalidBootCommand,
    UnsafeValue(&'static str),
}

impl fmt::Display for BootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAlpineRepository => f.write_str("Invalid Alpine Live repository URL"),
            Self::InvalidModloopPath => f.write_str("Invalid Alpine Live modloop path"),
            Self::InvalidFinalOsKey => f.write_str("Invalid final OS kernel argument key"),
            Self::InvalidBootPath => f.write_str("Invalid path in GRUB boot entry"),
            Self::InvalidBootCommand => f.write_str("Invalid GRUB kernel/initrd command"),
            Self::UnsafeValue(field) => {
                write!(f, "Cannot safely encode {field} in a GRUB command line")
            }
        }
    }
}

impl std::error::Error for BootError {}

pub fn entry_name(distro: &str, release: &str, hold: Option<u8>) -> String {
    let mut name = format!("reinstall ({distro}");
    if !release.is_empty() {
        name.push(' ');
        name.push_str(release);
    }
    if distro == "alpine" && hold == Some(1) {
        name.push_str(" Live OS");
    }
    name.push(')');
    name
}

pub fn build_nextos_cmdline(repo: &str, modloop: &str) -> Result<String, BootError> {
    let valid_repo = repo.strip_prefix("https://").is_some_and(|remainder| {
        let host = remainder.split('/').next().unwrap_or_default();
        !host.is_empty()
            && host.bytes().any(|byte| byte.is_ascii_alphanumeric())
            && repo.bytes().all(is_url_byte)
    });
    if !valid_repo {
        return Err(BootError::InvalidAlpineRepository);
    }
    if !modloop.starts_with('/')
        || !modloop.bytes().all(is_path_byte)
        || modloop.split('/').any(|segment| segment == "..")
    {
        return Err(BootError::InvalidModloopPath);
    }
    Ok(format!(
        "alpine_repo={repo} modloop={modloop} {TTY_CMDLINE}"
    ))
}

pub fn build_finalos_cmdline(fields: &BTreeMap<String, String>) -> Result<String, BootError> {
    let mut cmdline = String::new();
    for (key, value) in fields {
        if key == "mirrorlist" || value.is_empty() {
            continue;
        }
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(BootError::InvalidFinalOsKey);
        }
        append_value(&mut cmdline, "finalos", key, value)?;
    }
    Ok(cmdline)
}

pub fn build_extra_cmdline(plan: &InstallPlan, extra: &BootExtra) -> Result<String, BootError> {
    let mut cmdline = String::new();
    if let Some(value) = plan.confhome.as_deref() {
        append_named_value(&mut cmdline, "extra_confhome", value)?;
    }
    if let Some(hold) = plan.hold {
        append_named_value(&mut cmdline, "extra_hold", &hold.to_string())?;
    }
    if let Some(mode) = plan.force_boot_mode {
        append_named_value(&mut cmdline, "extra_force_boot_mode", mode.as_str())?;
    }
    if plan.no_cloud_kernel {
        append_named_value(&mut cmdline, "extra_no_cloud_kernel", "1")?;
    }
    if let Some(value) = extra.main_disk.as_deref() {
        append_named_value(&mut cmdline, "extra_main_disk", value)?;
    }
    if let Some(value) = plan.username.as_deref() {
        append_named_value(&mut cmdline, "extra_username", value)?;
    }
    if let Some(port) = plan.ssh_port {
        append_named_value(&mut cmdline, "extra_ssh_port", &port.to_string())?;
    }
    if let Some(port) = plan.web_port {
        append_named_value(&mut cmdline, "extra_web_port", &port.to_string())?;
    }
    if let Some(value) = extra.web_path.as_deref() {
        append_named_value(&mut cmdline, "extra_web_path", value)?;
    }
    if let Some(mirrorlist) = extra
        .finalos_mirrorlist
        .as_deref()
        .or(extra.nextos_mirrorlist.as_deref())
    {
        validate_value("mirrorlist", mirrorlist)?;
        cmdline.push_str(" extra_mirrorlist='");
        cmdline.push_str(mirrorlist);
        cmdline.push('\'');
    }
    if extra.link_grub_dir {
        cmdline.push_str(" extra_link_grub_dir=1");
    }
    Ok(cmdline)
}

pub fn build_cmdline(nextos: &str, finalos: &str, extra: &str) -> String {
    format!("{nextos} {finalos} {extra}")
}

pub fn render_grub_config(entry: &GrubBootEntry) -> Result<String, BootError> {
    if !valid_grub_path(&entry.kernel_path)
        || entry
            .initrd_path
            .as_deref()
            .is_some_and(|path| !valid_grub_path(path))
    {
        return Err(BootError::InvalidBootPath);
    }
    if !matches!(entry.linux_command.as_str(), "linux" | "linux16" | "LINUX")
        || !matches!(
            entry.initrd_command.as_str(),
            "initrd" | "initrd16" | "INITRD"
        )
    {
        return Err(BootError::InvalidBootCommand);
    }
    validate_grub_value("entry name", &entry.name)?;
    validate_grub_value("kernel command line", &entry.cmdline)?;

    let mut config = String::from("### BEGIN reinstall.sh ###\n");
    if entry.include_grubenv_helper {
        config.push_str(GRUBENV_HELPER);
    }
    config.push_str("set timeout_style=menu\nset timeout=5\n");
    config.push_str(&format!(
        "menuentry \"{}\" --unrestricted {{\n    insmod lvm\n",
        entry.name
    ));
    if entry.btrfs_root {
        config.push_str("    set btrfs_relative_path=n\n");
    }
    config.push_str("    insmod all_video\n");
    config.push_str(&format!(
        "    search --no-floppy --file --set=root {}\n",
        entry.kernel_path
    ));
    config.push_str(&format!(
        "    {} {}",
        entry.linux_command, entry.kernel_path
    ));
    if !entry.cmdline.is_empty() {
        config.push(' ');
        config.push_str(&entry.cmdline);
    }
    config.push('\n');
    if let Some(initrd_path) = entry.initrd_path.as_deref() {
        config.push_str(&format!("    {} {}\n", entry.initrd_command, initrd_path));
    }
    config.push_str("}\n### END reinstall.sh ###\n");
    Ok(config)
}

fn append_value(
    output: &mut String,
    prefix: &str,
    key: &str,
    value: &str,
) -> Result<(), BootError> {
    let name = format!("{prefix}_{key}");
    append_named_value(output, &name, value)
}

fn append_named_value(output: &mut String, name: &str, value: &str) -> Result<(), BootError> {
    validate_value("kernel argument", value)?;
    output.push(' ');
    output.push_str(name);
    output.push('=');
    if value.contains(' ') || value.contains('&') || value.starts_with("http") {
        output.push('\'');
        output.push_str(value);
        output.push('\'');
    } else {
        output.push_str(value);
    }
    Ok(())
}

fn validate_value(field: &'static str, value: &str) -> Result<(), BootError> {
    if value
        .bytes()
        .any(|byte| byte == b'\'' || byte == b'\n' || byte == b'\r' || byte == 0)
    {
        Err(BootError::UnsafeValue(field))
    } else {
        Ok(())
    }
}

fn is_url_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"._~:/-".contains(&byte)
}

fn is_path_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"._~:/-".contains(&byte)
}

fn valid_grub_path(path: &str) -> bool {
    path.starts_with('/')
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
        && !path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
}

fn validate_grub_value(field: &'static str, value: &str) -> Result<(), BootError> {
    if value.bytes().any(|byte| {
        byte == b'\n'
            || byte == b'\r'
            || byte == b'\0'
            || byte == b';'
            || byte == b'{'
            || byte == b'}'
            || byte == b'"'
            || byte == b'\\'
    }) {
        Err(BootError::UnsafeValue(field))
    } else {
        Ok(())
    }
}
