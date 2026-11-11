#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Field {
    Username,
    Password,
    Confirm,
    SshPort,
    Hostname,
    Timezone,
    Disk,
    Confhome,
    WebPort,
    Frpc,
    IsoPath,
    IsoConfig,
    IsoKernel,
    IsoInitrd,
    IsoCmdline,
    DnsServers,
    DnsSearch,
    Mtu,
    TcpBuffer,
    Somaxconn,
    SynBacklog,
    NetdevBacklog,
    MtuProbing,
    Fastopen,
    Syncookies,
    SwapSize,
    Swappiness,
    KeyPath,
    KeyRounds,
    KeyComment,
    KeyBits,
}
impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Self::Username => "Username",
            Self::Password => "New password",
            Self::Confirm => "Confirm password",
            Self::SshPort => "SSH port",
            Self::Hostname => "Hostname",
            Self::Timezone => "Timezone",
            Self::Disk => "Target disk",
            Self::Confhome => "Matching HTTPS resource root",
            Self::WebPort => "Web log port",
            Self::Frpc => "frpc configuration file / URL",
            Self::IsoPath => "Absolute ISO path",
            Self::IsoConfig => "Loopback configuration inside ISO",
            Self::IsoKernel => "Kernel path inside ISO",
            Self::IsoInitrd => "Initrd path inside ISO",
            Self::IsoCmdline => "ISO kernel command line",
            Self::DnsServers => "DNS servers (comma-separated)",
            Self::DnsSearch => "DNS search domains (comma-separated)",
            Self::Mtu => "Network interface MTU",
            Self::TcpBuffer => "TCP / socket buffer limit (MiB)",
            Self::Somaxconn => "Listen backlog (somaxconn)",
            Self::SynBacklog => "SYN backlog limit",
            Self::NetdevBacklog => "Network device backlog",
            Self::MtuProbing => "TCP MTU probing (0/1/2)",
            Self::Fastopen => "TCP Fast Open (0/1/2/3)",
            Self::Syncookies => "SYN cookies (0/1)",
            Self::SwapSize => "Swapfile size (MiB)",
            Self::Swappiness => "vm.swappiness",
            Self::KeyPath => "Absolute private key output path",
            Self::KeyRounds => "Private key KDF rounds (-a)",
            Self::KeyComment => "Key comment (-C)",
            Self::KeyBits => "RSA bits (-b)",
        }
    }
    pub fn secret(self) -> bool {
        matches!(
            self,
            Self::Password | Self::Confirm | Self::Confhome | Self::Frpc | Self::IsoCmdline
        )
    }
    pub fn option(self) -> &'static str {
        match self {
            Self::Username => "--username",
            Self::Password => "--password",
            Self::Confirm => "",
            Self::SshPort => "--ssh-port",
            Self::Hostname => "--hostname",
            Self::Timezone => "--timezone",
            Self::Disk => "--target-disk",
            Self::Confhome => "--confhome",
            Self::WebPort => "--web-port",
            Self::Frpc => "--frpc-config",
            Self::IsoPath => "--iso",
            Self::IsoConfig => "--iso-config",
            Self::IsoKernel => "--iso-kernel",
            Self::IsoInitrd => "--iso-initrd",
            Self::IsoCmdline => "--iso-cmdline",
            Self::DnsServers => "--dns",
            Self::DnsSearch => "--dns-search",
            Self::Mtu => "--mtu",
            Self::TcpBuffer => "--tcp-buffer-mib",
            Self::Somaxconn => "--somaxconn",
            Self::SynBacklog => "--syn-backlog",
            Self::NetdevBacklog => "--netdev-backlog",
            Self::MtuProbing => "--tcp-mtu-probing",
            Self::Fastopen => "--tcp-fastopen",
            Self::Syncookies => "--tcp-syncookies",
            Self::SwapSize => "--swapfile-mib",
            Self::Swappiness => "--swappiness",
            Self::KeyPath | Self::KeyRounds | Self::KeyComment | Self::KeyBits => "",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Self::Username => {
                "1-32 ASCII letters, digits, _ or -; begin with a letter or _. Current-system mode requires an existing user."
            }
            Self::Password | Self::Confirm => {
                "Hidden input. Leading/trailing spaces are preserved; both passwords must match."
            }
            Self::SshPort | Self::WebPort => {
                "Use a port from 1 to 65535. A blank SSH port leaves the current system unchanged."
            }
            Self::Hostname => {
                "One ASCII label, 1-63 letters, digits or hyphens. Blank: rustaller for a new install; unchanged in current-system mode."
            }
            Self::Timezone => {
                "Optional IANA name, for example Etc/UTC, Asia/Shanghai or America/New_York."
            }
            Self::Disk => {
                "Whole-disk path such as /dev/vda, never a partition. Blank requests later detection; no disk scan has run here."
            }
            Self::Confhome => {
                "Optional matching HTTPS resources for compatible legacy assets. The Rust installer uses its embedded worker and official repositories. Hidden input; menus show only the hostname."
            }
            Self::Frpc => {
                "Optional file path or URL. This editor does not read or download the configuration."
            }
            Self::IsoPath => {
                "Absolute path to an existing Linux ISO on this machine. This editor does not mount it."
            }
            Self::IsoConfig => "Absolute path inside the ISO. Blank uses /boot/grub/loopback.cfg.",
            Self::IsoKernel | Self::IsoInitrd => {
                "Absolute path inside the ISO. Custom boot requires both kernel and initrd."
            }
            Self::IsoCmdline => {
                "ISO-specific arguments; %ISO_PATH% and %ISO_UUID% are available. Hidden input."
            }
            Self::DnsServers => {
                "1-3 upstream IPv4/IPv6 addresses, for example 1.1.1.1,2606:4700:4700::1111. Reachability is not tested here."
            }
            Self::DnsSearch => {
                "Optional: up to 6 ASCII domains, for example example.internal,lab.example. No protocol or path."
            }
            Self::Mtu => {
                "Optional: 1280-9000 for non-loopback interfaces. Use the provider's supported MTU; no automatic probing."
            }
            Self::TcpBuffer => {
                "Optional: 1-64 MiB maximum per-socket buffer, not reserved RAM. No performance improvement is guaranteed."
            }
            Self::Somaxconn | Self::SynBacklog | Self::NetdevBacklog => {
                "Optional: 128-262144. Blank leaves the setting unspecified."
            }
            Self::MtuProbing => {
                "Optional: 0=off, 1=on blackhole detection, 2=always. Blank leaves it unspecified."
            }
            Self::Fastopen => {
                "Optional: 0=off, 1=client, 2=server, 3=both. Applications also need Fast Open support."
            }
            Self::Syncookies => "Optional: 0=off, 1=on. Blank leaves it unspecified.",
            Self::SwapSize => {
                "64-65536 MiB; 1024 MiB=1 GiB, 2048 MiB=2 GiB. Target /swapfile, xfs, mode 0600 and fstab activation."
            }
            Self::Swappiness => {
                "Optional: 0-200. Read by the new system at boot; source/Live swapon and swapoff are never run."
            }
            Self::KeyPath => {
                "Absolute local output path; no overwrites or symlink parents. Back up the private key to your client before erasing its disk."
            }
            Self::KeyRounds => {
                "1-65536, default 512. Protects passphrase-encrypted keys; more rounds slow unlock. Native ssh-keygen asks for the passphrase."
            }
            Self::KeyComment => {
                "Default: system date YYYYMMDD. Up to 256 bytes of one-line text; no Shell expansion."
            }
            Self::KeyBits => {
                "RSA only: 2048, 3072, 4096 or 8192. Ed25519 does not use a bit count."
            }
        }
    }
}
pub(super) const TCP_FIELDS: &[Field] = &[
    Field::TcpBuffer,
    Field::Somaxconn,
    Field::SynBacklog,
    Field::NetdevBacklog,
    Field::MtuProbing,
    Field::Fastopen,
    Field::Syncookies,
];
