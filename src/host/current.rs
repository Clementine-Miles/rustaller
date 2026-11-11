//! Explicit configuration of the running system, independent of installation.
//!
//! Only `apply_current` may select `/`. Tests use an isolated root and injected
//! facts/commands. File publication uses directory descriptors and O_NOFOLLOW;
//! SSH account homes do not turn a root write into a symlink-following operation.
//! No service is restarted, no sysctl is applied, and no disk/swap/kernel action
//! is available in this mode. Backups are complete before configuration writes.
use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use crate::config::{AddressPreference, Ipv6Policy, NetworkConfig};
use crate::config::{DnsConfig, DnsMode, SystemConfig};
use crate::model::{Target, validate_host_arch};
use crate::postinstall::PostInstallConfig;
use crate::shared::dns::replace_managed_block;
use crate::shared::network::{alpine_mtu, required_modules};
use std::ffi::{CString, OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const LIMIT: usize = 1024 * 1024;
const BEGIN: &str = "# BEGIN rustaller current system";
const END: &str = "# END rustaller current system";
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Default, Eq, PartialEq)]
pub struct CurrentConfig {
    pub hostname: Option<String>,
    pub timezone: Option<String>,
    pub dns: DnsConfig,
    pub network: NetworkConfig,
    /// Existing account only; defaults to root when keys are explicitly added.
    pub username: Option<String>,
    pub ssh_port: Option<u16>,
    /// Inline public keys or absolute local public-key files. No remote fetches.
    pub ssh_keys: Vec<String>,
}
impl fmt::Debug for CurrentConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CurrentConfig")
            .field("hostname", &self.hostname)
            .field("timezone", &self.timezone)
            .field("dns", &self.dns)
            .field("network", &self.network)
            .field("username", &self.username)
            .field("ssh_port", &self.ssh_port)
            .field(
                "ssh_keys",
                &format_args!("[{} public-key sources hidden]", self.ssh_keys.len()),
            )
            .finish()
    }
}
impl CurrentConfig {
    pub fn is_empty(&self) -> bool {
        self.hostname.is_none()
            && self.timezone.is_none()
            && self.dns == DnsConfig::default()
            && self.network == NetworkConfig::default()
            && self.ssh_port.is_none()
            && self.ssh_keys.is_empty()
    }
    pub fn validate(&self, target: Target) -> Result<(), CurrentError> {
        if !target.is_automatic() {
            return Err(CurrentError::UnsupportedDistribution);
        }
        PostInstallConfig {
            hostname: self.hostname.clone(),
            timezone: self.timezone.clone(),
            ..Default::default()
        }
        .validate(target, false, None)
        .map_err(|e| CurrentError::InvalidConfiguration(e.to_string()))?;
        SystemConfig {
            dns: self.dns.clone(),
            network: self.network.clone(),
            ..Default::default()
        }
        .validate(target, None)
        .map_err(|e| CurrentError::InvalidConfiguration(e.to_string()))?;
        if self.ssh_port == Some(0) {
            return Err(CurrentError::InvalidConfiguration(
                "SSH port must be 1-65535".into(),
            ));
        }
        if self
            .username
            .as_deref()
            .is_some_and(|name| !valid_username(name))
            || self.ssh_keys.len() > 128
            || self.ssh_keys.iter().any(|key| {
                key.is_empty()
                    || key.len() > 8192
                    || key.chars().any(char::is_control)
                    || (!key.starts_with('/') && !looks_like_key(key))
            })
        {
            return Err(CurrentError::InvalidConfiguration("Use an existing valid username and at most 128 inline or absolute local public-key sources".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CurrentFacts {
    pub target: Target,
    pub architecture: String,
    pub effective_uid: u32,
    pub kernel_config: String,
    /// A global IPv4 address and IPv4 default route have both been observed.
    pub ipv4_available: bool,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct CurrentReport {
    /// Relative paths, never private-key or password contents.
    pub changed_files: Vec<PathBuf>,
    pub backup_dir: Option<PathBuf>,
    pub notes: Vec<String>,
}
#[derive(Debug)]
pub enum CurrentError {
    RootRequired,
    UnsupportedArchitecture,
    UnsupportedDistribution,
    InvalidRoot,
    UnsafePath,
    UnsupportedNetworkManager,
    Ipv4Required,
    InvalidAccount,
    InvalidSshKey,
    SshValidationFailed,
    ConflictingSshConfiguration,
    ConcurrentChange,
    InvalidConfiguration(String),
    Filesystem(io::Error),
    CommandFailed(&'static str),
    Publication { backup_dir: PathBuf, path: PathBuf },
}
impl fmt::Display for CurrentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootRequired=>f.write_str("Applying current-system settings requires root"),
            Self::UnsupportedArchitecture=>f.write_str("Current-system configuration supports Linux x86_64 only"),
            Self::UnsupportedDistribution=>f.write_str("Current-system configuration requires Alpine, Debian-compatible, Arch or Gentoo"),
            Self::InvalidRoot=>f.write_str("The isolated current-system test root must be a real absolute directory other than /"),
            Self::UnsafePath=>f.write_str("Current-system configuration encountered an unsafe file, directory or symlink"),
            Self::UnsupportedNetworkManager=>f.write_str("DNS/MTU persistence requires supported native networkd files or Alpine networking; generated netplan/NetworkManager files are not edited"),
            Self::Ipv4Required=>f.write_str("Disabling IPv6 requires an observed global IPv4 address and IPv4 default route"),
            Self::InvalidAccount=>f.write_str("SSH keys require one existing login account with a real home directory and standard authorized_keys configuration"),
            Self::InvalidSshKey=>f.write_str("Public keys must be valid OpenSSH keys or safe local public-key files; private keys and remote sources are rejected"),
            Self::SshValidationFailed=>f.write_str("The system sshd/ssh-keygen tool rejected the proposed SSH configuration; no configuration was published"),
            Self::ConflictingSshConfiguration=>f.write_str("Included SSH settings conflict with the requested port, key file or account authentication policy"),
            Self::ConcurrentChange=>f.write_str("A configuration file changed during review; no configuration was published. Review the current files and retry"),
            Self::InvalidConfiguration(e)=>f.write_str(e),
            Self::Filesystem(e)=>write!(f,"Cannot prepare current-system configuration: {e}"),
            Self::CommandFailed(program)=>write!(f,"Cannot inspect the current system with {program}"),
            Self::Publication {backup_dir,path}=>write!(f,"Configuration publication failed at {}; previous files are backed up in {}. Some earlier files may already be updated",path.display(),backup_dir.display()),
        }
    }
}
impl std::error::Error for CurrentError {}
impl From<io::Error> for CurrentError {
    fn from(e: io::Error) -> Self {
        Self::Filesystem(e)
    }
}

/// Read only. ID_LIKE=debian selects Debian-compatible configuration semantics.
pub fn detect_current_target() -> Result<Target, CurrentError> {
    detect_target_at(Path::new("/"))
}
pub fn detect_target_at(root: &Path) -> Result<Target, CurrentError> {
    let directory = RootDirectory::open(root)?;
    let text = match directory.snapshot(Path::new("etc/os-release"), true)? {
        Snapshot::Symlink(target) => {
            directory.text(&resolve_inside_root(Path::new("etc"), &target)?)?
        }
        _ => directory.text(Path::new("etc/os-release"))?,
    };
    let value = |key: &str| {
        text.lines()
            .filter_map(|line| line.split_once('='))
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.trim().trim_matches(['\'', '"']))
    };
    match value("ID") {
        Some("alpine") => Ok(Target::Alpine),
        Some("debian") => Ok(Target::Debian),
        Some("arch") => Ok(Target::Arch),
        Some("gentoo") => Ok(Target::Gentoo),
        _ if value("ID_LIKE").is_some_and(|v| v.split_whitespace().any(|v| v == "debian")) => {
            Ok(Target::Debian)
        }
        _ => Err(CurrentError::UnsupportedDistribution),
    }
}

/// Explicit live-system entry. The caller must obtain the user's reviewed Apply
/// action; merely opening or saving a TUI draft must never call this function.
pub fn apply_current<R: CommandRunner>(
    runner: &mut R,
    config: &CurrentConfig,
) -> Result<CurrentReport, CurrentError> {
    if std::env::consts::OS != "linux" || std::env::consts::ARCH != "x86_64" {
        return Err(CurrentError::UnsupportedArchitecture);
    }
    let effective_uid = unsafe { geteuid() };
    if effective_uid != 0 {
        return Err(CurrentError::RootRequired);
    }
    let target = detect_current_target()?;
    config.validate(target)?;
    let mut facts = CurrentFacts {
        target,
        architecture: "x86_64".into(),
        effective_uid,
        kernel_config: String::new(),
        ipv4_available: false,
    };
    if config.is_empty() {
        return Ok(CurrentReport::default());
    }
    if config.network.congestion != crate::config::CongestionControl::Default
        || config.network.qdisc != crate::config::QueueDiscipline::Default
    {
        let release = capture(runner, "uname", ["-r"])?;
        if !release
            .trim()
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        {
            return Err(CurrentError::UnsupportedArchitecture);
        }
        let path = PathBuf::from(format!("/boot/config-{}", release.trim()));
        facts.kernel_config = match fs::read_to_string(path) {
            Ok(text) if text.len() <= LIMIT => text,
            _ => capture(runner, "zcat", ["/proc/config.gz"])?,
        };
    }
    if config.network.ipv6 == Ipv6Policy::Disable {
        let address = capture(runner, "ip", ["-4", "address", "show", "scope", "global"])?;
        let route = capture(runner, "ip", ["-4", "route", "show", "default"])?;
        facts.ipv4_available = address
            .lines()
            .any(|line| line.trim_start().starts_with("inet "))
            && route.lines().any(|line| line.starts_with("default "));
    }
    apply_checked(runner, Path::new("/"), config, &facts)
}

/// Isolated-root seam for mocks. Unlike the live entry this always rejects `/`.
pub fn apply_current_at<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    config: &CurrentConfig,
    facts: &CurrentFacts,
) -> Result<CurrentReport, CurrentError> {
    if root == Path::new("/") {
        return Err(CurrentError::InvalidRoot);
    }
    apply_checked(runner, root, config, facts)
}

fn apply_checked<R: CommandRunner>(
    runner: &mut R,
    root: &Path,
    config: &CurrentConfig,
    facts: &CurrentFacts,
) -> Result<CurrentReport, CurrentError> {
    if facts.effective_uid != 0 {
        return Err(CurrentError::RootRequired);
    }
    validate_host_arch(&facts.architecture).map_err(|_| CurrentError::UnsupportedArchitecture)?;
    config.validate(facts.target)?;
    let directory = RootDirectory::open(root)?;
    if config.is_empty() {
        return Ok(CurrentReport::default());
    }
    if config.network.ipv6 == Ipv6Policy::Disable && !facts.ipv4_available {
        return Err(CurrentError::Ipv4Required);
    }
    let mut files = Vec::new();
    if let Some(hostname) = &config.hostname {
        stage(
            &directory,
            "etc/hostname",
            format!("{hostname}\n").into_bytes(),
            None,
            false,
            &mut files,
        )?;
    }
    if let Some(timezone) = &config.timezone {
        let path = format!("usr/share/zoneinfo/{timezone}");
        let bytes = crate::shared::configure::read_target_zonefile(root, &path)
            .map_err(|e| CurrentError::InvalidConfiguration(e.to_string()))?;
        stage(
            &directory,
            "etc/timezone",
            format!("{timezone}\n").into_bytes(),
            None,
            false,
            &mut files,
        )?;
        stage(&directory, "etc/localtime", bytes, None, true, &mut files)?;
    }
    prepare_network(&directory, config, facts, &mut files)?;
    prepare_ssh(runner, &directory, config, &mut files)?;
    files.retain(|file| !file.unchanged());
    if files.is_empty() {
        return Ok(CurrentReport::default());
    }
    for file in &files {
        if directory.snapshot(&file.relative, matches!(file.old, Snapshot::Symlink(_)))? != file.old
        {
            return Err(CurrentError::ConcurrentChange);
        }
    }
    let backup_rel = PathBuf::from(format!(
        "var/backups/rustaller-current/{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    directory.mkdir_exclusive(&backup_rel, 0o700)?;
    let backup_dir = root.join(&backup_rel);
    let mut manifest = String::from(
        "# rustaller current-system backup; regular files stored with private permissions\n",
    );
    for file in &files {
        let backup = backup_rel.join(&file.relative);
        match &file.old {
            Snapshot::Absent => {
                manifest.push_str(&format!("{}\tabsent\n", file.relative.display()))
            }
            Snapshot::Regular {
                bytes,
                mode,
                uid,
                gid,
            } => {
                directory.publish(&backup, bytes, 0o600, None)?;
                manifest.push_str(&format!(
                    "{}\tfile\t{mode:o}\t{uid}\t{gid}\n",
                    file.relative.display()
                ));
            }
            Snapshot::Symlink(target) => {
                directory.publish_link(&backup, target)?;
                manifest.push_str(&format!("{}\tsymlink\n", file.relative.display()));
            }
        }
    }
    directory.publish(
        &backup_rel.join("MANIFEST"),
        manifest.as_bytes(),
        0o600,
        None,
    )?;
    let mut report=CurrentReport{changed_files:Vec::new(),backup_dir:Some(backup_dir.clone()),notes:vec!["Configuration files were updated; no service restart, sysctl apply, kernel change, disk operation or swap activation was performed.".into()]};
    for file in files {
        if directory
            .snapshot(&file.relative, matches!(file.old, Snapshot::Symlink(_)))
            .map_err(|_| CurrentError::Publication {
                backup_dir: backup_dir.clone(),
                path: file.relative.clone(),
            })?
            != file.old
        {
            return Err(CurrentError::Publication {
                backup_dir: backup_dir.clone(),
                path: file.relative,
            });
        }
        directory
            .publish(&file.relative, &file.bytes, file.mode, file.owner)
            .map_err(|_| CurrentError::Publication {
                backup_dir: backup_dir.clone(),
                path: file.relative.clone(),
            })?;
        report.changed_files.push(file.relative);
    }
    if config.dns.mode == DnsMode::Static {
        report.notes.push("Resolver file changes may affect new DNS lookups immediately even though network services were not restarted.".into());
    }
    if !config.ssh_keys.is_empty() || config.ssh_port.is_some() {
        report.notes.push("SSH files passed native validation; existing access restrictions are preserved. Reload SSH manually from a retained working session when ready; existing sessions were not changed. Socket-activated SSH listeners may require separate socket configuration.".into());
    }
    Ok(report)
}

struct PreparedFile {
    relative: PathBuf,
    bytes: Vec<u8>,
    mode: u32,
    owner: Option<(u32, u32)>,
    old: Snapshot,
}
impl PreparedFile {
    fn unchanged(&self) -> bool {
        matches!(&self.old,Snapshot::Regular{bytes,mode,uid,gid} if bytes==&self.bytes && *mode==self.mode && self.owner.is_none_or(|owner|owner==(*uid,*gid)))
    }
}
#[derive(Eq, PartialEq)]
enum Snapshot {
    Absent,
    Regular {
        bytes: Vec<u8>,
        mode: u32,
        uid: u32,
        gid: u32,
    },
    Symlink(PathBuf),
}
fn stage(
    directory: &RootDirectory,
    name: impl AsRef<Path>,
    bytes: Vec<u8>,
    owner: Option<(u32, u32)>,
    allow_link: bool,
    files: &mut Vec<PreparedFile>,
) -> Result<(), CurrentError> {
    let relative = name.as_ref();
    let old = directory.snapshot(relative, allow_link)?;
    let mode = if owner.is_some() {
        0o600
    } else {
        match &old {
            Snapshot::Regular { mode, .. } => *mode,
            _ => 0o644,
        }
    };
    let owner = owner.or(match &old {
        Snapshot::Regular { uid, gid, .. } => Some((*uid, *gid)),
        _ => None,
    });
    // Edits to the same native network file compose instead of overwriting DNS.
    if let Some(previous) = files.iter_mut().find(|file| file.relative == relative) {
        previous.bytes = bytes;
        return Ok(());
    }
    files.push(PreparedFile {
        relative: relative.to_owned(),
        bytes,
        mode,
        owner,
        old,
    });
    Ok(())
}
fn text_for(
    directory: &RootDirectory,
    path: &Path,
    files: &[PreparedFile],
) -> Result<String, CurrentError> {
    if let Some(file) = files.iter().find(|file| file.relative == path) {
        return String::from_utf8(file.bytes.clone()).map_err(|_| CurrentError::UnsafePath);
    }
    directory.text(path)
}
fn block(existing: &str, body: &str) -> Result<String, CurrentError> {
    replace_managed_block(existing, body, BEGIN, END, false)
        .map_err(|e| CurrentError::InvalidConfiguration(e.to_string()))
}

fn prepare_network(
    directory: &RootDirectory,
    config: &CurrentConfig,
    facts: &CurrentFacts,
    files: &mut Vec<PreparedFile>,
) -> Result<(), CurrentError> {
    let net = &config.network;
    let modules = required_modules(net, &facts.kernel_config)
        .map_err(|e| CurrentError::InvalidConfiguration(e.to_string()))?;
    let sysctl = net.sysctl();
    if !sysctl.is_empty() {
        stage(
            directory,
            "etc/sysctl.d/90-rustaller-current-network.conf",
            format!("# rustaller current-system network policy\n{sysctl}").into_bytes(),
            None,
            false,
            files,
        )?;
    }
    if !modules.is_empty() {
        let name = if facts.target == Target::Alpine {
            "etc/modules"
        } else {
            "etc/modules-load.d/90-rustaller-current-network.conf"
        };
        let body = format!("{}\n", modules.join("\n"));
        stage(
            directory,
            name,
            block(&text_for(directory, Path::new(name), files)?, &body)?.into_bytes(),
            None,
            false,
            files,
        )?;
    }
    if net.address_preference == AddressPreference::Ipv4 {
        let clean = block(&text_for(directory, Path::new("etc/gai.conf"), files)?, "")?;
        if clean
            .lines()
            .any(|line| line.split_whitespace().next() == Some("precedence"))
        {
            return Err(CurrentError::InvalidConfiguration("Existing gai.conf precedence rules must be reviewed before selecting IPv4 preference".into()));
        }
        stage(directory,"etc/gai.conf",block(&clean,"precedence ::1/128 50\nprecedence ::/0 40\nprecedence 2002::/16 30\nprecedence ::/96 20\nprecedence ::ffff:0:0/96 100\n")?.into_bytes(),None,false,files)?;
    }
    let network_paths = if (config.dns.mode == DnsMode::Static && facts.target != Target::Alpine)
        || (net.mtu.is_some() && facts.target != Target::Alpine)
    {
        directory.networkd_paths()?
    } else {
        Vec::new()
    };
    if config.dns.mode == DnsMode::Static {
        stage(
            directory,
            "etc/resolv.conf",
            config
                .dns
                .resolv_conf()
                .map_err(|e| CurrentError::InvalidConfiguration(e.to_string()))?
                .expect("static")
                .into_bytes(),
            None,
            true,
            files,
        )?;
        if facts.target == Target::Alpine {
            for (name, body) in [
                ("etc/dhcpcd.conf", "nohook resolv.conf\n"),
                ("etc/udhcpc/udhcpc.conf", "RESOLV_CONF=\"no\"\n"),
            ] {
                let existing = text_for(directory, Path::new(name), files)?;
                let value = replace_managed_block(
                    &existing,
                    body,
                    "# BEGIN rustaller current DNS",
                    "# END rustaller current DNS",
                    true,
                )
                .map_err(|e| CurrentError::InvalidConfiguration(e.to_string()))?;
                stage(directory, name, value.into_bytes(), None, false, files)?;
            }
        } else {
            let body = config
                .dns
                .networkd_settings()
                .map_err(|e| CurrentError::InvalidConfiguration(e.to_string()))?
                .expect("static");
            for path in &network_paths {
                let value = replace_managed_block(
                    &text_for(directory, path, files)?,
                    &body,
                    "# BEGIN rustaller current DNS",
                    "# END rustaller current DNS",
                    false,
                )
                .map_err(|e| CurrentError::InvalidConfiguration(e.to_string()))?;
                stage(directory, path, value.into_bytes(), None, false, files)?;
            }
        }
    }
    if let Some(mtu) = net.mtu {
        if facts.target == Target::Alpine {
            let path = Path::new("etc/network/interfaces");
            let value = alpine_mtu(&text_for(directory, path, files)?, mtu)
                .map_err(|e| CurrentError::InvalidConfiguration(e.to_string()))?;
            stage(directory, path, value.into_bytes(), None, false, files)?;
        } else {
            for path in &network_paths {
                let text = text_for(directory, path, files)?;
                if text.lines().any(|line| {
                    line.split_once('=').is_some_and(|(key, value)| {
                        matches!(key.trim(), "Name" | "Type")
                            && matches!(value.trim(), "lo" | "loopback")
                    })
                }) {
                    continue;
                }
                stage(
                    directory,
                    path,
                    block(&text, &format!("[Link]\nMTUBytes={mtu}\n"))?.into_bytes(),
                    None,
                    false,
                    files,
                )?;
            }
        }
    }
    Ok(())
}

fn prepare_ssh<R: CommandRunner>(
    runner: &mut R,
    directory: &RootDirectory,
    config: &CurrentConfig,
    files: &mut Vec<PreparedFile>,
) -> Result<(), CurrentError> {
    if config.ssh_port.is_none() && config.ssh_keys.is_empty() {
        return Ok(());
    }
    let original = directory.text(Path::new("etc/ssh/sshd_config"))?;
    if original.is_empty() {
        return Err(CurrentError::SshValidationFailed);
    }
    let candidate = if let Some(port) = config.ssh_port {
        let clean = replace_managed_block(
            &original,
            "",
            "# BEGIN rustaller SSH",
            "# END rustaller SSH",
            false,
        )
        .map_err(|_| CurrentError::SshValidationFailed)?;
        let mut matched = false;
        let mut lines = String::new();
        for line in clean.lines() {
            let first = line.split_whitespace().next().unwrap_or("");
            if first.eq_ignore_ascii_case("Match") {
                matched = true;
            }
            if !matched && first.eq_ignore_ascii_case("Port") {
                continue;
            }
            lines.push_str(line);
            lines.push('\n');
        }
        let text = replace_managed_block(
            &lines,
            &format!("Port {port}\n"),
            "# BEGIN rustaller SSH",
            "# END rustaller SSH",
            true,
        )
        .map_err(|_| CurrentError::SshValidationFailed)?;
        stage(
            directory,
            "etc/ssh/sshd_config",
            text.as_bytes().to_vec(),
            None,
            false,
            files,
        )?;
        text
    } else {
        original
    };
    let temporary = PrivateDirectory::new()?;
    let candidate_path = temporary.0.join("sshd_config");
    private_file(&candidate_path, candidate.as_bytes())?;
    let candidate_arg = candidate_path.to_str().ok_or(CurrentError::UnsafePath)?;
    ssh_capture(runner, "sshd", ["-t", "-f", candidate_arg])?;
    let settings = ssh_capture(runner, "sshd", ["-T", "-f", candidate_arg])?;
    if let Some(port) = config.ssh_port {
        let ports = settings
            .lines()
            .filter_map(|line| line.strip_prefix("port "))
            .collect::<Vec<_>>();
        if ports.is_empty()
            || ports.iter().any(|value| value.parse::<u16>() != Ok(port))
            || settings
                .lines()
                .filter_map(|line| line.strip_prefix("listenaddress "))
                .any(|value| {
                    value.rsplit(':').next().and_then(|v| v.parse::<u16>().ok()) != Some(port)
                })
        {
            return Err(CurrentError::ConflictingSshConfiguration);
        }
    }
    if config.ssh_keys.is_empty() {
        return Ok(());
    }
    let name = config.username.as_deref().unwrap_or("root");
    let account = find_account(directory, name)?;
    let context = format!("user={name},host=localhost,addr=127.0.0.1");
    let settings = ssh_capture(runner, "sshd", ["-T", "-C", &context, "-f", candidate_arg])?;
    if !settings
        .lines()
        .any(|line| line == "pubkeyauthentication yes")
        || (account.uid == 0
            && !settings.lines().any(|line| {
                matches!(
                    line,
                    "permitrootlogin yes"
                        | "permitrootlogin prohibit-password"
                        | "permitrootlogin without-password"
                )
            }))
        || !settings
            .lines()
            .filter_map(|line| line.strip_prefix("authorizedkeysfile "))
            .any(|value| {
                value.split_whitespace().any(|file| {
                    file == ".ssh/authorized_keys"
                        || file == "%h/.ssh/authorized_keys"
                        || file == format!("{}/.ssh/authorized_keys", account.home.display())
                })
            })
    {
        return Err(CurrentError::ConflictingSshConfiguration);
    }
    let relative = account
        .home
        .strip_prefix("/")
        .map_err(|_| CurrentError::InvalidAccount)?
        .join(".ssh/authorized_keys");
    directory.validate_account_home(&account.home, account.uid)?;
    let mut existing = directory.text(&relative)?;
    let mut expanded_keys = 0;
    for source in &config.ssh_keys {
        let public = if source.starts_with('/') {
            let path = Path::new(source)
                .strip_prefix("/")
                .map_err(|_| CurrentError::InvalidSshKey)?;
            directory
                .text(path)
                .map_err(|_| CurrentError::InvalidSshKey)?
        } else {
            source.to_owned()
        };
        let mut source_keys = 0;
        for key in public
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        {
            source_keys += 1;
            expanded_keys += 1;
            if expanded_keys > 128 {
                return Err(CurrentError::InvalidSshKey);
            }
            if key.len() > 8192 || !looks_like_key(key) || key.chars().any(char::is_control) {
                return Err(CurrentError::InvalidSshKey);
            }
            let key_path = temporary.0.join("validate.pub");
            // Each source key is individually checked; an invalid later line
            // cannot hide behind an earlier valid key in ssh-keygen output.
            fs::write(&key_path, format!("{key}\n"))?;
            ssh_capture(
                runner,
                "ssh-keygen",
                ["-lf", key_path.to_str().ok_or(CurrentError::UnsafePath)?],
            )?;
            if !existing.lines().any(|line| line == key) {
                if !existing.is_empty() && !existing.ends_with('\n') {
                    existing.push('\n');
                }
                existing.push_str(key);
                existing.push('\n');
            }
            if existing.len() > LIMIT {
                return Err(CurrentError::InvalidSshKey);
            }
        }
        if source_keys == 0 {
            return Err(CurrentError::InvalidSshKey);
        }
    }
    stage(
        directory,
        relative,
        existing.into_bytes(),
        Some((account.uid, account.gid)),
        false,
        files,
    )?;
    Ok(())
}

struct Account {
    uid: u32,
    gid: u32,
    home: PathBuf,
}
fn find_account(directory: &RootDirectory, name: &str) -> Result<Account, CurrentError> {
    let passwd = directory.text(Path::new("etc/passwd"))?;
    let found = passwd
        .lines()
        .map(|line| line.split(':').collect::<Vec<_>>())
        .filter(|fields| fields.first() == Some(&name))
        .collect::<Vec<_>>();
    let [fields] = found.as_slice() else {
        return Err(CurrentError::InvalidAccount);
    };
    if fields.len() != 7
        || matches!(
            fields[6],
            "/sbin/nologin" | "/usr/sbin/nologin" | "/bin/false"
        )
    {
        return Err(CurrentError::InvalidAccount);
    }
    let home = PathBuf::from(fields[5]);
    if !home.is_absolute()
        || home == Path::new("/")
        || home
            .as_os_str()
            .to_string_lossy()
            .chars()
            .any(char::is_control)
        || home
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(CurrentError::InvalidAccount);
    }
    Ok(Account {
        uid: fields[2]
            .parse()
            .map_err(|_| CurrentError::InvalidAccount)?,
        gid: fields[3]
            .parse()
            .map_err(|_| CurrentError::InvalidAccount)?,
        home,
    })
}
fn looks_like_key(text: &str) -> bool {
    let mut fields = text.split_whitespace();
    matches!(
        fields.next(),
        Some(
            "ssh-ed25519"
                | "ssh-rsa"
                | "ecdsa-sha2-nistp256"
                | "ecdsa-sha2-nistp384"
                | "ecdsa-sha2-nistp521"
                | "sk-ssh-ed25519@openssh.com"
                | "sk-ecdsa-sha2-nistp256@openssh.com"
        )
    ) && fields.next().is_some_and(|s| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    })
}
fn valid_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && (value.as_bytes()[0].is_ascii_alphabetic() || value.as_bytes()[0] == b'_')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

fn capture<R: CommandRunner, I, S>(
    runner: &mut R,
    program: &'static str,
    args: I,
) -> Result<String, CurrentError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let spec = CommandSpec::new(program, args)
        .phase("current-system-inspection")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Capture)
        .stderr_mode(OutputMode::Capture);
    let output = runner
        .run(&spec)
        .map_err(|_| CurrentError::CommandFailed(program))?;
    if output.status != Some(0) || output.stdout.len() > LIMIT {
        return Err(CurrentError::CommandFailed(program));
    }
    String::from_utf8(output.stdout).map_err(|_| CurrentError::CommandFailed(program))
}
fn ssh_capture<R: CommandRunner, I, S>(
    runner: &mut R,
    program: &'static str,
    args: I,
) -> Result<String, CurrentError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    capture(runner, program, args).map_err(|_| CurrentError::SshValidationFailed)
}
struct PrivateDirectory(PathBuf);
impl PrivateDirectory {
    fn new() -> Result<Self, CurrentError> {
        let path = std::env::temp_dir().join(format!(
            "rustaller-current-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn private_file(path: &Path, bytes: &[u8]) -> Result<(), CurrentError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

// Linux x86_64 descriptor-based native file operations, with no extra runtime
// dependency. Every ancestor is opened as a real directory with O_NOFOLLOW.
const O_RDONLY: i32 = 0;
const O_WRONLY: i32 = 1;
const O_CREAT: i32 = 0o100;
const O_EXCL: i32 = 0o200;
const O_NONBLOCK: i32 = 0o4000;
const O_DIRECTORY: i32 = 0o200000;
const O_NOFOLLOW: i32 = 0o400000;
const O_CLOEXEC: i32 = 0o2000000;
struct RootDirectory {
    file: File,
}
impl RootDirectory {
    fn open(root: &Path) -> Result<Self, CurrentError> {
        if !root.is_absolute()
            || root
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        {
            return Err(CurrentError::InvalidRoot);
        }
        let mut file = File::open("/")?;
        for part in root.components() {
            if let Component::Normal(name) = part {
                file = open_dir(&file, name, false, None)?;
            }
        }
        Ok(Self { file })
    }
    fn parent(
        &self,
        path: &Path,
        create: bool,
        owner: Option<(u32, u32)>,
    ) -> Result<(File, CString), CurrentError> {
        if path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(CurrentError::UnsafePath);
        }
        let mut parts = path.components().collect::<Vec<_>>();
        let Component::Normal(name) = parts.pop().ok_or(CurrentError::UnsafePath)? else {
            return Err(CurrentError::UnsafePath);
        };
        let mut directory = self.file.try_clone()?;
        for part in parts {
            let Component::Normal(name) = part else {
                return Err(CurrentError::UnsafePath);
            };
            directory = open_dir(&directory, name, create, owner)?;
        }
        Ok((directory, cstring(name)?))
    }
    fn snapshot(&self, path: &Path, allow_link: bool) -> Result<Snapshot, CurrentError> {
        let (parent, name) = match self.parent(path, false, None) {
            Ok(v) => v,
            Err(CurrentError::Filesystem(e)) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Snapshot::Absent);
            }
            Err(e) => return Err(e),
        };
        let fd = unsafe {
            openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK,
                0,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                return Ok(Snapshot::Absent);
            }
            if allow_link && error.raw_os_error() == Some(40) {
                return read_link(&parent, &name).map(Snapshot::Symlink);
            }
            return Err(CurrentError::UnsafePath);
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > LIMIT as u64 {
            return Err(CurrentError::UnsafePath);
        }
        let mut bytes = Vec::new();
        file.take(LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > LIMIT {
            return Err(CurrentError::UnsafePath);
        }
        Ok(Snapshot::Regular {
            bytes,
            mode: metadata.permissions().mode() & 0o777,
            uid: metadata.uid(),
            gid: metadata.gid(),
        })
    }
    fn text(&self, path: &Path) -> Result<String, CurrentError> {
        match self.snapshot(path, false)? {
            Snapshot::Absent => Ok(String::new()),
            Snapshot::Regular { bytes, .. } => {
                let text = String::from_utf8(bytes).map_err(|_| CurrentError::UnsafePath)?;
                if text.contains('\0') {
                    return Err(CurrentError::UnsafePath);
                }
                Ok(text)
            }
            Snapshot::Symlink(_) => Err(CurrentError::UnsafePath),
        }
    }
    fn mkdir_exclusive(&self, path: &Path, mode: u32) -> Result<(), CurrentError> {
        let (parent, name) = self.parent(path, true, None)?;
        let metadata = parent.metadata()?;
        if metadata.uid() != unsafe { geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err(CurrentError::UnsafePath);
        }
        if unsafe { mkdirat(parent.as_raw_fd(), name.as_ptr(), mode) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
    fn publish(
        &self,
        path: &Path,
        bytes: &[u8],
        mode: u32,
        owner: Option<(u32, u32)>,
    ) -> Result<(), CurrentError> {
        let (parent, name) = self.parent(path, true, owner)?;
        let temporary = cstring(OsStr::new(&format!(
            ".rustaller-current-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))?;
        let fd = unsafe {
            openat(
                parent.as_raw_fd(),
                temporary.as_ptr(),
                O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let result = (|| {
            file.write_all(bytes)?;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
            if let Some((uid, gid)) = owner
                && unsafe { fchown(file.as_raw_fd(), uid, gid) } != 0
            {
                return Err(io::Error::last_os_error());
            }
            file.sync_all()?;
            if unsafe {
                renameat(
                    parent.as_raw_fd(),
                    temporary.as_ptr(),
                    parent.as_raw_fd(),
                    name.as_ptr(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            parent.sync_all()
        })();
        if result.is_err() {
            unsafe { unlinkat(parent.as_raw_fd(), temporary.as_ptr(), 0) };
        }
        result.map_err(Into::into)
    }
    fn publish_link(&self, path: &Path, target: &Path) -> Result<(), CurrentError> {
        let (parent, name) = self.parent(path, true, None)?;
        let target = cstring(target.as_os_str())?;
        if unsafe { symlinkat(target.as_ptr(), parent.as_raw_fd(), name.as_ptr()) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
        parent.sync_all()?;
        Ok(())
    }
    fn networkd_paths(&self) -> Result<Vec<PathBuf>, CurrentError> {
        // Holding the directory descriptor prevents replacement by a symlink
        // between listing and subsequent descriptor-based file inspection.
        let path = Path::new("etc/systemd/network/.rustaller-check");
        let (parent, _) = self
            .parent(path, false, None)
            .map_err(|_| CurrentError::UnsupportedNetworkManager)?;
        let fd_path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
        let mut names = fs::read_dir(fd_path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        names.retain(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|ext| ext == "network")
        });
        if names
            .iter()
            .any(|name| name.to_string_lossy().chars().any(char::is_control))
        {
            return Err(CurrentError::UnsafePath);
        }
        names.sort();
        if names.is_empty() {
            return Err(CurrentError::UnsupportedNetworkManager);
        }
        let paths = names
            .into_iter()
            .map(|name| Path::new("etc/systemd/network").join(name))
            .collect::<Vec<_>>();
        for path in &paths {
            if !matches!(self.snapshot(path, false)?, Snapshot::Regular { .. }) {
                return Err(CurrentError::UnsafePath);
            }
        }
        Ok(paths)
    }
    fn validate_account_home(&self, home: &Path, uid: u32) -> Result<(), CurrentError> {
        let relative = home
            .strip_prefix("/")
            .map_err(|_| CurrentError::InvalidAccount)?;
        let (directory, _) = self.parent(&relative.join(".rustaller-check"), false, None)?;
        let metadata = directory.metadata()?;
        if metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
            return Err(CurrentError::InvalidAccount);
        }
        let ssh = relative.join(".ssh/.rustaller-check");
        match self.parent(&ssh, false, None) {
            Ok((directory, _)) => {
                let metadata = directory.metadata()?;
                if metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
                    return Err(CurrentError::InvalidAccount);
                }
            }
            Err(CurrentError::Filesystem(e)) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok(())
    }
}
fn open_dir(
    parent: &File,
    name: &OsStr,
    create: bool,
    owner: Option<(u32, u32)>,
) -> Result<File, CurrentError> {
    let name = cstring(name)?;
    let mut fd = unsafe {
        openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
            0,
        )
    };
    let mut created = false;
    if fd < 0 && create && io::Error::last_os_error().kind() == io::ErrorKind::NotFound {
        if unsafe {
            mkdirat(
                parent.as_raw_fd(),
                name.as_ptr(),
                if owner.is_some() { 0o700 } else { 0o755 },
            )
        } != 0
        {
            return Err(io::Error::last_os_error().into());
        }
        created = true;
        fd = unsafe {
            openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                0,
            )
        };
    }
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    if created
        && let Some((uid, gid)) = owner
        && unsafe { fchown(file.as_raw_fd(), uid, gid) } != 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(file)
}
fn read_link(parent: &File, name: &CString) -> Result<PathBuf, CurrentError> {
    let mut bytes = vec![0_u8; 8192];
    let count = unsafe {
        readlinkat(
            parent.as_raw_fd(),
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if count < 0 || count as usize == bytes.len() {
        return Err(CurrentError::UnsafePath);
    }
    bytes.truncate(count as usize);
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}
fn cstring(value: &OsStr) -> Result<CString, CurrentError> {
    CString::new(value.as_bytes()).map_err(|_| CurrentError::UnsafePath)
}
fn resolve_inside_root(base: &Path, target: &Path) -> Result<PathBuf, CurrentError> {
    let combined = if target.is_absolute() {
        target
            .strip_prefix("/")
            .map_err(|_| CurrentError::UnsafePath)?
            .to_owned()
    } else {
        base.join(target)
    };
    let mut result = PathBuf::new();
    for part in combined.components() {
        match part {
            Component::Normal(name) => result.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return Err(CurrentError::UnsafePath);
                }
            }
            _ => return Err(CurrentError::UnsafePath),
        }
    }
    if result.as_os_str().is_empty() {
        return Err(CurrentError::UnsafePath);
    }
    Ok(result)
}
unsafe extern "C" {
    fn geteuid() -> u32;
    fn openat(fd: i32, path: *const std::ffi::c_char, flags: i32, mode: u32) -> i32;
    fn mkdirat(fd: i32, path: *const std::ffi::c_char, mode: u32) -> i32;
    fn renameat(
        oldfd: i32,
        old: *const std::ffi::c_char,
        newfd: i32,
        new: *const std::ffi::c_char,
    ) -> i32;
    fn unlinkat(fd: i32, path: *const std::ffi::c_char, flags: i32) -> i32;
    fn symlinkat(target: *const std::ffi::c_char, fd: i32, path: *const std::ffi::c_char) -> i32;
    fn readlinkat(
        fd: i32,
        path: *const std::ffi::c_char,
        buffer: *mut std::ffi::c_char,
        size: usize,
    ) -> isize;
    fn fchown(fd: i32, uid: u32, gid: u32) -> i32;
}
