use super::fields::{Field, TCP_FIELDS};
use super::state::{
    Authentication, CHOICES, Draft, IsoMode, Page, Wizard, boot_label, hold_label, iso_label,
    optional_label, valid_public_key_source, valid_username, yes_no,
};
use rustaller::config::{DnsConfig, DnsMode, InstallProfile};
use rustaller::host::args::{CliError, CliOutcome, parse_args};
use rustaller::host::ssh_keygen::{self, KeyType, KeygenOptions};
use rustaller::model::{InstallPlan, SecretString};

impl Wizard {
    pub fn build_plan(&self) -> Result<Box<InstallPlan>, String> {
        let draft = self.draft();
        if self.is_automatic() && draft.hold == Some(1) {
            return Err("For rescue mode, choose Alpine Live from the system list".into());
        }
        if self.choice == 4
            && draft.iso_mode == IsoMode::Kernel
            && (draft.get(Field::IsoKernel).trim().is_empty()
                || draft.get(Field::IsoInitrd).trim().is_empty())
        {
            return Err("Custom ISO boot requires both kernel and initrd paths".into());
        }
        let mut args = vec![CHOICES[self.choice].1.to_owned()];
        let mut add = |field: Field| {
            let value = draft.get(field).trim();
            if !value.is_empty() {
                args.push(format!("{}={value}", field.option()));
            }
        };
        if self.has_account() {
            if !valid_username(draft.get(Field::Username).trim()) {
                return Err("Set a valid username".into());
            }
            for field in [Field::Username, Field::SshPort, Field::WebPort, Field::Frpc] {
                add(field);
            }
        }
        if self.is_automatic() {
            for field in [Field::Hostname, Field::Timezone, Field::Disk] {
                add(field);
            }
            for field in TCP_FIELDS {
                add(*field);
            }
            add(Field::Mtu);
            if draft.swap_enabled {
                if draft.get(Field::SwapSize).trim().is_empty() {
                    return Err("An enabled swapfile requires a size in MiB".into());
                }
                add(Field::SwapSize);
                add(Field::Swappiness);
            }
        }
        if self.choice == 4 {
            add(Field::IsoPath);
            if draft.iso_mode == IsoMode::Loopback {
                add(Field::IsoConfig);
            } else {
                for field in [Field::IsoKernel, Field::IsoInitrd, Field::IsoCmdline] {
                    add(field);
                }
            }
        }
        if self.is_automatic() {
            let dns = effective_dns(draft)?;
            for server in &dns.servers {
                args.push(format!("--dns={server}"));
            }
            for domain in &dns.search_domains {
                args.push(format!("--dns-search={domain}"));
            }
            args.push(format!("--kernel={}", draft.kernel.as_str()));
            args.push(format!("--filesystem={}", draft.root_filesystem.as_str()));
            args.push(format!("--profile={}", draft.profile.as_str()));
            args.push(format!("--dns-mode={}", draft.dns_mode.as_str()));
            args.push(format!("--tcp-congestion={}", draft.congestion.as_str()));
            args.push(format!("--qdisc={}", draft.qdisc.as_str()));
            args.push(format!("--ipv6={}", draft.ipv6.as_str()));
            args.push(format!(
                "--address-preference={}",
                draft.address_preference.as_str()
            ));
            if draft.with_tools {
                args.push("--with-tools".into());
            }
            if draft.profile == InstallProfile::Minimal {
                if draft.keep_docs {
                    args.push("--keep-docs".into());
                }
                if draft.keep_package_cache {
                    args.push("--keep-package-cache".into());
                }
            }
            if let Some(hold) = draft.hold {
                args.push(format!("--hold={hold}"));
            }
        }
        if self.choice == 5 {
            args.push("--hold=1".into());
            if draft.generic_kernel {
                args.push("--no-cloud-kernel".into());
            }
        }
        if let Some(boot) = draft.boot
            && self.choice != 7
        {
            args.push(format!("--force-boot-mode={}", boot.as_str()));
        }
        if draft.debug {
            args.push("--debug".into());
        }
        if self.has_account() && draft.authentication == Authentication::PublicKeys {
            if draft.keys.is_empty()
                || draft.keys.len() > 128
                || draft
                    .keys
                    .iter()
                    .any(|key| !valid_public_key_source(&key.0))
            {
                return Err("Add 1-128 valid SSH public key sources".into());
            }
            args.extend(
                draft
                    .keys
                    .iter()
                    .map(|key| format!("--ssh-key={}", key.0.trim())),
            );
        }
        let CliOutcome::Plan(mut plan) = parse_args(&args).map_err(safe_cli_error)? else {
            return Err("Cannot create the install plan; check the fields".into());
        };
        if self.has_account() && draft.authentication == Authentication::Password {
            let password = draft.get(Field::Password);
            if password.is_empty() {
                return Err("Set and confirm the login password, or select SSH public keys".into());
            }
            if password != draft.get(Field::Confirm) {
                return Err("The two passwords do not match".into());
            }
            plan.password = Some(SecretString::new(password));
        }
        if self.is_automatic() && draft.get(Field::Username).trim() != "root" {
            let password = draft.get(Field::RootPassword);
            if password.is_empty() {
                return Err("Set and confirm a separate root account password".into());
            }
            if password != draft.get(Field::RootConfirm) {
                return Err("The two root passwords do not match".into());
            }
            plan.root_password = Some(SecretString::new(password));
        }
        plan.validate().map_err(|error| error.to_string())?;
        plan.validate_install_credentials()
            .map_err(|error| error.to_string())?;
        if let Some(iso) = plan.iso.as_ref() {
            rustaller::host::iso::validate_iso_options(iso).map_err(|_| {
                "Invalid ISO path/boot configuration; check absolute paths and arguments".to_owned()
            })?;
        }
        Ok(plan)
    }

