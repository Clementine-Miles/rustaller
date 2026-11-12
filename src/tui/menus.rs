use super::fields::{Field, TCP_FIELDS};
use super::state::{
    Action, Authentication, CHOICES, DnsProvider, EditSlot, Entry, IsoMode, Page, Setting, Wizard,
    boot_label, hold_label, iso_label, optional_label, yes_no,
};
use rustaller::config::{AddressPreference, CongestionControl, Ipv6Policy, QueueDiscipline};
use rustaller::config::{DnsMode, InstallProfile, RootFilesystem};
use rustaller::host::ssh_keygen::{self, KeyType};
use rustaller::model::{BootMode, Target};
use rustaller::postinstall::KernelChoice;

impl Wizard {
    pub fn entries(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        let mut add = |label: String, action| entries.push(Entry { label, action });
        let draft = self.draft();
        match self.page() {
            Page::Home => {
                add("Install a system  >".into(), Action::Open(Page::Targets));
                add("Configure current system  >".into(), Action::Current);
                add("Quit".into(), Action::Quit);
            }
            Page::Targets => {
                for (index, (label, _)) in CHOICES.iter().enumerate() {
                    add((*label).into(), Action::Target(index));
                }
            }
            Page::Overview => {
                if self.has_account() {
                    add("Account and SSH  ›".into(), Action::Open(Page::Account));
                }
                if self.is_automatic() {
                    add(
                        "Installation profile  ›".into(),
                        Action::Open(Page::Installation),
                    );
                }
                if self.has_system_settings() {
                    add(
                        if self.is_current() {
                            "Host settings  ›"
                        } else {
                            "New host settings  ›"
                        }
                        .into(),
                        Action::Open(Page::Host),
                    );
                    add("Network and DNS  ›".into(), Action::Open(Page::Network));
                }
                if self.is_automatic() {
                    add("Target swapfile  ›".into(), Action::Open(Page::Swap));
                }
                if self.choice == 4 {
                    add("ISO media  ›".into(), Action::Open(Page::Iso));
                }
                if self.choice != 7 && !self.is_current() {
                    add("Disk and boot  ›".into(), Action::Open(Page::Boot));
                }
                if self.has_account() && !self.is_current() {
                    add(
                        "Resources and logs  ›".into(),
                        Action::Open(Page::Resources),
                    );
                }
                if self.choice == 7 {
                    add(
                        format!("Debug output: {}", yes_no(draft.debug)),
                        Action::ToggleDebug,
                    );
                }
                add(
                    if self.is_current() {
                        "Review and apply configuration  >"
                    } else {
                        "Review installation  >"
                    }
                    .into(),
                    Action::Review,
                );
            }
            Page::Account => {
                add(
                    format!(
                        "{}: {}",
                        if self.is_current() {
                            "Existing user"
                        } else {
                            "Username"
                        },
                        draft.display(Field::Username)
                    ),
                    Action::Edit(EditSlot::Field(Field::Username)),
                );
                if self.is_current() {
                    add(
                        format!("Public keys for existing user: {}  >", draft.keys.len()),
                        Action::Open(Page::Keys),
                    );
                } else {
                    add(
                        format!(
                            "Authentication: {}  ›",
                            if draft.authentication == Authentication::Password {
                                "Password"
                            } else {
                                "SSH public keys"
                            }
                        ),
                        Action::Open(Page::Auth),
                    );
                }
                add(
                    format!("SSH port: {}", draft.display(Field::SshPort)),
                    Action::Edit(EditSlot::Field(Field::SshPort)),
                );
                if self.is_automatic() && draft.get(Field::Username).trim() != "root" {
                    add(
                        "Root account password (required)  ›".into(),
                        Action::Open(Page::RootPasswords),
                    );
                }
            }
            Page::Auth => {
                add(
                    format!(
                        "Authentication method: {}",
                        if draft.authentication == Authentication::Password {
                            "Password"
                        } else {
                            "SSH public keys"
                        }
                    ),
                    Action::Select(Setting::Authentication),
                );
                if draft.authentication == Authentication::Password {
                    add("Password settings  ›".into(), Action::Open(Page::Passwords));
                } else {
                    add(
                        format!("Public keys: {} items  >", draft.keys.len()),
                        Action::Open(Page::Keys),
                    );
                }
            }
            Page::Passwords => {
                for field in [Field::Password, Field::Confirm] {
                    add(
                        format!("{}: {}", field.label(), draft.display(field)),
                        Action::Edit(EditSlot::Field(field)),
                    );
                }
            }
            Page::RootPasswords => {
                for field in [Field::RootPassword, Field::RootConfirm] {
                    add(
                        format!("{}: {}", field.label(), draft.display(field)),
                        Action::Edit(EditSlot::Field(field)),
                    );
                }
            }
            Page::Keys => {
                for (index, _) in draft.keys.iter().enumerate() {
                    add(
                        format!("Public key {} (hidden)  >", index + 1),
                        Action::Open(Page::KeyActions(index)),
                    );
                }
                add(
                    "Generate key (system ssh-keygen)  >".into(),
                    Action::Open(Page::Keygen),
                );
                add("Add public key  +".into(), Action::Edit(EditSlot::NewKey));
            }
            Page::KeyActions(index) => {
                add(
                    format!("Edit public key {}", index + 1),
                    Action::Edit(EditSlot::Key(index)),
                );
                add(
                    format!("Delete public key {}", index + 1),
                    Action::DeleteKey(index),
                );
            }
            Page::Host => {
                for field in [Field::Hostname, Field::Timezone] {
                    add(
                        format!("{}: {}", field.label(), draft.display(field)),
                        Action::Edit(EditSlot::Field(field)),
                    );
                }
                if !self.is_current() {
                    add(
                        format!("Kernel: {}", self.kernel_label()),
                        Action::Select(Setting::Kernel),
                    );
                }
            }
            Page::Installation => {
                add(
                    format!("Install profile: {}", draft.profile.as_str()),
                    Action::Select(Setting::Profile),
                );
                add(
                    format!("Add curl / less: {}", yes_no(draft.with_tools)),
                    Action::ToggleTools,
                );
                if draft.profile == InstallProfile::Minimal {
                    add(
                        format!(
                            "Keep documentation/translations: {}",
                            yes_no(draft.keep_docs)
                        ),
                        Action::ToggleKeepDocs,
                    );
                    add(
                        format!("Keep package cache: {}", yes_no(draft.keep_package_cache)),
                        Action::ToggleKeepCache,
                    );
                }
            }
            Page::Network => {
                for page in [Page::Dns, Page::Tcp, Page::IpStack, Page::Mtu] {
                    add(format!("{}  ›", page.title()), Action::Open(page));
                }
            }
            Page::Dns => {
                add(
                    format!(
                        "DNS provider: {}",
                        if draft.dns_mode == DnsMode::Auto {
                            "Automatic / unchanged"
                        } else {
                            draft.dns_provider.label()
                        }
                    ),
                    Action::Select(Setting::DnsMode),
                );
                if draft.dns_mode == DnsMode::Static {
                    if draft.dns_provider == DnsProvider::Custom {
                        add(
                            format!(
                                "{}: {}",
                                Field::DnsServers.label(),
                                draft.display(Field::DnsServers)
                            ),
                            Action::Edit(EditSlot::Field(Field::DnsServers)),
                        );
                    }
                    add(
                        format!(
                            "{}: {}",
                            Field::DnsSearch.label(),
                            draft.display(Field::DnsSearch)
                        ),
                        Action::Edit(EditSlot::Field(Field::DnsSearch)),
                    );
                }
            }
            Page::Tcp => {
                add(
                    format!("Congestion control: {}", draft.congestion.as_str()),
                    Action::Select(Setting::Congestion),
                );
                add(
                    format!("Default queue: {}", draft.qdisc.as_str()),
                    Action::Select(Setting::Qdisc),
                );
                for field in TCP_FIELDS {
                    add(
                        format!(
                            "{}: {}",
                            field.label(),
                            optional_label(draft.get(*field), "Kernel default")
                        ),
                        Action::Edit(EditSlot::Field(*field)),
                    );
                }
                add(
                    "Fill reference TCP values (editable draft)".into(),
                    Action::TcpPreset,
                );
                add("Clear TCP draft overrides".into(), Action::ResetTcp);
            }
            Page::IpStack => {
                add(
                    format!("IPv6: {}", draft.ipv6.as_str()),
                    Action::Select(Setting::Ipv6),
                );
                if self.selected_target() != Some(Target::Alpine) {
                    add(
                        format!(
                            "glibc address preference: {}",
                            draft.address_preference.as_str()
                        ),
                        Action::Select(Setting::Preference),
                    );
                }
            }
            Page::Mtu => {
                add(
                    format!(
                        "MTU: {}",
                        optional_label(draft.get(Field::Mtu), "Keep interface configuration")
                    ),
                    Action::Edit(EditSlot::Field(Field::Mtu)),
                );
            }
            Page::Swap => {
                add(
                    format!("Create target /swapfile: {}", yes_no(draft.swap_enabled)),
                    Action::ToggleSwap,
                );
                if draft.swap_enabled {
                    for field in [Field::SwapSize, Field::Swappiness] {
                        add(
                            format!(
                                "{}: {}",
                                field.label(),
                                optional_label(
                                    draft.get(field),
                                    if field == Field::SwapSize {
                                        "Required: enter MiB"
                                    } else {
                                        "Kernel default"
                                    }
                                )
                            ),
                            Action::Edit(EditSlot::Field(field)),
                        );
                    }
                }
            }
            Page::Keygen => {
                add(
                    format!("Key type: {}", draft.key_type.as_str()),
                    Action::Select(Setting::KeyType),
                );
                for field in [Field::KeyRounds, Field::KeyComment, Field::KeyPath] {
                    add(
                        format!("{}: {}", field.label(), draft.display(field)),
                        Action::Edit(EditSlot::Field(field)),
                    );
                }
                add(
                    format!(
                        "Private key cipher (-Z): {}",
                        ssh_keygen::CIPHERS[draft.key_cipher]
                    ),
                    Action::Select(Setting::KeyCipher),
                );
                if draft.key_type == KeyType::Rsa {
                    add(
                        format!("RSA bits: {}", draft.display(Field::KeyBits)),
                        Action::Edit(EditSlot::Field(Field::KeyBits)),
                    );
                }
                add(
                    "Generate and add public key (writes local key files)".into(),
                    Action::GenerateKey,
                );
            }
            Page::Boot => {
                if self.is_automatic() {
                    add(
                        format!("Target disk: {}", draft.display(Field::Disk)),
                        Action::Edit(EditSlot::Field(Field::Disk)),
                    );
                    add(
                        format!("Root filesystem: {}", draft.root_filesystem.as_str()),
                        Action::Select(Setting::RootFilesystem),
                    );
                }
                add(
                    format!("Boot mode: {}", boot_label(draft.boot)),
                    Action::Select(Setting::Boot),
                );
                if self.is_automatic() {
                    add(
                        format!("Hold behavior: {}", hold_label(draft.hold)),
                        Action::Select(Setting::Hold),
                    );
                }
                if self.choice == 5 {
                    add(
                        format!("Generic kernel: {}", yes_no(draft.generic_kernel)),
                        Action::ToggleGeneric,
                    );
                }
                if matches!(self.choice, 4 | 6) {
                    add(
                        format!("Debug output: {}", yes_no(draft.debug)),
                        Action::ToggleDebug,
                    );
                }
            }
            Page::Resources => {
                for field in [Field::Confhome, Field::WebPort, Field::Frpc] {
                    add(
                        format!("{}: {}", field.label(), draft.display(field)),
                        Action::Edit(EditSlot::Field(field)),
                    );
                }
                add(
                    format!("Debug output: {}", yes_no(draft.debug)),
                    Action::ToggleDebug,
                );
            }
            Page::Iso => {
                add(
                    format!("ISO path: {}", draft.display(Field::IsoPath)),
                    Action::Edit(EditSlot::Field(Field::IsoPath)),
                );
                add(
                    format!("Boot method: {}", iso_label(draft.iso_mode)),
                    Action::Select(Setting::IsoMode),
                );
                let fields: &[Field] = if draft.iso_mode == IsoMode::Loopback {
                    &[Field::IsoConfig]
                } else {
                    &[Field::IsoKernel, Field::IsoInitrd, Field::IsoCmdline]
                };
                for field in fields {
                    add(
                        format!("{}: {}", field.label(), draft.display(*field)),
                        Action::Edit(EditSlot::Field(*field)),
                    );
                }
            }
        }
        if self.page() != Page::Home {
            add("Back to previous menu".into(), Action::Back);
        }
        entries
    }
    pub fn choices(&self, setting: Setting) -> Vec<&'static str> {
        match setting {
            Setting::Authentication => vec!["Password", "SSH public keys"],
            Setting::Boot => vec!["Auto-detect", "BIOS", "UEFI"],
            Setting::Hold => vec![
                "Default behavior",
                "Wait for confirmation after preparation (hold=0)",
                "Keep Live environment after installation (hold=2)",
            ],
            Setting::IsoMode => vec![
                "ISO-provided loopback configuration",
                "Explicit kernel and initrd",
            ],
            Setting::Profile => vec![
                "standard (existing defaults)",
                "minimal (trim documentation/cache)",
            ],
            Setting::DnsMode => vec![
                "auto (automatic / unchanged DNS)",
                "Custom DNS (IPv4/IPv6 addresses)",
                "Cloudflare (1.1.1.1 / 1.0.0.1)",
                "Google (8.8.8.8 / 8.8.4.4)",
                "Quad9 (9.9.9.9 / 149.112.112.112)",
            ],
            Setting::RootFilesystem => vec!["xfs", "ext4"],
            Setting::Congestion => CongestionControl::ALL.iter().map(|v| v.as_str()).collect(),
            Setting::Qdisc => QueueDiscipline::ALL.iter().map(|v| v.as_str()).collect(),
            Setting::Ipv6 => Ipv6Policy::ALL.iter().map(|v| v.as_str()).collect(),
            Setting::Preference => {
                vec!["default (system defaults)", "ipv4 (glibc IPv4 preference)"]
            }
            Setting::KeyType => vec!["ed25519", "rsa"],
            Setting::KeyCipher => ssh_keygen::CIPHERS.to_vec(),
            Setting::Kernel => KernelChoice::choices_for(match self.choice {
                0 => Target::Alpine,
                1 => Target::Debian,
                2 => Target::Arch,
                _ => Target::Gentoo,
            })
            .split('/')
            .collect(),
        }
    }
    pub(super) fn setting_index(&self, setting: Setting) -> usize {
        let draft = self.draft();
        match setting {
            Setting::Authentication => {
                usize::from(draft.authentication == Authentication::PublicKeys)
            }
            Setting::Boot => match draft.boot {
                None => 0,
                Some(BootMode::Bios) => 1,
                Some(BootMode::Efi) => 2,
            },
            Setting::Hold => match draft.hold {
                Some(0) => 1,
                Some(2) => 2,
                _ => 0,
            },
            Setting::IsoMode => usize::from(draft.iso_mode == IsoMode::Kernel),
            Setting::Profile => usize::from(draft.profile == InstallProfile::Minimal),
            Setting::DnsMode => {
                if draft.dns_mode == DnsMode::Auto {
                    0
                } else {
                    match draft.dns_provider {
                        DnsProvider::Custom => 1,
                        DnsProvider::Cloudflare => 2,
                        DnsProvider::Google => 3,
                        DnsProvider::Quad9 => 4,
                    }
                }
            }
            Setting::RootFilesystem => usize::from(draft.root_filesystem == RootFilesystem::Ext4),
            Setting::Congestion => CongestionControl::ALL
                .iter()
                .position(|v| *v == draft.congestion)
                .unwrap_or(0),
            Setting::Qdisc => QueueDiscipline::ALL
                .iter()
                .position(|v| *v == draft.qdisc)
                .unwrap_or(0),
            Setting::Ipv6 => Ipv6Policy::ALL
                .iter()
                .position(|v| *v == draft.ipv6)
                .unwrap_or(0),
            Setting::Preference => usize::from(draft.address_preference == AddressPreference::Ipv4),
            Setting::KeyType => usize::from(draft.key_type == KeyType::Rsa),
            Setting::KeyCipher => draft.key_cipher,
            Setting::Kernel => self
                .choices(setting)
                .iter()
                .position(|value| *value == draft.kernel.as_str())
                .unwrap_or(0),
        }
    }
    pub(super) fn select(&mut self, setting: Setting, index: usize) {
        let kernel = if matches!(setting, Setting::Kernel) {
            self.choices(setting)[index].parse().ok()
        } else {
            None
        };
        let draft = self.draft_mut();
        match setting {
            Setting::Congestion => {
                draft.congestion = CongestionControl::ALL[index];
                if draft.congestion == CongestionControl::Bbr {
                    draft.qdisc = QueueDiscipline::Fq;
                }
            }
            Setting::Qdisc => draft.qdisc = QueueDiscipline::ALL[index],
            Setting::Ipv6 => draft.ipv6 = Ipv6Policy::ALL[index],
            Setting::Preference => {
                draft.address_preference = if index == 0 {
                    AddressPreference::Default
                } else {
                    AddressPreference::Ipv4
                }
            }
            Setting::KeyType => {
                draft.key_type = if index == 0 {
                    KeyType::Ed25519
                } else {
                    KeyType::Rsa
                }
            }
            Setting::KeyCipher => draft.key_cipher = index,
            Setting::Profile => {
                draft.profile = if index == 0 {
                    InstallProfile::Standard
                } else {
                    InstallProfile::Minimal
                }
            }
            Setting::DnsMode => {
                draft.dns_mode = if index == 0 {
                    DnsMode::Auto
                } else {
                    DnsMode::Static
                };
                draft.dns_provider = match index {
                    2 => DnsProvider::Cloudflare,
                    3 => DnsProvider::Google,
                    4 => DnsProvider::Quad9,
                    _ => DnsProvider::Custom,
                };
            }
            Setting::RootFilesystem => {
                draft.root_filesystem = if index == 0 {
                    RootFilesystem::Xfs
                } else {
                    RootFilesystem::Ext4
                }
            }
            Setting::Authentication => {
                draft.authentication = if index == 0 {
                    Authentication::Password
                } else {
                    Authentication::PublicKeys
                }
            }
            Setting::Boot => {
                draft.boot = match index {
                    1 => Some(BootMode::Bios),
                    2 => Some(BootMode::Efi),
                    _ => None,
                }
            }
            Setting::Hold => {
                draft.hold = match index {
                    1 => Some(0),
                    2 => Some(2),
                    _ => None,
                }
            }
            Setting::IsoMode => {
                draft.iso_mode = if index == 0 {
                    IsoMode::Loopback
                } else {
                    IsoMode::Kernel
                }
            }
            Setting::Kernel => {
                if let Some(kernel) = kernel {
                    draft.kernel = kernel;
                }
            }
        }
    }
}
