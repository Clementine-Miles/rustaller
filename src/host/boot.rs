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
    InvalidBootPath,
    InvalidBootCommand,
    UnsafeValue(&'static str),
}

impl fmt::Display for BootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAlpineRepository => f.write_str("Invalid Alpine Live repository URL"),
            Self::InvalidModloopPath => f.write_str("Invalid Alpine Live modloop path"),
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
    let mut name = format!("rustaller ({distro}");
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
            && repo.bytes().all(is_path_byte)
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

    let mut config = String::from("### BEGIN rustaller ###\n");
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
    config.push_str("}\n### END rustaller ###\n");
    Ok(config)
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
