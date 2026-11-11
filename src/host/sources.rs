//! Official x86_64 installation sources. Parsing a digest or armored message
//! does not authenticate it; callers must verify trusted signatures and payloads
//! before extracting or executing downloaded content.

use crate::model::Target;
use std::collections::BTreeMap;
use std::fmt;

pub const ALPINE_RELEASES_URL: &str =
    "https://dl-cdn.alpinelinux.org/alpine/latest-stable/releases/x86_64/latest-releases.yaml";
pub const ALPINE_ARCHIVE: &str = "https://dl-cdn.alpinelinux.org/alpine";
pub const DEBIAN_RELEASE_URL: &str = "https://deb.debian.org/debian/dists/stable/Release";
pub const DEBIAN_ARCHIVE: &str = "https://deb.debian.org/debian";
pub const DEBIAN_SECURITY_ARCHIVE: &str = "https://security.debian.org/debian-security";
pub const ARCH_MIRROR: &str = "https://geo.mirror.pkgbuild.com";
pub const GENTOO_ARCHIVE: &str = "https://distfiles.gentoo.org/releases/amd64/autobuilds";
pub const GENTOO_STAGE3_INDEX_URL: &str =
    "https://distfiles.gentoo.org/releases/amd64/autobuilds/latest-stage3-amd64-systemd.txt";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceKind {
    NetbootArchive,
    DebianArchive,
    PackageRepository,
    SystemdStage3,
}

