use super::editor::{Editor, TextValue};
pub(super) use super::fields::Field;
use super::fields::TCP_FIELDS;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rustaller::config::{AddressPreference, CongestionControl, Ipv6Policy, QueueDiscipline};
use rustaller::config::{DnsMode, InstallProfile, RootFilesystem};
use rustaller::host::ssh_keygen::{self, GeneratedKey, KeyType, KeygenOptions};
use rustaller::model::{BootMode, InstallPlan, Target};
use rustaller::postinstall::KernelChoice;

pub(super) const CHOICES: [(&str, &str); 8] = [
    ("Alpine Linux", "alpine"),
    ("Debian minimal", "debian"),
    ("Arch Linux", "arch"),
    ("Gentoo", "gentoo"),
    ("Local Linux ISO", "iso"),
    ("Rescue: Alpine Live", "alpine"),
    ("Tool: netboot.xyz", "netboot.xyz"),
    ("Tool: remove installation boot entry", "reset"),
];

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Authentication {
    Password,
    PublicKeys,
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum IsoMode {
    Loopback,
    Kernel,
}
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
    pub values: Vec<TextValue>,
    pub keys: Vec<TextValue>,
    pub authentication: Authentication,
    pub kernel: KernelChoice,
    pub boot: Option<BootMode>,
    pub hold: Option<u8>,
    pub iso_mode: IsoMode,
    pub generic_kernel: bool,
    pub debug: bool,
    pub profile: InstallProfile,
    pub dns_mode: DnsMode,
    pub dns_provider: DnsProvider,
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
    fn new(comment: &str, key_path: &str) -> Self {
        let mut draft = Self {
            values: (0..=Field::KeyBits as usize)
                .map(|_| TextValue::new(""))
                .collect(),
            keys: Vec::new(),
            authentication: Authentication::Password,
            kernel: KernelChoice::Auto,
            boot: None,
            hold: None,
            iso_mode: IsoMode::Loopback,
            generic_kernel: false,
            debug: false,
            profile: InstallProfile::Standard,
            dns_mode: DnsMode::Auto,
            dns_provider: DnsProvider::Custom,
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
            (Field::Username, "root"),
            (Field::SshPort, "22"),
            (Field::WebPort, "80"),
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
        self.values[field as usize].replace(value);
    }
    pub fn display(&self, field: Field) -> String {
        let value = self.get(field);
        let value = if matches!(
            field,
            Field::Password | Field::Confirm | Field::RootPassword | Field::RootConfirm
        ) {
            value
        } else {
            value.trim()
        };
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Page {
    Home,
    Targets,
    Overview,
    Account,
    Auth,
    Passwords,
    RootPasswords,
    Keys,
    KeyActions(usize),
    Host,
    Installation,
    Network,
    Dns,
    Tcp,
    IpStack,
    Mtu,
    Swap,
    Keygen,
    Boot,
    Resources,
    Iso,
}
impl Page {
    pub fn title(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Targets => "Choose a system",
            Self::Overview => "System configuration",
            Self::Account => "Account and SSH",
            Self::Auth => "Authentication",
            Self::Passwords => "Password settings",
            Self::RootPasswords => "Root account password",
            Self::Keys => "SSH public keys",
            Self::KeyActions(_) => "Public key actions",
            Self::Host => "Host settings",
            Self::Installation => "Installation profile",
            Self::Network => "Network and DNS",
            Self::Dns => "DNS settings",
            Self::Tcp => "TCP and congestion control",
            Self::IpStack => "IPv6 and address preference",
            Self::Mtu => "Interface MTU",
            Self::Swap => "Target swapfile",
            Self::Keygen => "System ssh-keygen",
            Self::Boot => "Disk and boot",
            Self::Resources => "Resources and logs",
            Self::Iso => "ISO media",
        }
    }
}
pub(super) struct MenuFrame {
    pub page: Page,
    pub selected: usize,
    pub offset: usize,
    pub baseline: Option<Draft>,
}
#[derive(Clone, Copy)]
pub(super) enum EditSlot {
    Field(Field),
    Key(usize),
    NewKey,
}
impl EditSlot {
    pub fn label(self) -> &'static str {
        match self {
            Self::Field(field) => field.label(),
            _ => "SSH public key / file / URL",
        }
    }
    fn secret(self) -> bool {
        match self {
            Self::Field(field) => field.secret(),
            _ => true,
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Self::Field(field) => field.hint(),
            _ => {
                "Public keys only: inline key, public-key file, HTTP(S) URL, github:USER or gitlab:USER. Never enter a private key."
            }
        }
    }
}
#[derive(Clone, Copy)]
pub(super) enum Setting {
    Authentication,
    Kernel,
    Boot,
    Hold,
    IsoMode,
    Profile,
    DnsMode,
    RootFilesystem,
    Congestion,
    Qdisc,
    Ipv6,
    Preference,
    KeyType,
    KeyCipher,
}
pub(super) enum View {
    Menu,
    Edit {
        slot: EditSlot,
        input: Editor,
    },
    Select {
        setting: Setting,
        selected: usize,
        offset: usize,
    },
    Review {
        scroll: usize,
    },
    Outcome {
        success: bool,
        message: String,
        scroll: usize,
    },
    ToolResult {
        message: String,
        scroll: usize,
    },
    Leaving {
        selected: usize,
        offset: usize,
    },
}
pub(super) enum Progress {
    Continue,
    Ready(Box<InstallPlan>),
    GenerateKey(KeygenOptions),
    Cancel,
}
pub(super) enum Action {
    Open(Page),
    Back,
    Edit(EditSlot),
    Select(Setting),
    ToggleDebug,
    ToggleGeneric,
    ToggleTools,
    ToggleKeepDocs,
    ToggleKeepCache,
    ToggleSwap,
    ResetTcp,
    GenerateKey,
    Review,
    Target(usize),
    DeleteKey(usize),
    Quit,
}
pub(super) struct Entry {
    pub label: String,
    pub action: Action,
}
pub(super) struct Wizard {
    pub choice: usize,
    pub drafts: Vec<Draft>,
    pub frames: Vec<MenuFrame>,
    pub view: View,
    pub error: String,
    pub usable_terminal: bool,
}

impl Wizard {
    pub fn prefill_hostnames(&mut self, root: &std::path::Path) {
        for (index, target) in [Target::Alpine, Target::Debian, Target::Arch, Target::Gentoo]
            .into_iter()
            .enumerate()
        {
            if self.drafts[index].get(Field::Hostname).trim().is_empty() {
                self.drafts[index].set(
                    Field::Hostname,
                    rustaller::host::hostname::default_hostname(target, root),
                );
            }
        }
    }
    pub fn new() -> Self {
        let comment = ssh_keygen::date_comment();
        let key_path = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| "/root".into())
            .join(".ssh/id_rustaller_ed25519")
            .to_string_lossy()
            .into_owned();
        let drafts: Vec<_> = (0..CHOICES.len())
            .map(|_| Draft::new(&comment, &key_path))
            .collect();
        Self {
            choice: 0,
            drafts,
            frames: vec![MenuFrame {
                page: Page::Home,
                selected: 0,
                offset: 0,
                baseline: None,
            }],
            view: View::Menu,
            error: String::new(),
            usable_terminal: true,
        }
    }
    pub fn draft(&self) -> &Draft {
        &self.drafts[self.choice]
    }
    pub fn draft_mut(&mut self) -> &mut Draft {
        &mut self.drafts[self.choice]
    }
    pub fn page(&self) -> Page {
        self.frames.last().expect("root menu").page
    }
    pub fn is_automatic(&self) -> bool {
        self.choice < 4
    }
    pub fn selected_target(&self) -> Option<Target> {
        match self.choice {
            0 | 5 => Some(Target::Alpine),
            1 => Some(Target::Debian),
            2 => Some(Target::Arch),
            3 => Some(Target::Gentoo),
            _ => None,
        }
    }
    pub fn context_label(&self) -> String {
        CHOICES[self.choice].0.into()
    }
    pub fn has_account(&self) -> bool {
        self.choice < 4 || self.choice == 5
    }
    pub fn kernel_label(&self) -> &'static str {
        if self.choice < 3 && self.draft().kernel == KernelChoice::Auto {
            "standard"
        } else {
            self.draft().kernel.as_str()
        }
    }
    fn open(&mut self, page: Page) {
        self.frames.push(MenuFrame {
            page,
            selected: 0,
            offset: 0,
            baseline: if matches!(page, Page::Home | Page::Targets) {
                None
            } else {
                Some(self.draft().clone())
            },
        });
    }
    pub fn breadcrumb(&self) -> String {
        let parts: Vec<_> = self
            .frames
            .iter()
            .map(|frame| {
                if frame.page == Page::Overview {
                    self.context_label()
                } else {
                    frame.page.title().to_owned()
                }
            })
            .collect();
        parts.join(" / ")
    }
    fn request_back(&mut self) -> Progress {
        if self.frames.len() == 1 {
            return Progress::Cancel;
        }
        if self
            .frames
            .last()
            .and_then(|frame| frame.baseline.as_ref())
            .is_some_and(|baseline| baseline != self.draft())
        {
            self.view = View::Leaving {
                selected: 0,
                offset: 0,
            };
        } else {
            self.frames.pop();
        }
        Progress::Continue
    }
    fn leave_directory(&mut self, discard: bool) {
        if let Some(frame) = self.frames.pop()
            && discard
            && let Some(baseline) = frame.baseline
        {
            self.drafts[self.choice] = baseline;
        }
    }
    fn activate(&mut self, action: Action) -> Progress {
        match action {
            Action::Open(Page::Targets) => {
                self.choice = 0;
                self.open(Page::Targets);
            }
            Action::Open(page) => self.open(page),
            Action::Back => return self.request_back(),
            Action::Edit(slot) => {
                let value = match slot {
                    EditSlot::Field(field) => self.draft().get(field),
                    EditSlot::Key(index) => &self.draft().keys[index].0,
                    EditSlot::NewKey => "",
                };
                self.view = View::Edit {
                    slot,
                    input: Editor::new(value, slot.secret()),
                };
            }
            Action::Select(setting) => {
                self.view = View::Select {
                    setting,
                    selected: self.setting_index(setting),
                    offset: 0,
                }
            }
            Action::ToggleDebug => self.draft_mut().debug = !self.draft().debug,
            Action::ToggleTools => self.draft_mut().with_tools = !self.draft().with_tools,
            Action::ToggleKeepDocs => self.draft_mut().keep_docs = !self.draft().keep_docs,
            Action::ToggleKeepCache => {
                self.draft_mut().keep_package_cache = !self.draft().keep_package_cache
            }
            Action::ToggleSwap => {
                self.draft_mut().swap_enabled = !self.draft().swap_enabled;
                // Guide the next Enter to the size editor, while preserving
                // per-target size/swappiness drafts when disabled.
                self.frames.last_mut().expect("swap menu").selected =
                    usize::from(self.draft().swap_enabled);
            }
            Action::ResetTcp => {
                let draft = self.draft_mut();
                draft.congestion = CongestionControl::Default;
                draft.qdisc = QueueDiscipline::Default;
                for field in TCP_FIELDS {
                    draft.set(*field, String::new());
                }
            }
            Action::GenerateKey => match self.keygen_options() {
                Ok(options) => return Progress::GenerateKey(options),
                Err(error) => self.error = error,
            },
            Action::ToggleGeneric => self.draft_mut().generic_kernel = !self.draft().generic_kernel,
            Action::Target(index) => {
                self.choice = index;
                self.frames.truncate(2);
                self.frames[1].selected = index;
                self.open(Page::Overview);
            }
            Action::DeleteKey(index) => {
                self.draft_mut().keys.remove(index);
                self.frames.pop();
                let len = self.entries().len();
                self.frames.last_mut().expect("keys").selected = index.min(len - 1);
            }
            Action::Review => self.review(),
            Action::Quit => return Progress::Cancel,
        }
        Progress::Continue
    }
    fn commit(&mut self, slot: EditSlot, input: &mut Editor) -> Result<(), String> {
        let value = &input.value.0;
        match slot {
            EditSlot::Field(Field::Username) if !valid_username(value.trim()) => {
                return Err(
                    "Username: 1-32 ASCII letters, digits, _ or -; begin with a letter or _".into(),
                );
            }
            EditSlot::Field(Field::SshPort | Field::WebPort)
                if value.parse::<u16>().ok().is_none_or(|port| port == 0) =>
            {
                return Err("Port must be an integer from 1 to 65535".into());
            }
            EditSlot::Field(Field::DnsServers) => {
                let servers = value.split(',').map(|server| server.trim().parse().map_err(|_| "DNS requires comma-separated IPv4/IPv6 addresses without empty entries".to_owned())).collect::<Result<_, _>>()?;
                rustaller::config::DnsConfig {
                    mode: DnsMode::Static,
                    servers,
                    search_domains: Vec::new(),
                }
                .validate()
                .map_err(|error| error.to_string())?;
            }
            EditSlot::Key(_) | EditSlot::NewKey if !valid_public_key_source(value) => {
                return Err("Enter a nonempty SSH public key/source, never a private key".into());
            }
            EditSlot::NewKey if self.draft().keys.len() >= 128 => {
                return Err("At most 128 SSH public key sources are supported".into());
            }
            _ => {}
        }
        let value = std::mem::take(&mut input.value.0);
        match slot {
            EditSlot::Field(field) => self.draft_mut().set(field, value),
            EditSlot::Key(index) => self.draft_mut().keys[index].replace(value.trim().into()),
            EditSlot::NewKey => self.draft_mut().keys.push(TextValue::new(value.trim())),
        }
        if matches!(slot, EditSlot::Field(Field::Username))
            && self.is_automatic()
            && self.draft().get(Field::Username).trim() != "root"
            && self.draft().get(Field::RootPassword).is_empty()
        {
            self.open(Page::RootPasswords);
        }
        Ok(())
    }
    fn review(&mut self) {
        if matches!(self.page(), Page::Home | Page::Targets) {
            self.error = "Choose an installation target first".into();
            return;
        }
        match self.build_plan() {
            Ok(_) => self.view = View::Review { scroll: 0 },
            Err(error) => self.error = error,
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Progress {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Progress::Cancel;
        }
        if !self.usable_terminal && key.code != KeyCode::Esc {
            return Progress::Continue;
        }
        self.error.clear();
        let view = std::mem::replace(&mut self.view, View::Menu);
        match view {
            View::Menu => {
                if key.code == KeyCode::F(2) {
                    self.review();
                    return Progress::Continue;
                }
                let entries = self.entries();
                let frame = self.frames.last_mut().expect("menu");
                if navigate(key.code, &mut frame.selected, entries.len()) {
                    return Progress::Continue;
                }
                match key.code {
                    KeyCode::Esc | KeyCode::Left => {
                        return self.request_back();
                    }
                    KeyCode::Enter | KeyCode::Right => {
                        let selected = frame.selected;
                        return self.activate(
                            entries
                                .into_iter()
                                .nth(selected)
                                .expect("menu entry")
                                .action,
                        );
                    }
                    _ => {}
                }
            }
            View::Edit { slot, mut input } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Err(error) = self.commit(slot, &mut input) {
                        self.error = error;
                        self.view = View::Edit { slot, input };
                    }
                }
                _ => {
                    if let Err(error) = input.key(key) {
                        self.error = error.into();
                    }
                    self.view = View::Edit { slot, input };
                }
            },
            View::Select {
                setting,
                mut selected,
                offset,
            } => {
                if navigate(key.code, &mut selected, self.choices(setting).len()) {
                    self.view = View::Select {
                        setting,
                        selected,
                        offset,
                    };
                } else {
                    match key.code {
                        KeyCode::Esc | KeyCode::Left => {}
                        KeyCode::Enter | KeyCode::Right => self.select(setting, selected),
                        _ => {
                            self.view = View::Select {
                                setting,
                                selected,
                                offset,
                            }
                        }
                    }
                }
            }
            View::Review { mut scroll } => match key.code {
                KeyCode::Esc | KeyCode::Left => {}
                KeyCode::Enter => match self.build_plan() {
                    Ok(plan) => return Progress::Ready(plan),
                    Err(error) => {
                        self.error = error;
                        self.view = View::Review { scroll };
                    }
                },
                _ => {
                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') => scroll = scroll.saturating_sub(1),
                        KeyCode::Down | KeyCode::Char('j') => scroll = scroll.saturating_add(1),
                        KeyCode::PageUp => scroll = scroll.saturating_sub(8),
                        KeyCode::PageDown => scroll = scroll.saturating_add(8),
                        KeyCode::Home => scroll = 0,
                        KeyCode::End => scroll = usize::MAX,
                        _ => {}
                    };
                    self.view = View::Review { scroll };
                }
            },
            View::Outcome {
                success,
                message,
                mut scroll,
            } => match key.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Left => {
                    self.view = View::Review { scroll: 0 }
                }
                _ => {
                    scroll_text(key.code, &mut scroll);
                    self.view = View::Outcome {
                        success,
                        message,
                        scroll,
                    };
                }
            },
            View::ToolResult {
                message,
                mut scroll,
            } => {
                if !matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Left) {
                    scroll_text(key.code, &mut scroll);
                    self.view = View::ToolResult { message, scroll };
                }
            }
            View::Leaving {
                mut selected,
                offset,
            } => {
                if navigate(key.code, &mut selected, 3) {
                    self.view = View::Leaving { selected, offset };
                } else {
                    match key.code {
                        KeyCode::Esc | KeyCode::Left => {}
                        KeyCode::Enter => match selected {
                            0 => self.leave_directory(false),
                            1 => self.leave_directory(true),
                            _ => {}
                        },
                        _ => self.view = View::Leaving { selected, offset },
                    }
                }
            }
        }
        Progress::Continue
    }
    pub fn paste(&mut self, text: &str) {
        if self.usable_terminal
            && let View::Edit { input, .. } = &mut self.view
        {
            match input.paste(text) {
                Ok(()) => self.error.clear(),
                Err(error) => self.error = error.into(),
            }
        }
    }
    pub fn complete(&mut self, result: Result<(), String>) {
        self.complete_message(result.map(|()| {
            "Action completed. Return to review to keep editing the configuration.".into()
        }));
    }
    fn complete_message(&mut self, result: Result<String, String>) {
        let success = result.is_ok();
        let mut message = match result {
            Ok(message) => message,
            Err(error) => error,
        };
        let mut secrets = Vec::new();
        for draft in &self.drafts {
            for field in [
                Field::Password,
                Field::Confirm,
                Field::RootPassword,
                Field::RootConfirm,
                Field::Frpc,
                Field::IsoCmdline,
            ] {
                let value = draft.get(field);
                secrets.extend([value, value.trim()]);
            }
            for key in &draft.keys {
                secrets.extend([key.0.as_str(), key.0.trim()]);
            }
        }
        secrets.retain(|value| !value.is_empty());
        secrets.sort_unstable_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        secrets.dedup();
        for secret in secrets {
            message = message.replace(secret, "█");
        }
        let message = message
            .chars()
            .map(|character| {
                if character == '\n' || !character.is_control() {
                    character
                } else {
                    '\u{fffd}'
                }
            })
            .collect();
        self.view = View::Outcome {
            success,
            message,
            scroll: 0,
        };
    }
    pub fn keygen_complete(&mut self, result: Result<GeneratedKey, String>) {
        let message = match result {
            Ok(key) => {
                self.draft_mut().keys.push(TextValue::new(key.public_key));
                format!(
                    "Key generated and public key added.\nPrivate key: {}\nPublic key: {}\n\nBack up the private key to your client first. Installation erases its target disk; the private key is not copied into the new system. ssh-keygen handles the passphrase natively.",
                    clean(&key.private_path.to_string_lossy()),
                    clean(&key.public_path.to_string_lossy())
                )
            }
            Err(error) => format!("Key was not added: {}", clean(&error)),
        };
        self.view = View::ToolResult { message, scroll: 0 };
    }
}

