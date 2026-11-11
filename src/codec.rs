use crate::config::NetworkConfig;
use crate::config::SwapConfig;
use crate::config::{DnsConfig, DnsMode, InstallProfile, SystemConfig, SystemConfigError};
use crate::model::{
    BootMode, CURRENT_SCHEMA_VERSION, InstallPlan, IsoOptions, PlanError, SecretString, Target,
};
use crate::postinstall::{KernelChoice, PostInstallConfig};

const MAGIC: &[u8] = b"RINSTALL\0";
const MAX_PLAN_SIZE: usize = 4 * 1024 * 1024;
const MAX_FIELD_SIZE: usize = 1024 * 1024;
const MAX_SSH_KEYS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanCodecError {
    BadMagic,
    Truncated,
    InvalidTag,
    InvalidUtf8,
    Oversized,
    TooManySshKeys,
    TrailingData,
    InvalidPlan(PlanError),
}

impl std::fmt::Display for PlanCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => f.write_str("Invalid install plan magic"),
            Self::Truncated => f.write_str("Truncated install plan"),
            Self::InvalidTag => f.write_str("Invalid install plan field tag"),
            Self::InvalidUtf8 => f.write_str("Install plan contains invalid UTF-8"),
            Self::Oversized => f.write_str("Install plan exceeds the supported size"),
            Self::TooManySshKeys => f.write_str("Install plan contains too many SSH keys"),
            Self::TrailingData => f.write_str("Install plan contains trailing data"),
            Self::InvalidPlan(error) => write!(f, "Invalid install plan: {error}"),
        }
    }
}

impl std::error::Error for PlanCodecError {}

pub fn encode_plan(plan: &InstallPlan) -> Result<Vec<u8>, PlanCodecError> {
    let mut output = Vec::new();
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&plan.schema_version.to_le_bytes());
    output.push(target_to_tag(plan.target));
    put_optional_string(&mut output, plan.username.as_deref())?;
    put_optional_bytes(
        &mut output,
        plan.password
            .as_ref()
            .map(|secret| secret.expose().as_bytes()),
    )?;

    if plan.ssh_keys.len() > MAX_SSH_KEYS {
        return Err(PlanCodecError::TooManySshKeys);
    }
    put_u32(&mut output, plan.ssh_keys.len() as u32);
    for key in &plan.ssh_keys {
        put_bytes(&mut output, key.as_bytes())?;
    }

    put_optional_u16(&mut output, plan.ssh_port);
    put_optional_u16(&mut output, plan.web_port);
    put_optional_string(&mut output, plan.frpc_config.as_deref())?;
    put_optional_string(&mut output, plan.target_disk.as_deref())?;
    output.push(u8::from(plan.no_cloud_kernel));
    put_optional_u8(&mut output, plan.hold);
    output.push(match plan.force_boot_mode {
        None => 0,
        Some(BootMode::Bios) => 1,
        Some(BootMode::Efi) => 2,
    });
    put_optional_string(&mut output, plan.confhome.as_deref())?;
    match &plan.iso {
        None => output.push(0),
        Some(iso) => {
            output.push(1);
            put_string(&mut output, &iso.path)?;
            put_optional_string(&mut output, iso.config.as_deref())?;
            put_optional_string(&mut output, iso.kernel.as_deref())?;
            put_optional_string(&mut output, iso.initrd.as_deref())?;
            put_optional_string(&mut output, iso.cmdline.as_deref())?;
        }
    }
    output.push(u8::from(plan.debug));
    output.push(match plan.system.profile {
        InstallProfile::Standard => 0,
        InstallProfile::Minimal => 1,
    });
    for flag in [
        plan.system.with_tools,
        plan.system.keep_docs,
        plan.system.keep_package_cache,
    ] {
        output.push(u8::from(flag));
    }
    output.push(match plan.system.dns.mode {
        DnsMode::Auto => 0,
        DnsMode::Static => 1,
    });
    if plan.system.dns.servers.len() > 3 {
        return Err(PlanCodecError::InvalidPlan(PlanError::SystemConfig(
            SystemConfigError::InvalidDnsCount,
        )));
    }
    output.push(plan.system.dns.servers.len() as u8);
    for server in &plan.system.dns.servers {
        put_string(&mut output, &server.to_string())?;
    }
    if plan.system.dns.search_domains.len() > 6 {
        return Err(PlanCodecError::InvalidPlan(PlanError::SystemConfig(
            SystemConfigError::InvalidSearchDomain,
        )));
    }
    output.push(plan.system.dns.search_domains.len() as u8);
    for domain in &plan.system.dns.search_domains {
        put_string(&mut output, domain)?;
    }
    let net = &plan.system.network;
    for choice in [
        net.congestion.as_str(),
        net.qdisc.as_str(),
        net.ipv6.as_str(),
        net.address_preference.as_str(),
    ] {
        put_string(&mut output, choice)?;
    }
    put_optional_u16(&mut output, net.mtu);
    put_optional_u16(&mut output, net.buffer_mib);
    for value in [net.somaxconn, net.syn_backlog, net.netdev_backlog] {
        put_optional_u32(&mut output, value);
    }
    for value in [net.mtu_probing, net.fastopen, net.syncookies.map(u8::from)] {
        put_optional_u8(&mut output, value);
    }
    put_optional_u32(&mut output, plan.system.swap.size_mib);
    put_optional_u8(&mut output, plan.system.swap.swappiness);
    output.push(match plan.post_install.kernel {
        KernelChoice::Auto => 0,
        KernelChoice::Cloud => 1,
        KernelChoice::Generic => 2,
        KernelChoice::Lts => 3,
        KernelChoice::Standard => 4,
    });
    put_optional_string(&mut output, plan.post_install.hostname.as_deref())?;
    put_optional_string(&mut output, plan.post_install.timezone.as_deref())?;

    if output.len() > MAX_PLAN_SIZE {
        return Err(PlanCodecError::Oversized);
    }
    Ok(output)
}

