//! Target-only DNS: preserve automatic configuration, or override DHCP/RA DNS.
//! The caller owns the target exclusively. No host DNS or service is changed.
use super::configure::{
    ConfigurationError, validate_destination, validate_directory_tree, validate_root,
};
use crate::config::DnsMode;
use crate::fsutil::atomic_replace;
use crate::model::{InstallPlan, Target};
use crate::worker::live::ValidatedLivePlan;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_CONFIG_SIZE: u64 = 1024 * 1024;
const BEGIN: &str = "# BEGIN rustaller target DNS";
const END: &str = "# END rustaller target DNS";

pub(crate) struct ConfigFile {
    pub path: PathBuf,
    pub contents: Vec<u8>,
}

pub fn apply_target_dns(root: &Path, plan: &ValidatedLivePlan) -> Result<(), ConfigurationError> {
    publish_files(prepare_target_dns(root, plan.plan())?)
}

pub(crate) fn prepare_target_dns(
    root: &Path,
    plan: &InstallPlan,
) -> Result<Vec<ConfigFile>, ConfigurationError> {
    if plan.hold == Some(1) {
        return Err(ConfigurationError::RescuePlan);
    }
    plan.system
        .validate(plan.target, plan.hold)
        .map_err(ConfigurationError::SystemConfig)?;
    validate_root(root)?;
    let dns = &plan.system.dns;
    if dns.mode == DnsMode::Auto {
        return Ok(Vec::new());
    }
    validate_directory_tree(&root.join("etc"))?;
    let resolv = root.join("etc/resolv.conf");
    validate_destination(&resolv, true)?;
    let mut files = vec![ConfigFile {
        path: resolv,
        contents: dns
            .resolv_conf()
            .map_err(ConfigurationError::SystemConfig)?
            .expect("static DNS")
            .into_bytes(),
    }];
    if plan.target == Target::Alpine {
        for (name, body) in [
            ("etc/dhcpcd.conf", "nohook resolv.conf\n"),
            ("etc/udhcpc/udhcpc.conf", "RESOLV_CONF=\"no\"\n"),
        ] {
            let path = root.join(name);
            validate_planned_file(&path)?;
            let text = managed_block(&read_config(&path)?, body, true)?;
            files.push(ConfigFile {
                path,
                contents: text.into_bytes(),
            });
        }
    } else {
        for path in networkd_files(root)? {
            let body = dns
                .networkd_settings()
                .map_err(ConfigurationError::SystemConfig)?
                .expect("static DNS");
            let text = managed_block(&read_config(&path)?, &body, false)?;
            files.push(ConfigFile {
                path,
                contents: text.into_bytes(),
            });
        }
    }
    Ok(files)
}

pub(crate) fn networkd_files(root: &Path) -> Result<Vec<PathBuf>, ConfigurationError> {
    let directory = root.join("etc/systemd/network");
    validate_directory_tree(&directory)?;
    let mut paths = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| path.extension().is_some_and(|ext| ext == "network"));
    paths.sort();
    if paths.is_empty() {
        return Err(ConfigurationError::MissingNetworkConfig);
    }
    for path in &paths {
        validate_planned_file(path)?;
    }
    Ok(paths)
}

pub(crate) fn publish_files(files: Vec<ConfigFile>) -> Result<(), ConfigurationError> {
    for file in files {
        ensure_parent(&file.path)?;
        atomic_replace(&file.path, &file.contents, 0o644)?;
    }
    Ok(())
}

/// Missing future directories are allowed; existing ancestors may not be links.
pub(crate) fn validate_planned_file(path: &Path) -> Result<(), ConfigurationError> {
    validate_planned_parent(path)?;
    validate_destination(path, false)
}

pub(crate) fn validate_planned_parent(path: &Path) -> Result<(), ConfigurationError> {
    let mut prefix = PathBuf::new();
    for component in path
        .parent()
        .ok_or(ConfigurationError::UnsafePath)?
        .components()
    {
        prefix.push(component);
        match fs::symlink_metadata(&prefix) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(ConfigurationError::UnsafePath),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn ensure_parent(path: &Path) -> Result<(), ConfigurationError> {
    let mut prefix = PathBuf::new();
    for component in path
        .parent()
        .ok_or(ConfigurationError::UnsafePath)?
        .components()
    {
        prefix.push(component);
        match fs::symlink_metadata(&prefix) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err(ConfigurationError::UnsafePath),
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&prefix)?,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(crate) fn read_config(path: &Path) -> Result<String, ConfigurationError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() || file.metadata()?.len() > MAX_CONFIG_SIZE {
        return Err(ConfigurationError::InvalidConfigFile);
    }
    let mut text = String::new();
    file.take(MAX_CONFIG_SIZE + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_CONFIG_SIZE || text.contains('\0') {
        return Err(ConfigurationError::InvalidConfigFile);
    }
    Ok(text)
}

fn managed_block(existing: &str, body: &str, prepend: bool) -> Result<String, ConfigurationError> {
    replace_managed_block(existing, body, BEGIN, END, prepend)
}

pub(crate) fn replace_managed_block(
    existing: &str,
    body: &str,
    begin: &str,
    end: &str,
    prepend: bool,
) -> Result<String, ConfigurationError> {
    let mut base = existing.to_owned();
    match (
        existing.matches(begin).count(),
        existing.matches(end).count(),
    ) {
        (0, 0) => {}
        (1, 1) => {
            let start = existing.find(begin).expect("one begin");
            let finish = existing.find(end).expect("one end") + end.len();
            if start >= finish
                || (start > 0 && existing.as_bytes()[start - 1] != b'\n')
                || (finish < existing.len() && existing.as_bytes()[finish] != b'\n')
            {
                return Err(ConfigurationError::InvalidConfigFile);
            }
            let finish = finish + usize::from(existing.as_bytes().get(finish) == Some(&b'\n'));
            base.replace_range(start..finish, "");
        }
        _ => return Err(ConfigurationError::InvalidConfigFile),
    }
    if body.is_empty() {
        return Ok(base);
    }
    if !base.is_empty() && !base.ends_with('\n') {
        base.push('\n');
    }
    let block = format!("{begin}\n{body}{end}\n");
    Ok(if prepend {
        format!("{block}{base}")
    } else {
        format!("{base}{block}")
    })
}
