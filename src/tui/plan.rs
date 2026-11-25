use super::fields::{ADDRESS_FIELDS, Field, TCP_FIELDS};
use super::state::{
    Authentication, CHOICES, Draft, Page, Route, Wizard, boot_label, hold_label, optional_label,
    valid_public_key_source, valid_username, yes_no,
};
use rustaller::config::{DnsConfig, DnsMode, InstallProfile};
use rustaller::host::args::{CliError, CliOutcome, parse_args};
use rustaller::host::ssh_keygen::{self, KeyType, KeygenOptions};
use rustaller::model::{InstallPlan, SecretString};

impl Wizard {
    pub fn validate_input(&self, field: Field, value: &str) -> Result<(), String> {
        if value.trim().is_empty() && !matches!(field, Field::Username | Field::SshPort) {
            return Ok(());
        }
        if matches!(
            field,
            Field::BootSize
                | Field::RootSize
                | Field::Compression
                | Field::Hostname
                | Field::Timezone
                | Field::Disk
                | Field::Mtu
                | Field::TcpBuffer
                | Field::Somaxconn
                | Field::SynBacklog
                | Field::NetdevBacklog
                | Field::MtuProbing
                | Field::Fastopen
                | Field::Syncookies
                | Field::SwapSize
                | Field::Swappiness
                | Field::Username
                | Field::SshPort
        ) {
            let mut args = vec![
                CHOICES[self.choice].1.to_owned(),
                format!("{}={}", field.option(), value.trim()),
            ];
            if field == Field::Compression {
                args.push("--filesystem=btrfs".into());
            }
            if field == Field::Swappiness {
                args.push("--swapfile-mib=2048".into());
            }
            parse_args(args).map_err(safe_cli_error)?;
        }
        Ok(())
    }
    pub fn invalid_field(&self) -> Option<Field> {
        let mut fields = Vec::new();
        if self.has_account() {
            fields.extend([Field::Username, Field::SshPort]);
        }
        if self.is_automatic() {
            fields.extend([
                Field::Hostname,
                Field::Timezone,
                Field::Disk,
                Field::BootSize,
                Field::RootSize,
                Field::Mtu,
            ]);
            if self.draft().root_filesystem == rustaller::config::RootFilesystem::Btrfs {
                fields.push(Field::Compression);
            }
            fields.extend_from_slice(TCP_FIELDS);
            if self.draft().swap_enabled {
                fields.extend([Field::SwapSize, Field::Swappiness]);
            }
        }
        fields.into_iter().find(|field| {
            self.validate_input(*field, self.draft().get(*field))
                .is_err()
        })
    }
    pub fn build_plan(&self) -> Result<Box<InstallPlan>, String> {
        let draft = self.draft();
        if self.is_automatic() && draft.hold == Some(1) {
            return Err("For rescue mode, choose Alpine Live from the system list".into());
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
            for field in [Field::Username, Field::SshPort, Field::Frpc] {
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
            for field in ADDRESS_FIELDS {
                add(*field);
            }
            add(Field::BootSize);
            add(Field::RootSize);
            if draft.root_filesystem.as_str() == "btrfs" {
                add(Field::Compression);
            }
            if draft.swap_enabled {
                if draft.get(Field::SwapSize).trim().is_empty() {
                    return Err("An enabled swapfile requires a size in MiB".into());
                }
                add(Field::SwapSize);
                add(Field::Swappiness);
            }
        }

        if self.is_automatic() {
            for (enabled, option) in [
                (draft.explicit_dhcp4, "--dhcp4"),
                (draft.explicit_dhcp6, "--dhcp6"),
                (draft.explicit_ra, "--ipv6-ra"),
            ] {
                if enabled {
                    args.push(option.into());
                }
            }
            let dns = effective_dns(draft)?;
            for server in &dns.servers {
                args.push(format!("--dns={server}"));
            }
            for domain in &dns.search_domains {
                args.push(format!("--dns-search={domain}"));
            }
            args.push(format!("--kernel={}", draft.kernel.as_str()));
            if self.selected_target() == Some(rustaller::model::Target::Gentoo) {
                args.push(format!("--gentoo-init={}", draft.gentoo_init.as_str()));
            }
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
        if self.route() == Route::Rescue {
            args.push("--hold=1".into());
            if draft.generic_kernel {
                args.push("--no-cloud-kernel".into());
            }
        }
        if let Some(boot) = draft.boot {
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
                format!("Address policy: {}", if draft.get(Field::NetworkMac).is_empty() { "Inherit verified source policy" } else { "Explicit Ethernet interface" }),
                format!("DNS policy: {}", draft.dns_mode.as_str()),
                format!("Root filesystem: {}", draft.root_filesystem.as_str()),
                format!("Target bootloader: rustaller-boot {}; FAT32 at /boot",rustaller::targets::boot::VERSION),
                format!("Root partition size: {}", if draft.get(Field::RootSize).is_empty() {"fill remaining disk".into()} else {format!("{} MiB",draft.get(Field::RootSize))}),
                format!("Boot partition: {} MiB", optional_label(draft.get(Field::BootSize), "512")),
                format!("Btrfs: {}", if draft.root_filesystem == rustaller::config::RootFilesystem::Btrfs { format!("@ root; zstd level {} (0=off)", optional_label(draft.get(Field::Compression), "3")) } else { "not selected".into() }),
                format!(
                    "Hostname: {}",
                    optional_label(draft.get(Field::Hostname), "Existing hostname, otherwise target system name")
                ),
                format!(
                    "Timezone: {}",
                    optional_label(draft.get(Field::Timezone), "Installer default")
                ),
                format!("Kernel choice: {}", self.kernel_label()),
                format!("Kernel package: {}", self.kernel_package_label()),
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
                    optional_label(draft.get(Field::Mtu), "Target defaults; not inherited")
                ),
            ]);
            if self.selected_target() == Some(rustaller::model::Target::Gentoo) {
                lines.extend([
                    format!("Stage3: {}", draft.gentoo_init.stage3_variant()),
                    format!("Init: {}", draft.gentoo_init.as_str()),
                    "ABI: amd64, no-multilib (64-bit only)".into(),
                ]);
            } else {
                lines.push(format!(
                    "Init: {}",
                    if self.selected_target() == Some(rustaller::model::Target::Alpine) {
                        "OpenRC"
                    } else {
                        "systemd"
                    }
                ));
            }
            if draft.root_filesystem == rustaller::config::RootFilesystem::Btrfs {
                lines.push("Btrfs snapshots exclude FAT32 /boot; restore matching kernels and initramfs separately.".into());
            }
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
        if self.is_automatic() && !draft.get(Field::NetworkMac).is_empty() {
            lines.extend([
                format!("Static IPv4: {}", draft.display(Field::Ipv4Address)),
                format!("IPv4 gateway: {}", draft.display(Field::Ipv4Gateway)),
                format!("Static IPv6: {}", draft.display(Field::Ipv6Address)),
                format!("IPv6 gateway: {}", draft.display(Field::Ipv6Gateway)),
                format!(
                    "DHCPv4 / DHCPv6 / IPv6 RA: {} / {} / {}",
                    yes_no(draft.explicit_dhcp4),
                    yes_no(draft.explicit_dhcp6),
                    yes_no(draft.explicit_ra)
                ),
            ]);
        }
        if self.is_automatic() && draft.get(Field::Username).trim() != "root" {
            lines.push(format!(
                "Root account password: {}",
                draft.display(Field::RootPassword)
            ));
        }
        if self.route() == Route::Rescue {
            lines.extend([
                "Mode: rescue (hold=1), no automatic partitioning".into(),
                format!("Generic kernel: {}", yes_no(draft.generic_kernel)),
            ]);
        }
        if self.has_account() {
            lines.extend([
                "IP addressing/routes: inherit existing network configuration".into(),
                format!("frpc: {}", draft.display(Field::Frpc)),
            ]);
        }

        lines.push(format!("Boot mode: {}", boot_label(draft.boot)));
        lines.push(format!("Debug output: {}", yes_no(draft.debug)));
        lines.extend([
            "".into(),
            "Installation requires final disk identity confirmation in the native terminal.".into(),
            "BIOS/UEFI acceptance status is reported separately; unit tests do not prove boot compatibility.".into(),
        ]);
        lines
    }

    fn kernel_package_label(&self) -> String {
        use rustaller::model::Target;
        use rustaller::postinstall::{KernelChoice, PostInstallConfig};
        let target = self.route().target();
        let draft = self.draft();
        if target == Target::Debian && draft.kernel == KernelChoice::Auto {
            return "automatic: linux-image-amd64 or linux-image-cloud-amd64 (host selection pending)".into();
        }
        PostInstallConfig {
            kernel: draft.kernel,
            ..Default::default()
        }
        .kernel_package(target, false, false)
        .map(|kernel| kernel.package.to_owned())
        .unwrap_or_else(|_| "Unavailable (invalid target/kernel combination)".into())
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
