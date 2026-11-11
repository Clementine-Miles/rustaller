//! Private, bounded host-to-Live deployment facts. No login secrets belong here.
use crate::host::sources::{GENTOO_ARCHIVE, GentooStage3};
use crate::model::BootMode;
use std::fs::{self, File};
use std::io::{self, Read};
use std::net::IpAddr;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

const MAGIC: &[u8] = b"RDEPLOY\0\x01\0";
const MAX_SIZE: usize = 128 * 1024;
const MAX_FIELD: usize = 8192;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiskIdentity {
    pub device: String,
    pub size_bytes: u64,
    pub serial: String,
    pub wwn: String,
    pub partition_table_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkInterface {
    pub name: String,
    pub mac: String,
    pub ipv4_addresses: Vec<String>,
    pub ipv4_gateway: Option<String>,
    pub ipv6_addresses: Vec<String>,
    pub ipv6_gateway: Option<String>,
    pub dhcp4: bool,
    pub dhcp6: bool,
    pub accept_ra: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeploymentManifest {
    pub disk: DiskIdentity,
    pub boot_mode: BootMode,
    /// Official Alpine repository branch, for example v3.24.
    pub alpine_branch: String,
    pub debian_codename: String,
    pub gentoo_stage3: Option<GentooStage3>,
    pub network: Vec<NetworkInterface>,
}

#[derive(Debug)]
pub enum DeploymentError {
    Invalid(&'static str),
    Filesystem(io::Error),
}
impl std::fmt::Display for DeploymentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) => write!(f, "Invalid deployment manifest: {reason}"),
            Self::Filesystem(error) => write!(f, "Cannot access deployment manifest: {error}"),
        }
    }
}
impl std::error::Error for DeploymentError {}
impl From<io::Error> for DeploymentError {
    fn from(error: io::Error) -> Self {
        Self::Filesystem(error)
    }
}