pub(super) use rustaller::model::valid_account_name as valid_username;
pub(super) fn valid_public_key_source(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.contains("PRIVATE KEY")
        && !value.contains("-----BEGIN")
        && !value.chars().any(char::is_control)
}
pub(super) fn clean(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}
pub(super) fn optional_label(value: &str, default: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        default.into()
    } else {
        clean(value)
    }
}

pub(super) fn boot_label(mode: Option<BootMode>) -> &'static str {
    match mode {
        None => "Auto-detect",
        Some(BootMode::Bios) => "BIOS",
        Some(BootMode::Efi) => "UEFI",
    }
}
pub(super) fn hold_label(hold: Option<u8>) -> &'static str {
    match hold {
        Some(0) => "Wait for confirmation after preparation (hold=0)",
        Some(2) => "Keep Live environment after installation (hold=2)",
        _ => "Default behavior",
    }
}
pub(super) fn iso_label(mode: IsoMode) -> &'static str {
    match mode {
        IsoMode::Loopback => "ISO-provided loopback configuration",
        IsoMode::Kernel => "Explicit kernel and initrd",
    }
}
pub(super) fn yes_no(value: bool) -> &'static str {
    if value { "On" } else { "Off" }
}
fn navigate(key: KeyCode, selected: &mut usize, len: usize) -> bool {
    match key {
        KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => *selected = (*selected + 1) % len,
        KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => {
            *selected = (*selected + len - 1) % len
        }
        KeyCode::Home => *selected = 0,
        KeyCode::End => *selected = len - 1,
        KeyCode::PageDown => *selected = (*selected + 5).min(len - 1),
        KeyCode::PageUp => *selected = selected.saturating_sub(5),
        _ => return false,
    };
    true
}
pub(super) fn scroll_text(key: KeyCode, scroll: &mut usize) {
    match key {
        KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
        KeyCode::PageUp => *scroll = scroll.saturating_sub(8),
        KeyCode::PageDown => *scroll = scroll.saturating_add(8),
        KeyCode::Home => *scroll = 0,
        KeyCode::End => *scroll = usize::MAX,
        _ => {}
    }
}