    pub fn keygen_options(&self) -> Result<KeygenOptions, String> {
        if self.page() != Page::Keygen
            || !self.has_account()
            || self.draft().authentication != Authentication::PublicKeys
            || self.draft().keys.len() >= 128
        {
            return Err(
                "Key generation requires SSH public-key authentication and fewer than 128 keys"
                    .into(),
            );
        }
        let draft = self.draft();
        let options = KeygenOptions {
            key_type: draft.key_type,
            rounds: draft
                .get(Field::KeyRounds)
                .trim()
                .parse()
                .map_err(|_| "Invalid KDF round count")?,
            cipher: ssh_keygen::CIPHERS[draft.key_cipher].into(),
            rsa_bits: if draft.key_type == KeyType::Rsa {
                draft
                    .get(Field::KeyBits)
                    .trim()
                    .parse()
                    .map_err(|_| "Invalid RSA bit count")?
            } else {
                4096
            },
            comment: draft.get(Field::KeyComment).to_owned(),
            output: draft.get(Field::KeyPath).trim().into(),
        };
        options.validate().map_err(|e| e.to_string())?;
        Ok(options)
    }

    pub fn summary(&self) -> Vec<String> {
        let draft = self.draft();
        let mut lines = vec![format!("System: {}", CHOICES[self.choice].0), "".into()];
        if self.has_account() {
            lines.extend([
                format!("Username: {}", draft.display(Field::Username)),
                format!(
                    "Authentication: {}",
                    match draft.authentication {
                        Authentication::Password => "Password configured (hidden)".into(),
                        Authentication::PublicKeys =>
                            format!("{} SSH public keys (hidden)", draft.keys.len()),
                    }
                ),
                format!("SSH port: {}", draft.display(Field::SshPort)),
            ]);
        }
        if self.is_automatic() {
            lines.extend([
                format!("Install profile: {}", draft.profile.as_str()),
                format!("Add curl / less: {}", yes_no(draft.with_tools)),
                format!(
                    "Documentation/translations: {}",
                    if draft.profile == InstallProfile::Minimal && !draft.keep_docs {
                        "Trim documentation (retain copyright/licenses)"
                    } else {
                        "Keep"
                    }
                ),
                format!(
                    "Package cache: {}",
                    if draft.profile == InstallProfile::Minimal && !draft.keep_package_cache {
                        "Clean cache after installation"
                    } else {
                        "Keep existing behavior"
                    }
                ),
                format!("DNS policy: {}", draft.dns_mode.as_str()),
                format!("Root filesystem: {}", draft.root_filesystem.as_str()),
                format!(
                    "Hostname: {}",
                    optional_label(draft.get(Field::Hostname), "Existing hostname, otherwise target system name")
                ),
                format!(
                    "Timezone: {}",
                    optional_label(draft.get(Field::Timezone), "Installer default")
                ),
                format!("Kernel: {}", self.kernel_label()),
                format!(
                    "Target disk: {}",
                    optional_label(draft.get(Field::Disk), "Auto-detect (not scanned yet)")
                ),
                format!("Hold behavior: {}", hold_label(draft.hold)),
                format!(
                    "Target swapfile: {}",
                    if draft.swap_enabled {
                        format!(
                            "/swapfile, {} MiB, {}, mode 0600, activated at target boot",
                            draft.display(Field::SwapSize), draft.root_filesystem.as_str()
                        )
                    } else {
                        "Do not create; preserve existing swap policy".into()
                    }
                ),
                "Execution: kexec into Alpine Live, install, then one final reboot into the new system"
                    .into(),
                format!(
                    "TCP congestion / queue: {} / {}",
                    draft.congestion.as_str(),
                    draft.qdisc.as_str()
                ),
                format!(
                    "IPv6: {}; address preference: {}",
                    draft.ipv6.as_str(),
                    draft.address_preference.as_str()
                ),
                format!(
                    "MTU: {}",
                    optional_label(draft.get(Field::Mtu), "Keep interface configuration")
                ),
            ]);
            for field in TCP_FIELDS {
                if !draft.get(*field).trim().is_empty() {
                    lines.push(format!("{}: {}", field.label(), draft.display(*field)));
                }
            }
            if draft.swap_enabled {
                lines.push(format!(
                    "swappiness: {}",
                    optional_label(draft.get(Field::Swappiness), "Kernel default")
                ));
            }
            if draft.dns_mode == DnsMode::Static {
                lines.extend([
                    format!("DNS provider: {}", draft.dns_provider.label()),
                    format!("DNS servers: {}", draft.dns_servers()),
                    format!(
                        "DNS search domains: {}",
                        optional_label(draft.get(Field::DnsSearch), "None")
                    ),
                ]);
            }
        }
        if self.is_automatic() && draft.get(Field::Username).trim() != "root" {
            lines.push(format!(
                "Root account password: {}",
                draft.display(Field::RootPassword)
            ));
        }
        if self.choice == 5 {
            lines.extend([
                "Mode: rescue (hold=1), no automatic partitioning".into(),
                format!("Generic kernel: {}", yes_no(draft.generic_kernel)),
            ]);
        }
        if self.has_account() {
            lines.extend([
                "IP addressing/routes: inherit existing network configuration".into(),
                format!("Web log port: {}", draft.display(Field::WebPort)),
                format!("frpc: {}", draft.display(Field::Frpc)),
            ]);
        }
        if self.choice == 4 {
            lines.extend([
                format!("ISO: {}", draft.display(Field::IsoPath)),
                format!("ISO boot: {}", iso_label(draft.iso_mode)),
            ]);
            if draft.iso_mode == IsoMode::Loopback {
                lines.push(format!(
                    "Loopback configuration: {}",
                    optional_label(draft.get(Field::IsoConfig), "/boot/grub/loopback.cfg")
                ));
            } else {
                for field in [Field::IsoKernel, Field::IsoInitrd, Field::IsoCmdline] {
                    lines.push(format!("{}: {}", field.label(), draft.display(field)));
                }
            }
            lines.push(
                "ISO compatibility is media-specific; use the console for interactive installation"
                    .into(),
            );
        }
        if self.choice != 7 {
            lines.push(format!("Boot mode: {}", boot_label(draft.boot)));
        }
        lines.push(format!("Debug output: {}", yes_no(draft.debug)));
        lines.extend([
            "".into(),
            "Installation requires final disk identity confirmation in the native terminal.".into(),
            "BIOS/UEFI acceptance status is reported separately; unit tests do not prove boot compatibility.".into(),
        ]);
        lines
    }
}

fn effective_dns(draft: &Draft) -> Result<DnsConfig, String> {
    let mut dns = DnsConfig {
        mode: draft.dns_mode,
        ..Default::default()
    };
    if draft.dns_mode == DnsMode::Static {
        dns.servers = draft
            .dns_servers()
            .split(',')
            .map(|value| {
                value.trim().parse().map_err(|_| {
                    "Enter DNS IPv4/IPv6 addresses separated by commas, without empty entries"
                        .to_owned()
                })
            })
            .collect::<Result<_, _>>()?;
        let search = draft.get(Field::DnsSearch).trim();
        if !search.is_empty() {
            dns.search_domains = search
                .split(',')
                .map(|value| value.trim().to_owned())
                .collect();
        }
    }
    dns.validate().map_err(|error| error.to_string())?;
    Ok(dns)
}

fn safe_cli_error(error: CliError) -> String {
    match error {
        CliError::InvalidPlan(error) => error.to_string(),
        CliError::InvalidValue(option) | CliError::MissingValue(option) => {
            format!("Invalid {option}; check this field")
        }
        _ => "Invalid configuration; check the selected system and supported fields".into(),
    }
}