pub fn source_kind(target: Target, architecture: &str) -> Result<SourceKind, SourceError> {
    require_x86_64(architecture)?;
    match target {
        Target::Alpine => Ok(SourceKind::NetbootArchive),
        Target::Debian => Ok(SourceKind::DebianArchive),
        Target::Arch => Ok(SourceKind::PackageRepository),
        Target::Gentoo => Ok(SourceKind::SystemdStage3),
        _ => Err(SourceError("target has no automatic installation source")),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceError(pub &'static str);

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for SourceError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlpineNetboot {
    pub version: String,
    pub branch: String,
    pub filename: String,
    pub url: String,
    pub sha256: String,
    pub checksum_url: String,
    pub signature_url: String,
}

pub fn parse_alpine_netboot(
    metadata: &str,
    architecture: &str,
) -> Result<AlpineNetboot, SourceError> {
    require_x86_64(architecture)?;
    let release = parse_alpine_release_metadata(metadata)
        .map_err(|_| SourceError("invalid or ambiguous Alpine stable version"))?;
    let mut records = Vec::new();
    let mut record = BTreeMap::new();
    for line in metadata.lines() {
        let line = line.trim();
        if line == "-" || line.starts_with("- ") {
            records.push(std::mem::take(&mut record));
        }
        let line = line.strip_prefix("- ").unwrap_or(line);
        if let Some((key, value)) = line.split_once(':')
            && matches!(
                key,
                "branch" | "arch" | "version" | "flavor" | "file" | "iso" | "sha256"
            )
            && record.insert(key, value.trim()).is_some()
        {
            return Err(SourceError("duplicate Alpine metadata field"));
        }
    }
    records.push(record);
    let mut candidates = records
        .iter()
        .filter(|record| record.get("flavor") == Some(&"alpine-netboot"));
    let record = candidates
        .next()
        .ok_or(SourceError("Alpine netboot archive is missing"))?;
    if candidates.next().is_some() {
        return Err(SourceError("duplicate Alpine netboot archive"));
    }
    let field = |name| {
        record
            .get(name)
            .copied()
            .ok_or(SourceError("incomplete Alpine netboot metadata"))
    };
    let branch = field("branch")?;
    let version = field("version")?;
    let filename = field("file")?;
    if field("arch")? != "x86_64"
        || branch != format!("v{release}")
        || !version.starts_with(&format!("{release}."))
        || !numeric_version(version, 3)
        || filename != format!("alpine-netboot-{version}-x86_64.tar.gz")
        || record.get("iso").is_some_and(|iso| *iso != filename)
    {
        return Err(SourceError(
            "Alpine netboot archive identity does not match its release",
        ));
    }
    let sha256 = hex_digest(field("sha256")?, 64)?;
    let url = format!("{ALPINE_ARCHIVE}/{branch}/releases/x86_64/{filename}");
    Ok(AlpineNetboot {
        version: version.into(),
        branch: branch.into(),
        filename: filename.into(),
        checksum_url: format!("{url}.sha256"),
        signature_url: format!("{url}.asc"),
        url,
        sha256,
    })
}

pub fn validate_alpine_checksum(source: &AlpineNetboot, metadata: &str) -> Result<(), SourceError> {
    let lines: Vec<_> = metadata
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() != 1 {
        return Err(SourceError(
            "Alpine checksum must identify exactly one archive",
        ));
    }
    let fields: Vec<_> = lines[0].split_ascii_whitespace().collect();
    if fields.len() != 2
        || fields[1].strip_prefix('*').unwrap_or(fields[1]) != source.filename
        || hex_digest(fields[0], 64)? != source.sha256
    {
        return Err(SourceError(
            "Alpine checksum does not match the selected archive",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebianArchive {
    pub version: String,
    pub codename: String,
    pub archive: &'static str,
    pub inrelease_url: String,
    pub security_inrelease_url: String,
}

pub fn parse_debian_archive(
    metadata: &str,
    architecture: &str,
) -> Result<DebianArchive, SourceError> {
    require_x86_64(architecture)?;
    // RFC822 permits tabs after a colon; the legacy parser only reads the
    // canonical spelling. Reject duplicates under either spacing first.
    single_field(metadata, "Version")?;
    single_field(metadata, "Codename")?;
    let StableRelease::Debian { version, codename } = parse_debian_release_metadata(metadata)
        .map_err(|_| SourceError("invalid Debian stable release metadata"))?
    else {
        return Err(SourceError("expected Debian release metadata"));
    };
    if single_field(metadata, "Origin")? != "Debian"
        || single_field(metadata, "Suite")? != "stable"
        || single_field(metadata, "Architectures")?
            .split_ascii_whitespace()
            .filter(|arch| *arch == "amd64")
            .count()
            != 1
    {
        return Err(SourceError(
            "Debian release identity or amd64 support is invalid",
        ));
    }
    Ok(DebianArchive {
        version,
        inrelease_url: format!("{DEBIAN_ARCHIVE}/dists/{codename}/InRelease"),
        security_inrelease_url: format!(
            "{DEBIAN_SECURITY_ARCHIVE}/dists/{codename}-security/InRelease"
        ),
        codename,
        archive: DEBIAN_ARCHIVE,
    })
}

/// Checks the selected release identity inside an armored InRelease. This does
/// not verify its OpenPGP signature; that requires a trusted Debian keyring.
pub fn validate_debian_inrelease(
    source: &DebianArchive,
    metadata: &str,
) -> Result<(), SourceError> {
    if parse_debian_archive(clearsigned_payload(metadata)?, "x86_64")? != *source {
        return Err(SourceError(
            "Debian signed-index metadata differs from the selected release",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchRepositories {
    pub core_db: String,
    pub extra_db: String,
    pub keyring_package: &'static str,
    pub signature_policy: &'static str,
}

pub fn arch_repositories(architecture: &str) -> Result<ArchRepositories, SourceError> {
    require_x86_64(architecture)?;
    Ok(ArchRepositories {
        core_db: format!("{ARCH_MIRROR}/core/os/x86_64/core.db"),
        extra_db: format!("{ARCH_MIRROR}/extra/os/x86_64/extra.db"),
        keyring_package: "archlinux-keyring",
        signature_policy: "Required DatabaseOptional",
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GentooStage3 {
    pub timestamp: String,
    pub filename: String,
    pub size: u64,
    pub url: String,
    pub digests_url: String,
    pub signature_url: String,
}

pub fn parse_gentoo_stage3(
    metadata: &str,
    architecture: &str,
) -> Result<GentooStage3, SourceError> {
    require_x86_64(architecture)?;
    let lines: Vec<_> = clearsigned_payload(metadata)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    if lines.len() != 1 {
        return Err(SourceError("Gentoo index must select exactly one stage3"));
    }
    let fields: Vec<_> = lines[0].split_ascii_whitespace().collect();
    if fields.len() != 2 {
        return Err(SourceError("invalid Gentoo stage3 index record"));
    }
    let (timestamp, filename) = fields[0]
        .split_once('/')
        .ok_or(SourceError("invalid Gentoo stage3 path"))?;
    if !valid_timestamp(timestamp)
        || filename != format!("stage3-amd64-systemd-{timestamp}.tar.xz")
        || !fields[1].bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(SourceError(
            "Gentoo stage3 must be a timestamped amd64 systemd archive",
        ));
    }
    let size = fields[1]
        .parse::<u64>()
        .ok()
        .filter(|size| *size > 0)
        .ok_or(SourceError("invalid Gentoo stage3 size"))?;
    let url = format!("{GENTOO_ARCHIVE}/{timestamp}/{filename}");
    Ok(GentooStage3 {
        timestamp: timestamp.into(),
        filename: filename.into(),
        size,
        digests_url: format!("{url}.DIGESTS"),
        signature_url: format!("{url}.asc"),
        url,
    })
}

pub fn parse_gentoo_sha512(source: &GentooStage3, metadata: &str) -> Result<String, SourceError> {
    let mut algorithm = None;
    let mut result = None;
    for line in clearsigned_payload(metadata)?.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            algorithm = match line {
                "# SHA512 HASH" => Some(true),
                "# BLAKE2B HASH" => Some(false),
                _ => return Err(SourceError("unsupported Gentoo digest section")),
            };
            continue;
        }
        let is_sha512 = algorithm.ok_or(SourceError("Gentoo digest has no algorithm section"))?;
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        if fields.len() != 2
            || (fields[1] != source.filename
                && fields[1] != format!("{}.CONTENTS.gz", source.filename))
        {
            return Err(SourceError("Gentoo digest names an unexpected archive"));
        }
        let digest = hex_digest(fields[0], 128)?;
        if is_sha512 && fields[1] == source.filename && result.replace(digest).is_some() {
            return Err(SourceError("duplicate Gentoo archive SHA512"));
        }
    }
    result.ok_or(SourceError("selected Gentoo stage3 has no SHA512 digest"))
}

/// Extracts, but never authenticates, an ASCII-armored signed message.
pub fn clearsigned_payload(metadata: &str) -> Result<&str, SourceError> {
    const BEGIN: &str = "-----BEGIN PGP SIGNED MESSAGE-----";
    const SIGNATURE: &str = "-----BEGIN PGP SIGNATURE-----";
    const END: &str = "-----END PGP SIGNATURE-----";
    if !metadata.starts_with(BEGIN)
        || !metadata.trim_end().ends_with(END)
        || [BEGIN, SIGNATURE, END]
            .iter()
            .any(|marker| metadata.matches(marker).count() != 1)
    {
        return Err(SourceError(
            "expected one complete clearsigned metadata message",
        ));
    }
    let separator = if metadata.contains("\r\n\r\n") {
        "\r\n\r\n"
    } else {
        "\n\n"
    };
    let (header, rest) = metadata
        .split_once(separator)
        .ok_or(SourceError("missing clearsigned metadata header"))?;
    if header
        .lines()
        .filter(|line| line.starts_with("Hash: "))
        .count()
        != 1
    {
        return Err(SourceError("invalid clearsigned metadata hash header"));
    }
    let (payload, signature) = rest
        .split_once(SIGNATURE)
        .ok_or(SourceError("missing metadata signature block"))?;
    if payload.is_empty()
        || !payload.ends_with('\n')
        || signature.trim().trim_end_matches(END).trim().is_empty()
    {
        return Err(SourceError("empty signed metadata or signature block"));
    }
    Ok(payload)
}

fn single_field<'a>(metadata: &'a str, key: &str) -> Result<&'a str, SourceError> {
    let mut fields = metadata
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| *name == key)
        .map(|(_, value)| value.trim());
    let value = fields
        .next()
        .filter(|value| !value.is_empty())
        .ok_or(SourceError("required source metadata field is missing"))?;
    if fields.next().is_some() {
        return Err(SourceError("duplicate source metadata field"));
    }
    Ok(value)
}

fn require_x86_64(architecture: &str) -> Result<(), SourceError> {
    if architecture == "x86_64" {
        Ok(())
    } else {
        Err(SourceError("installation sources support x86_64 only"))
    }
}

fn hex_digest(value: &str, length: usize) -> Result<String, SourceError> {
    if value.len() != length || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SourceError("invalid source checksum"));
    }
    Ok(value.to_ascii_lowercase())
}

fn numeric_version(value: &str, count: usize) -> bool {
    value.split('.').count() == count
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn valid_timestamp(value: &str) -> bool {
    value.len() == 16
        && value.as_bytes()[8] == b'T'
        && value.as_bytes()[15] == b'Z'
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| index == 8 || index == 15 || byte.is_ascii_digit())
}

// Stable-release discovery shares the same official source definitions.
use crate::command::{CommandInput, CommandRunner, CommandSpec, OutputMode};
use std::collections::BTreeSet;
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StableRelease {
    Alpine { version: String },
    Debian { version: String, codename: String },
}

#[derive(Debug)]
pub enum MetadataError {
    UnsupportedTarget(Target),
    InvalidArchitecture(String),
    InvalidAlpineMetadata,
    InvalidDebianMetadata,
    FetchFailed { url: String, status: Option<i32> },
    InvalidUtf8,
    Command(std::io::Error),
}

impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTarget(target) => {
                write!(f, "Cannot resolve stable metadata for {target}")
            }
            Self::InvalidArchitecture(architecture) => {
                write!(f, "Unsupported metadata architecture: {architecture}")
            }
            Self::InvalidAlpineMetadata => f.write_str("Invalid Alpine release metadata"),
            Self::InvalidDebianMetadata => f.write_str("Invalid Debian release metadata"),
            Self::FetchFailed { url, status } => {
                write!(
                    f,
                    "Cannot fetch stable metadata from {url} (status {status:?})"
                )
            }
            Self::InvalidUtf8 => f.write_str("Stable release metadata is not UTF-8"),
            Self::Command(error) => write!(f, "Metadata command failed: {error}"),
        }
    }
}

impl std::error::Error for MetadataError {}

pub fn parse_alpine_release_metadata(metadata: &str) -> Result<String, MetadataError> {
    let versions: BTreeSet<String> = metadata
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("version:"))
        .filter_map(|value| parse_alpine_version(value.trim()))
        .collect();
    if versions.len() != 1 {
        return Err(MetadataError::InvalidAlpineMetadata);
    }
    Ok(versions.into_iter().next().expect("one version"))
}

fn parse_alpine_version(value: &str) -> Option<String> {
    let mut components = value.split('.');
    let major = components.next()?;
    let minor = components.next()?;
    let patch = components.next()?;
    if components.next().is_some()
        || [major, minor, patch].iter().any(|component| {
            component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return None;
    }
    Some(format!("{major}.{minor}"))
}

pub fn parse_debian_release_metadata(metadata: &str) -> Result<StableRelease, MetadataError> {
    let versions: Vec<&str> = metadata
        .lines()
        .filter_map(|line| line.strip_prefix("Version: "))
        .collect();
    let codenames: Vec<&str> = metadata
        .lines()
        .filter_map(|line| line.strip_prefix("Codename: "))
        .collect();
    if versions.len() != 1 || codenames.len() != 1 {
        return Err(MetadataError::InvalidDebianMetadata);
    }

    let version = versions[0];
    let codename = codenames[0];
    if version.is_empty()
        || version.split('.').any(|component| {
            component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit())
        })
        || codename.is_empty()
        || !codename.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        return Err(MetadataError::InvalidDebianMetadata);
    }

    Ok(StableRelease::Debian {
        version: version
            .split('.')
            .next()
            .expect("nonempty version")
            .to_owned(),
        codename: codename.to_owned(),
    })
}

type ReleaseParser = fn(&str) -> Result<StableRelease, MetadataError>;

pub fn fetch_stable_release<R: CommandRunner>(
    runner: &mut R,
    target: Target,
    basearch: &str,
) -> Result<StableRelease, MetadataError> {
    if basearch != "x86_64" {
        return Err(MetadataError::InvalidArchitecture(basearch.to_owned()));
    }
    let (url, parser): (&str, ReleaseParser) = match target {
        Target::Alpine => (ALPINE_RELEASES_URL, |body| {
            parse_alpine_release_metadata(body).map(|version| StableRelease::Alpine { version })
        }),
        Target::Debian => (DEBIAN_RELEASE_URL, parse_debian_release_metadata),
        _ => return Err(MetadataError::UnsupportedTarget(target)),
    };

    let body = fetch_text(runner, url)?;
    parser(&body)
}

fn fetch_text<R: CommandRunner>(runner: &mut R, url: &str) -> Result<String, MetadataError> {
    let mut last_status = None;
    for attempt in 1..=5 {
        let command = CommandSpec::new("curl", ["--connect-timeout", "10", "-f", "-L", url])
            .phase("resolve-stable-release")
            .stdin(CommandInput::Null)
            .stdout_mode(OutputMode::Capture)
            .stderr_mode(OutputMode::Stream);
        let output = match runner.run(&command) {
            Ok(output) => output,
            Err(error) if attempt == 5 => {
                return Err(MetadataError::Command(error));
            }
            Err(_) => {
                sleep_one_second(runner)?;
                continue;
            }
        };

        last_status = output.status;
        if output.status == Some(0) {
            return String::from_utf8(output.stdout).map_err(|_| MetadataError::InvalidUtf8);
        }
        if output.status == Some(22) || attempt == 5 {
            return Err(MetadataError::FetchFailed {
                url: url.to_owned(),
                status: output.status,
            });
        }
        sleep_one_second(runner)?;
    }
    Err(MetadataError::FetchFailed {
        url: url.to_owned(),
        status: last_status,
    })
}

fn sleep_one_second<R: CommandRunner>(runner: &mut R) -> Result<(), MetadataError> {
    let command = CommandSpec::new("sleep", ["1"])
        .phase("resolve-stable-release-retry")
        .stdin(CommandInput::Null)
        .stdout_mode(OutputMode::Discard)
        .stderr_mode(OutputMode::Discard);
    runner.run(&command).map_err(MetadataError::Command)?;
    Ok(())
}