impl DeploymentManifest {
    pub fn validate(&self) -> Result<(), DeploymentError> {
        let fail = DeploymentError::Invalid;
        if !self.disk.device.starts_with("/dev/")
            || self.disk.device[5..].is_empty()
            || !self.disk.device[5..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || self.disk.size_bytes < 1024 * 1024 * 1024
        {
            return Err(fail("whole-disk identity/size"));
        }
        for value in [
            &self.disk.serial,
            &self.disk.wwn,
            &self.disk.partition_table_id,
        ] {
            if value.len() > 256 || value.chars().any(char::is_control) {
                return Err(fail("disk identity value"));
            }
        }
        if self.disk.serial.is_empty()
            && self.disk.wwn.is_empty()
            && self.disk.partition_table_id.is_empty()
        {
            return Err(fail("no stable disk identifier"));
        }
        if !valid_branch(&self.alpine_branch) {
            return Err(fail("Alpine repository branch"));
        }
        if !self.debian_codename.is_empty()
            && !self.debian_codename.bytes().all(|b| b.is_ascii_lowercase())
        {
            return Err(fail("Debian stable codename"));
        }
        if let Some(stage) = &self.gentoo_stage3 {
            let timestamp = &stage.timestamp;
            if timestamp.len() != 16
                || timestamp.as_bytes()[8] != b'T'
                || timestamp.as_bytes()[15] != b'Z'
                || timestamp
                    .bytes()
                    .enumerate()
                    .any(|(i, b)| i != 8 && i != 15 && !b.is_ascii_digit())
                || stage.size == 0
                || stage.filename != format!("stage3-amd64-systemd-{timestamp}.tar.xz")
                || stage.url != format!("{GENTOO_ARCHIVE}/{timestamp}/{}", stage.filename)
                || stage.digests_url != format!("{}.DIGESTS", stage.url)
                || stage.signature_url != format!("{}.asc", stage.url)
            {
                return Err(fail("Gentoo source is not the official selected stage3"));
            }
        }
        if self.network.is_empty() || self.network.len() > 16 {
            return Err(fail("network interface count"));
        }
        for (index, nic) in self.network.iter().enumerate() {
            if nic.name.is_empty()
                || nic.name.len() > 15
                || nic.name == "lo"
                || !nic
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
                || self.network[..index]
                    .iter()
                    .any(|old| old.name == nic.name || old.mac.eq_ignore_ascii_case(&nic.mac))
            {
                return Err(fail("network interface identity"));
            }
            crate::worker::initrd_network::InitrdNetworkConfig::new(&nic.mac, "", "", "", "", "")
                .map_err(|_| fail("network MAC address"))?;
            for (v6, addresses, gateway) in [
                (false, &nic.ipv4_addresses, &nic.ipv4_gateway),
                (true, &nic.ipv6_addresses, &nic.ipv6_gateway),
            ] {
                if addresses.len() > 16 {
                    return Err(fail("too many interface addresses"));
                }
                for (i, value) in addresses.iter().enumerate() {
                    let (address, prefix) = value
                        .split_once('/')
                        .ok_or_else(|| fail("interface address requires CIDR"))?;
                    let address = address
                        .parse::<IpAddr>()
                        .map_err(|_| fail("interface IP address"))?;
                    let prefix = prefix.parse::<u8>().map_err(|_| fail("interface prefix"))?;
                    if address.is_ipv6() != v6
                        || address.is_unspecified()
                        || address.is_loopback()
                        || address.is_multicast()
                        || prefix > if v6 { 128 } else { 32 }
                        || addresses[..i].contains(value)
                    {
                        return Err(fail("invalid/duplicate interface address"));
                    }
                }
                if let Some(gateway) = gateway {
                    let address = gateway.parse::<IpAddr>().map_err(|_| fail("gateway IP"))?;
                    if address.is_ipv6() != v6
                        || address.is_unspecified()
                        || address.is_loopback()
                        || address.is_multicast()
                    {
                        return Err(fail("gateway address family"));
                    }
                }
            }
            if nic.ipv4_addresses.is_empty()
                && nic.ipv6_addresses.is_empty()
                && !nic.dhcp4
                && !nic.dhcp6
                && !nic.accept_ra
            {
                return Err(fail("interface has no network configuration"));
            }
        }
        Ok(())
    }
}

pub fn valid_branch(value: &str) -> bool {
    value
        .strip_prefix('v')
        .and_then(|v| v.split_once('.'))
        .is_some_and(|(a, b)| {
            !a.is_empty()
                && !b.is_empty()
                && a.bytes().all(|v| v.is_ascii_digit())
                && b.bytes().all(|v| v.is_ascii_digit())
        })
}

pub fn encode_manifest(manifest: &DeploymentManifest) -> Result<Vec<u8>, DeploymentError> {
    manifest.validate()?;
    let mut bytes = MAGIC.to_vec();
    for value in [
        &manifest.disk.device,
        &manifest.disk.serial,
        &manifest.disk.wwn,
        &manifest.disk.partition_table_id,
    ] {
        put(&mut bytes, value)?;
    }
    bytes.extend_from_slice(&manifest.disk.size_bytes.to_le_bytes());
    bytes.push(u8::from(manifest.boot_mode == BootMode::Efi));
    put(&mut bytes, &manifest.alpine_branch)?;
    put(&mut bytes, &manifest.debian_codename)?;
    bytes.push(u8::from(manifest.gentoo_stage3.is_some()));
    if let Some(stage) = &manifest.gentoo_stage3 {
        for value in [
            &stage.timestamp,
            &stage.filename,
            &stage.url,
            &stage.digests_url,
            &stage.signature_url,
        ] {
            put(&mut bytes, value)?;
        }
        bytes.extend_from_slice(&stage.size.to_le_bytes());
    }
    bytes.push(manifest.network.len() as u8);
    for nic in &manifest.network {
        put(&mut bytes, &nic.name)?;
        put(&mut bytes, &nic.mac)?;
        for addresses in [&nic.ipv4_addresses, &nic.ipv6_addresses] {
            bytes.push(addresses.len() as u8);
            for address in addresses {
                put(&mut bytes, address)?;
            }
        }
        put(&mut bytes, nic.ipv4_gateway.as_deref().unwrap_or_default())?;
        put(&mut bytes, nic.ipv6_gateway.as_deref().unwrap_or_default())?;
        bytes.extend([
            u8::from(nic.dhcp4),
            u8::from(nic.dhcp6),
            u8::from(nic.accept_ra),
        ]);
    }
    if bytes.len() > MAX_SIZE {
        return Err(DeploymentError::Invalid("oversized"));
    }
    Ok(bytes)
}
fn put(bytes: &mut Vec<u8>, value: &str) -> Result<(), DeploymentError> {
    if value.len() > MAX_FIELD {
        return Err(DeploymentError::Invalid("oversized field"));
    }
    bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
    Ok(())
}

pub fn decode_manifest(bytes: &[u8]) -> Result<DeploymentManifest, DeploymentError> {
    if bytes.len() > MAX_SIZE || !bytes.starts_with(MAGIC) {
        return Err(DeploymentError::Invalid("header/version/size"));
    }
    let mut r = Reader {
        bytes: &bytes[MAGIC.len()..],
    };
    let device = r.string()?;
    let serial = r.string()?;
    let wwn = r.string()?;
    let partition_table_id = r.string()?;
    let size_bytes = r.u64()?;
    let boot_mode = if r.boolean()? {
        BootMode::Efi
    } else {
        BootMode::Bios
    };
    let alpine_branch = r.string()?;
    let debian_codename = r.string()?;
    let gentoo_stage3 = if r.boolean()? {
        Some(GentooStage3 {
            timestamp: r.string()?,
            filename: r.string()?,
            url: r.string()?,
            digests_url: r.string()?,
            signature_url: r.string()?,
            size: r.u64()?,
        })
    } else {
        None
    };
    let count = r.u8()?;
    if count > 16 {
        return Err(DeploymentError::Invalid("interface count"));
    }
    let mut network = Vec::new();
    for _ in 0..count {
        let name = r.string()?;
        let mac = r.string()?;
        let mut sets = Vec::new();
        for _ in 0..2 {
            let count = r.u8()?;
            if count > 16 {
                return Err(DeploymentError::Invalid("address count"));
            }
            let mut addresses = Vec::new();
            for _ in 0..count {
                addresses.push(r.string()?);
            }
            sets.push(addresses);
        }
        let ipv4_gateway = r.string()?;
        let ipv6_gateway = r.string()?;
        network.push(NetworkInterface {
            name,
            mac,
            ipv4_addresses: sets.remove(0),
            ipv6_addresses: sets.remove(0),
            ipv4_gateway: (!ipv4_gateway.is_empty()).then_some(ipv4_gateway),
            ipv6_gateway: (!ipv6_gateway.is_empty()).then_some(ipv6_gateway),
            dhcp4: r.boolean()?,
            dhcp6: r.boolean()?,
            accept_ra: r.boolean()?,
        });
    }
    if !r.bytes.is_empty() {
        return Err(DeploymentError::Invalid("trailing data"));
    }
    let manifest = DeploymentManifest {
        disk: DiskIdentity {
            device,
            size_bytes,
            serial,
            wwn,
            partition_table_id,
        },
        boot_mode,
        alpine_branch,
        debian_codename,
        gentoo_stage3,
        network,
    };
    manifest.validate()?;
    Ok(manifest)
}
struct Reader<'a> {
    bytes: &'a [u8],
}
impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], DeploymentError> {
        if n > self.bytes.len() {
            return Err(DeploymentError::Invalid("truncated"));
        }
        let (a, b) = self.bytes.split_at(n);
        self.bytes = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, DeploymentError> {
        Ok(self.take(1)?[0])
    }
    fn boolean(&mut self) -> Result<bool, DeploymentError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DeploymentError::Invalid("boolean tag")),
        }
    }
    fn u64(&mut self) -> Result<u64, DeploymentError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("eight bytes"),
        ))
    }
    fn string(&mut self) -> Result<String, DeploymentError> {
        let n = u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")) as usize;
        if n > MAX_FIELD {
            return Err(DeploymentError::Invalid("field size"));
        }
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| DeploymentError::Invalid("UTF-8"))
    }
}

pub fn write_manifest(path: &Path, manifest: &DeploymentManifest) -> Result<(), DeploymentError> {
    crate::shared::dns::validate_planned_file(path)
        .map_err(|_| DeploymentError::Invalid("unsafe file path"))?;
    crate::fsutil::atomic_replace(path, &encode_manifest(manifest)?, 0o600)?;
    Ok(())
}
pub fn read_manifest(path: &Path) -> Result<DeploymentManifest, DeploymentError> {
    crate::shared::dns::validate_planned_file(path)
        .map_err(|_| DeploymentError::Invalid("unsafe file path"))?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > MAX_SIZE as u64
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { geteuid() }
    {
        return Err(DeploymentError::Invalid(
            "private caller-owned regular file required",
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_SIZE + 1) as u64)
        .read_to_end(&mut bytes)?;
    decode_manifest(&bytes)
}
unsafe extern "C" {
    fn geteuid() -> u32;
}
