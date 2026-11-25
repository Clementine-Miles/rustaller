use super::editor::TextValue;
use super::fields::Field;
use super::state::clean;
use rustaller::config::{
    AddressPreference, CongestionControl, DnsMode, InstallProfile, Ipv6Policy, QueueDiscipline,
    RootFilesystem,
};
use rustaller::host::ssh_keygen::KeyType;
use rustaller::model::{BootMode, DEFAULT_ACCOUNT};
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum DnsProvider {
    Custom,
    Cloudflare,
    Google,
    Quad9,
}
impl DnsProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Custom => "Custom",
            Self::Cloudflare => "Cloudflare",
            Self::Google => "Google",
            Self::Quad9 => "Quad9",
        }
    }
}
#[derive(Clone, Eq, PartialEq)]
pub(super) struct Draft {
    pub values: Vec<std::rc::Rc<TextValue>>,
    pub keys: Vec<TextValue>,
    pub boot: Option<BootMode>,
    pub hold: Option<u8>,
    pub debug: bool,
    pub profile: InstallProfile,
    pub dns_mode: DnsMode,
    pub dns_provider: DnsProvider,
    pub explicit_dhcp4: bool,
    pub explicit_dhcp6: bool,
    pub explicit_ra: bool,
    pub root_filesystem: RootFilesystem,
    pub with_tools: bool,
    pub keep_docs: bool,
    pub keep_package_cache: bool,
    pub congestion: CongestionControl,
    pub qdisc: QueueDiscipline,
    pub ipv6: Ipv6Policy,
    pub address_preference: AddressPreference,
    pub swap_enabled: bool,
    pub key_type: KeyType,
    pub key_cipher: usize,
}
impl Draft {
    pub fn new(comment: &str, key_path: &str) -> Self {
        let mut draft = Self {
            values: (0..=Field::KeyBits as usize)
                .map(|_| std::rc::Rc::new(TextValue::new("")))
                .collect(),
            keys: Vec::new(),
            boot: None,
            hold: None,
            debug: false,
            profile: InstallProfile::default(),
            dns_mode: DnsMode::Auto,
            dns_provider: DnsProvider::Custom,
            explicit_dhcp4: false,
            explicit_dhcp6: false,
            explicit_ra: false,
            root_filesystem: RootFilesystem::Xfs,
            with_tools: false,
            keep_docs: false,
            keep_package_cache: false,
            congestion: CongestionControl::Default,
            qdisc: QueueDiscipline::Default,
            ipv6: Ipv6Policy::Inherit,
            address_preference: AddressPreference::Default,
            swap_enabled: false,
            key_type: KeyType::Ed25519,
            key_cipher: 0,
        };
        for (field, value) in [
            (Field::Username, DEFAULT_ACCOUNT),
            (Field::SshPort, "22"),
            (Field::SwapSize, "2048"),
            (Field::KeyPath, key_path),
            (Field::KeyRounds, "512"),
            (Field::KeyComment, comment),
            (Field::KeyBits, "4096"),
        ] {
            draft.set(field, value.to_owned());
        }
        draft
    }
    pub fn get(&self, field: Field) -> &str {
        &self.values[field as usize].0
    }
    pub fn set(&mut self, field: Field, value: String) {
        self.values[field as usize] = std::rc::Rc::new(TextValue::new(value));
    }
    pub fn display(&self, field: Field) -> String {
        let value = self.get(field).trim();
        if value.is_empty() {
            return "Not set".into();
        }
        if field.secret() {
            return "Configured (hidden)".into();
        }
        clean(value)
    }
    pub fn dns_servers(&self) -> String {
        let ipv6 = self.ipv6 != Ipv6Policy::Disable;
        match self.dns_provider {
            DnsProvider::Custom => self.get(Field::DnsServers).trim().to_owned(),
            DnsProvider::Cloudflare => if ipv6 {
                "1.1.1.1,1.0.0.1,2606:4700:4700::1111"
            } else {
                "1.1.1.1,1.0.0.1"
            }
            .into(),
            DnsProvider::Google => if ipv6 {
                "8.8.8.8,8.8.4.4,2001:4860:4860::8888"
            } else {
                "8.8.8.8,8.8.4.4"
            }
            .into(),
            DnsProvider::Quad9 => if ipv6 {
                "9.9.9.9,149.112.112.112,2620:fe::fe"
            } else {
                "9.9.9.9,149.112.112.112"
            }
            .into(),
        }
    }
}