pub fn decode_plan(bytes: &[u8]) -> Result<InstallPlan, PlanCodecError> {
    if bytes.len() > MAX_PLAN_SIZE {
        return Err(PlanCodecError::Oversized);
    }
    if !bytes.starts_with(MAGIC) {
        return Err(PlanCodecError::BadMagic);
    }

    let mut reader = Reader::new(&bytes[MAGIC.len()..]);
    let schema_version = reader.read_u16()?;
    if schema_version != CURRENT_SCHEMA_VERSION {
        return Err(PlanCodecError::InvalidPlan(PlanError::UnsupportedSchema(
            schema_version,
        )));
    }
    let target = tag_to_target(reader.read_u8()?)?;
    let username = reader.read_optional_string()?;
    let password = match reader.read_optional_bytes()? {
        Some(value) => {
            Some(SecretString::from_bytes(value).map_err(|_| PlanCodecError::InvalidUtf8)?)
        }
        None => None,
    };

    let key_count = reader.read_u32()? as usize;
    if key_count > MAX_SSH_KEYS {
        return Err(PlanCodecError::TooManySshKeys);
    }
    let mut ssh_keys = Vec::with_capacity(key_count);
    for _ in 0..key_count {
        ssh_keys.push(reader.read_string()?);
    }

    let ssh_port = reader.read_optional_u16()?;
    let web_port = reader.read_optional_u16()?;
    let frpc_config = reader.read_optional_string()?;
    let target_disk = reader.read_optional_string()?;
    let no_cloud_kernel = reader.read_bool()?;
    let hold = reader.read_optional_u8()?;
    let force_boot_mode = match reader.read_u8()? {
        0 => None,
        1 => Some(BootMode::Bios),
        2 => Some(BootMode::Efi),
        _ => return Err(PlanCodecError::InvalidTag),
    };
    let confhome = reader.read_optional_string()?;
    let iso = match reader.read_u8()? {
        0 => None,
        1 => Some(IsoOptions {
            path: reader.read_string()?,
            config: reader.read_optional_string()?,
            kernel: reader.read_optional_string()?,
            initrd: reader.read_optional_string()?,
            cmdline: reader.read_optional_string()?,
        }),
        _ => return Err(PlanCodecError::InvalidTag),
    };
    let debug = reader.read_bool()?;
    let profile = match reader.read_u8()? {
        0 => InstallProfile::Standard,
        1 => InstallProfile::Minimal,
        _ => return Err(PlanCodecError::InvalidTag),
    };
    let with_tools = reader.read_bool()?;
    let keep_docs = reader.read_bool()?;
    let keep_package_cache = reader.read_bool()?;
    let mode = match reader.read_u8()? {
        0 => DnsMode::Auto,
        1 => DnsMode::Static,
        _ => return Err(PlanCodecError::InvalidTag),
    };
    let server_count = reader.read_u8()?;
    if server_count > 3 {
        return Err(PlanCodecError::InvalidTag);
    }
    let mut servers = Vec::new();
    for _ in 0..server_count {
        servers.push(
            reader
                .read_string()?
                .parse()
                .map_err(|_| PlanCodecError::InvalidTag)?,
        );
    }
    let domain_count = reader.read_u8()?;
    if domain_count > 6 {
        return Err(PlanCodecError::InvalidTag);
    }
    let mut search_domains = Vec::new();
    for _ in 0..domain_count {
        search_domains.push(reader.read_string()?);
    }
    let network = NetworkConfig {
        congestion: reader
            .read_string()?
            .parse()
            .map_err(|_| PlanCodecError::InvalidTag)?,
        qdisc: reader
            .read_string()?
            .parse()
            .map_err(|_| PlanCodecError::InvalidTag)?,
        ipv6: reader
            .read_string()?
            .parse()
            .map_err(|_| PlanCodecError::InvalidTag)?,
        address_preference: reader
            .read_string()?
            .parse()
            .map_err(|_| PlanCodecError::InvalidTag)?,
        mtu: reader.read_optional_u16()?,
        buffer_mib: reader.read_optional_u16()?,
        somaxconn: reader.read_optional_u32()?,
        syn_backlog: reader.read_optional_u32()?,
        netdev_backlog: reader.read_optional_u32()?,
        mtu_probing: reader.read_optional_u8()?,
        fastopen: reader.read_optional_u8()?,
        syncookies: match reader.read_optional_u8()? {
            None => None,
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => return Err(PlanCodecError::InvalidTag),
        },
    };
    let swap = SwapConfig {
        size_mib: reader.read_optional_u32()?,
        swappiness: reader.read_optional_u8()?,
    };
    let system = SystemConfig {
        profile,
        with_tools,
        keep_docs,
        keep_package_cache,
        dns: DnsConfig {
            mode,
            servers,
            search_domains,
        },
        network,
        swap,
    };
    let post_install = PostInstallConfig {
        kernel: match reader.read_u8()? {
            0 => KernelChoice::Auto,
            1 => KernelChoice::Cloud,
            2 => KernelChoice::Generic,
            3 => KernelChoice::Lts,
            4 => KernelChoice::Standard,
            _ => return Err(PlanCodecError::InvalidTag),
        },
        hostname: reader.read_optional_string()?,
        timezone: reader.read_optional_string()?,
    };
    if !reader.is_empty() {
        return Err(PlanCodecError::TrailingData);
    }

    let plan = InstallPlan {
        schema_version,
        target,
        username,
        password,
        ssh_keys,
        ssh_port,
        web_port,
        frpc_config,
        target_disk,
        no_cloud_kernel,
        post_install,
        system,
        hold,
        force_boot_mode,
        confhome,
        iso,
        debug,
    };
    plan.validate().map_err(PlanCodecError::InvalidPlan)?;
    Ok(plan)
}

