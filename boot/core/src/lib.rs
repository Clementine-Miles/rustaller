//! The versioned Linux boot policy shared by the installer and firmware.
#![no_std]

extern crate alloc;
use alloc::{collections::BTreeMap, format, string::String, vec::Vec};
use core::fmt;
use serde::Deserialize;

pub const MAX_CONFIG_BYTES: usize = 32 * 1024;
pub const MAX_CMDLINE_BYTES: usize = 2047;
pub const CONFIG_PATH: &str = "/rustaller/boot.toml";

#[derive(Debug, PartialEq, Eq)]
pub struct Error(&'static str);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl core::error::Error for Error {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub version: Option<String>,
    pub kernel: String,
    /// Ordered Linux external initrd stream: early microcode first, main initramfs last.
    pub initrds: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Boot {
    pub default: String,
    pub timeout_seconds: u8,
    pub serial: bool,
    pub serial_baudrate: u32,
}
impl Default for Boot {
    fn default() -> Self {
        Self {
            default: "current".into(),
            timeout_seconds: 3,
            serial: true,
            serial_baudrate: 115200,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Linux {
    pub root: String,
    pub rootfstype: String,
    pub rootflags: Option<String>,
    #[serde(default = "read_write")]
    pub mode: String,
    #[serde(default = "default_consoles")]
    pub consoles: Vec<String>,
    #[serde(default)]
    pub extra_cmdline: String,
    pub cmdline_override: Option<String>,
}
fn read_write() -> String {
    "rw".into()
}
fn default_consoles() -> Vec<String> {
    alloc::vec!["ttyS0,115200n8".into(), "tty0".into()]
}

#[derive(Debug)]
pub struct Config {
    pub boot: Boot,
    pub linux: Linux,
    pub entries: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    #[serde(default)]
    boot: Boot,
    linux: Linux,
    entries: BTreeMap<String, Paths>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Paths {
    version: Option<String>,
    kernel: String,
    initrds: Vec<String>,
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, Error> {
        if text.len() > MAX_CONFIG_BYTES {
            return Err(Error("boot.toml exceeds 32 KiB"));
        }
        let document: Document =
            toml::from_str(text).map_err(|_| Error("invalid boot.toml syntax, field or type"))?;
        if document.schema_version != 1 {
            return Err(Error("unsupported boot.toml schema_version"));
        }
        if document.boot.timeout_seconds > 60
            || !matches!(
                document.boot.serial_baudrate,
                9600 | 19200 | 38400 | 57600 | 115200
            )
        {
            return Err(Error("invalid boot timeout or serial baudrate"));
        }
        let entries = document
            .entries
            .into_iter()
            .map(|(name, paths)| Entry {
                name,
                version: paths.version,
                kernel: paths.kernel,
                initrds: paths.initrds,
            })
            .collect();
        let config = Self {
            boot: document.boot,
            linux: document.linux,
            entries,
        };
        validate_entries(&config.entries)?;
        if config.entry(&config.boot.default).is_none() {
            return Err(Error("configured default boot entry is absent"));
        }
        config.cmdline()?;
        Ok(config)
    }

    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    pub fn default_entry(&self) -> &Entry {
        self.entry(&self.boot.default)
            .expect("default entry validated on parse")
    }

    /// Only one older generation is eligible after a recoverable load error.
    /// Do not retry a manually selected previous entry or identical payloads.
    pub fn fallback_entry(&self, selected: &Entry) -> Option<&Entry> {
        self.entry("previous").filter(|previous| {
            selected.name != "previous"
                && (previous.kernel != selected.kernel || previous.initrds != selected.initrds)
        })
    }

    pub fn cmdline(&self) -> Result<String, Error> {
        let linux = &self.linux;
        let uuid = linux
            .root
            .strip_prefix("UUID=")
            .ok_or(Error("root must be UUID=..."))?;
        if uuid.is_empty()
            || uuid.len() > 64
            || !uuid.bytes().any(|c| c.is_ascii_hexdigit())
            || !uuid.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-')
            || !matches!(linux.rootfstype.as_str(), "ext4" | "btrfs" | "xfs")
            || !matches!(linux.mode.as_str(), "ro" | "rw")
        {
            return Err(Error("invalid root, rootfstype or mode"));
        }
        let rootflags = linux
            .rootflags
            .as_deref()
            .unwrap_or(if linux.rootfstype == "btrfs" {
                "subvol=@"
            } else {
                ""
            });
        if rootflags.len() > 256
            || !rootflags
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b",=_:@./+-".contains(&c))
        {
            return Err(Error("invalid rootflags"));
        }
        if linux.consoles.len() > 4 {
            return Err(Error("too many console arguments"));
        }
        let mut args = format!("root={} rootfstype={}", linux.root, linux.rootfstype);
        if !rootflags.is_empty() {
            args.push_str(" rootflags=");
            args.push_str(rootflags);
        }
        args.push(' ');
        args.push_str(&linux.mode);
        for (i, console) in linux.consoles.iter().enumerate() {
            let device = console.split(',').next().unwrap_or_default();
            if device.is_empty()
                || console.len() > 64
                || !console
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b',')
                || linux.consoles[..i]
                    .iter()
                    .any(|v| v.split(',').next() == Some(device))
            {
                return Err(Error("invalid or repeated Linux console"));
            }
            args.push_str(" console=");
            args.push_str(console);
        }
        // A full override must be explicit. Both paths are passed as literal ASCII
        // to the kernel; no shell quoting or expansion occurs.
        if let Some(line) = &linux.cmdline_override {
            if !valid_cmdline(line) || line.trim().is_empty() {
                return Err(Error("invalid full Linux command-line override"));
            }
            return Ok(line.clone());
        }
        if !valid_cmdline(&linux.extra_cmdline) {
            return Err(Error("invalid extra kernel command line"));
        }
        // Linux permits double quotes around parameters (including around the
        // whole token). Reject attempts to override managed keys even if quoted.
        for token in linux.extra_cmdline.split_ascii_whitespace() {
            let key = token.split('=').next().unwrap_or("").trim_matches('"');
            if matches!(
                key,
                "root" | "rootfstype" | "rootflags" | "ro" | "rw" | "console" | "initrd"
            ) {
                return Err(Error(
                    "use the corresponding Linux field or cmdline_override for managed arguments",
                ));
            }
        }
        if !linux.extra_cmdline.trim().is_empty() {
            args.push(' ');
            args.push_str(linux.extra_cmdline.trim());
        }
        if args.len() > MAX_CMDLINE_BYTES {
            return Err(Error("Linux command line exceeds 2047 bytes"));
        }
        Ok(args)
    }
}
fn valid_cmdline(value: &str) -> bool {
    value.len() <= MAX_CMDLINE_BYTES && value.bytes().all(|c| (0x20..=0x7e).contains(&c))
}

pub fn valid_boot_path(path: &str) -> bool {
    path.len() <= 240
        && path.strip_prefix('/').is_some_and(|p| {
            p.split('/').all(|part| {
                !part.is_empty()
                    && part.len() <= 128
                    && !part.ends_with('.')
                    && part
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-.+".contains(&c))
            })
        })
}

fn validate_entries(entries: &[Entry]) -> Result<(), Error> {
    if entries.is_empty() || entries.len() > 16 || !entries.iter().any(|e| e.name == "current") {
        return Err(Error(
            "require a current generation and at most 16 kernel entries",
        ));
    }
    for (index, entry) in entries.iter().enumerate() {
        if entry.name.is_empty()
            || entry.name.len() > 80
            || !entry
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            || entries[..index].iter().any(|e| e.name == entry.name)
            || entry.version.as_ref().is_some_and(|v| {
                v.is_empty()
                    || v.len() > 128
                    || !v
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_.+-".contains(&c))
            })
            || !valid_boot_path(&entry.kernel)
            || !(1..=3).contains(&entry.initrds.len())
        {
            return Err(Error(
                "invalid boot entry name, kernel path or initrd count",
            ));
        }
        for (i, initrd) in entry.initrds.iter().enumerate() {
            if !valid_boot_path(initrd)
                || initrd.eq_ignore_ascii_case(&entry.kernel)
                || entry.initrds[..i]
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(initrd))
            {
                return Err(Error("invalid or repeated initrd path"));
            }
        }
    }
    Ok(())
}

#[cfg(feature = "edit")]
mod edit;
#[cfg(feature = "edit")]
pub use edit::{create, replace_entries, replace_entries_for_root};

mod efi;
pub use efi::{valid_bzimage, valid_efi_stub};

#[cfg(test)]
mod tests {
    use super::*;
    const POLICY: &str = r#"schema_version = 1
[linux]
root = "UUID=1234-abcd"
rootfstype = "btrfs"
[entries.current]
kernel = "/generation/vmlinuz"
initrds = ["/generation/early", "/generation/initramfs"]
"#;
    #[test]
    fn generated_arguments_and_explicit_override() {
        let cfg = Config::parse(POLICY).unwrap();
        assert_eq!(
            cfg.cmdline().unwrap(),
            "root=UUID=1234-abcd rootfstype=btrfs rootflags=subvol=@ rw console=ttyS0,115200n8 console=tty0"
        );
        assert_eq!(cfg.default_entry().initrds[0], "/generation/early");
        let advanced = POLICY.replace(
            "[entries.current]",
            "cmdline_override='root=/dev/test ro panic=9'\n[entries.current]",
        );
        assert_eq!(
            Config::parse(&advanced).unwrap().cmdline().unwrap(),
            "root=/dev/test ro panic=9"
        );
        for extra in [
            "rw",
            "ro",
            "root=/dev/x",
            "console=tty2",
            "rootflags=x",
            "initrd=/x",
            "\"root=/dev/x\"",
        ] {
            assert!(
                Config::parse(&POLICY.replace(
                    "[entries.current]",
                    &format!("extra_cmdline='{extra}'\n[entries.current]")
                ))
                .is_err()
            );
        }
    }
    #[test]
    fn only_a_distinct_previous_payload_is_eligible_for_one_fallback() {
        let text = format!(
            "{POLICY}\n[entries.previous]\nkernel='/older/kernel'\ninitrds=['/older/initrd']\n"
        );
        let cfg = Config::parse(&text).unwrap();
        let previous = cfg.fallback_entry(cfg.entry("current").unwrap()).unwrap();
        assert_eq!(previous.name, "previous");
        assert!(cfg.fallback_entry(previous).is_none());
        let identical = text
            .replace("/older/kernel", "/generation/vmlinuz")
            .replace(
                "initrds=['/older/initrd']",
                "initrds=['/generation/early','/generation/initramfs']",
            );
        let cfg = Config::parse(&identical).unwrap();
        assert!(cfg.fallback_entry(cfg.entry("current").unwrap()).is_none());
        assert!(
            Config::parse(POLICY)
                .unwrap()
                .fallback_entry(&Entry::default())
                .is_none()
        );
    }
    #[cfg(feature = "edit")]
    #[test]
    fn identity_refresh_retains_custom_entry_comments_default_and_explicit_override() {
        let text=POLICY.replace("[linux]","[boot]\ndefault='rescue' # chosen entry\n[linux]\ncmdline_override='root=/dev/custom ro' # explicit override")
            .replace("root = \"UUID=1234-abcd\"","root = \"UUID=1234-abcd\" # recorded root")
            + "\n[entries.rescue] # custom menu\nkernel='/custom/kernel' # custom image\ninitrds=['/custom/initrd'] # custom stream\n";
        let cfg = Config::parse(&text).unwrap();
        let updated =
            replace_entries_for_root(&text, &cfg.entries, "UUID=abcd-5678", "xfs").unwrap();
        for comment in [
            "# chosen entry",
            "# explicit override",
            "# recorded root",
            "# custom menu",
            "# custom image",
            "# custom stream",
        ] {
            assert!(updated.contains(comment), "{comment}: {updated}");
        }
        let cfg = Config::parse(&updated).unwrap();
        assert_eq!(cfg.boot.default, "rescue");
        assert_eq!(cfg.linux.root, "UUID=abcd-5678");
        assert_eq!(cfg.linux.rootfstype, "xfs");
        assert_eq!(cfg.cmdline().unwrap(), "root=/dev/custom ro");
    }
    #[test]
    fn parser_bounds_ambiguity_and_paths() {
        for bad in [
            POLICY.replace("= 1", "= 2"),
            POLICY.replace("= 1", "= 1\nschema_version = 1"),
            POLICY.replace("[linux]", "[boot]\ndefault='missing'\n[linux]"),
            POLICY.replace("[linux]", "[boot]\ntimeout_seconds=200\n[linux]"),
            POLICY.replace("[linux]", "unknown=4\n[linux]"),
            POLICY.replace("/generation/vmlinuz", "/generation/../kernel"),
            POLICY.replace("/generation/early", "/GENERATION/INITRAMFS"),
            POLICY.repeat(300),
        ] {
            assert!(Config::parse(&bad).is_err());
        }
    }
    #[cfg(feature = "edit")]
    #[test]
    fn publication_preserves_comments_and_user_policy() {
        let text=POLICY.replace("[linux]", "[boot]\ntimeout_seconds=12 # slow serial console\n[linux]\nextra_cmdline='panic=3' # keep");
        let mut current = Config::parse(&text).unwrap().default_entry().clone();
        current.kernel = "/new/kernel".into();
        let updated = replace_entries(&text, &[current]).unwrap();
        assert!(updated.contains("timeout_seconds=12 # slow serial console"));
        assert!(updated.contains("extra_cmdline='panic=3' # keep"));
        assert!(
            Config::parse(&updated)
                .unwrap()
                .cmdline()
                .unwrap()
                .ends_with("panic=3")
        );
    }
}