fn target_to_tag(target: Target) -> u8 {
    match target {
        Target::Alpine => 1,
        Target::Debian => 2,
        Target::Arch => 3,
        Target::Gentoo => 4,
        Target::LocalIso => 5,
        Target::NetbootXyz => 6,
        Target::Reset => 7,
    }
}

fn tag_to_target(tag: u8) -> Result<Target, PlanCodecError> {
    match tag {
        1 => Ok(Target::Alpine),
        2 => Ok(Target::Debian),
        3 => Ok(Target::Arch),
        4 => Ok(Target::Gentoo),
        5 => Ok(Target::LocalIso),
        6 => Ok(Target::NetbootXyz),
        7 => Ok(Target::Reset),
        _ => Err(PlanCodecError::InvalidTag),
    }
}

fn put_optional_u16(output: &mut Vec<u8>, value: Option<u16>) {
    match value {
        None => output.push(0),
        Some(value) => {
            output.push(1);
            output.extend_from_slice(&value.to_le_bytes());
        }
    }
}

fn put_optional_u32(output: &mut Vec<u8>, value: Option<u32>) {
    match value {
        None => output.push(0),
        Some(value) => {
            output.push(1);
            put_u32(output, value);
        }
    }
}

fn put_optional_u8(output: &mut Vec<u8>, value: Option<u8>) {
    match value {
        None => output.push(0),
        Some(value) => {
            output.push(1);
            output.push(value);
        }
    }
}

fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn put_optional_string(output: &mut Vec<u8>, value: Option<&str>) -> Result<(), PlanCodecError> {
    match value {
        None => output.push(0),
        Some(value) => {
            output.push(1);
            put_string(output, value)?;
        }
    }
    Ok(())
}

fn put_optional_bytes(output: &mut Vec<u8>, value: Option<&[u8]>) -> Result<(), PlanCodecError> {
    match value {
        None => output.push(0),
        Some(value) => {
            output.push(1);
            put_bytes(output, value)?;
        }
    }
    Ok(())
}

fn put_string(output: &mut Vec<u8>, value: &str) -> Result<(), PlanCodecError> {
    put_bytes(output, value.as_bytes())
}

fn put_bytes(output: &mut Vec<u8>, value: &[u8]) -> Result<(), PlanCodecError> {
    if value.len() > MAX_FIELD_SIZE || value.len() > u32::MAX as usize {
        return Err(PlanCodecError::Oversized);
    }
    put_u32(output, value.len() as u32);
    output.extend_from_slice(value);
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn is_empty(&self) -> bool {
        self.offset == self.bytes.len()
    }

    fn read_u8(&mut self) -> Result<u8, PlanCodecError> {
        let value = *self
            .bytes
            .get(self.offset)
            .ok_or(PlanCodecError::Truncated)?;
        self.offset += 1;
        Ok(value)
    }

    fn read_u16(&mut self) -> Result<u16, PlanCodecError> {
        let bytes = self.read_exact(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, PlanCodecError> {
        let bytes = self.read_exact(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_bool(&mut self) -> Result<bool, PlanCodecError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(PlanCodecError::InvalidTag),
        }
    }

    fn read_optional_u8(&mut self) -> Result<Option<u8>, PlanCodecError> {
        match self.read_u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.read_u8()?)),
            _ => Err(PlanCodecError::InvalidTag),
        }
    }

    fn read_optional_u16(&mut self) -> Result<Option<u16>, PlanCodecError> {
        match self.read_u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.read_u16()?)),
            _ => Err(PlanCodecError::InvalidTag),
        }
    }

    fn read_optional_u32(&mut self) -> Result<Option<u32>, PlanCodecError> {
        match self.read_u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.read_u32()?)),
            _ => Err(PlanCodecError::InvalidTag),
        }
    }

    fn read_optional_string(&mut self) -> Result<Option<String>, PlanCodecError> {
        match self.read_u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.read_string()?)),
            _ => Err(PlanCodecError::InvalidTag),
        }
    }

    fn read_optional_bytes(&mut self) -> Result<Option<Vec<u8>>, PlanCodecError> {
        match self.read_u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.read_bytes()?.to_vec())),
            _ => Err(PlanCodecError::InvalidTag),
        }
    }

    fn read_string(&mut self) -> Result<String, PlanCodecError> {
        String::from_utf8(self.read_bytes()?.to_vec()).map_err(|_| PlanCodecError::InvalidUtf8)
    }

    fn read_bytes(&mut self) -> Result<&'a [u8], PlanCodecError> {
        let length = self.read_u32()? as usize;
        if length > MAX_FIELD_SIZE {
            return Err(PlanCodecError::Oversized);
        }
        self.read_exact(length)
    }

    fn read_exact(&mut self, length: usize) -> Result<&'a [u8], PlanCodecError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(PlanCodecError::Oversized)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(PlanCodecError::Truncated)?;
        self.offset = end;
        Ok(value)
    }
}
