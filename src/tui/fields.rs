#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Field {
    Username,
    SshPort,
    BootSize,
    RootSize,
    Hostname,
    Timezone,
    Disk,
    Frpc,
    NetworkMac,
    Ipv4Address,
    Ipv4Gateway,
    Ipv6Address,
    Ipv6Gateway,
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
            Self::SshPort => "SSH port",
            Self::RootSize => "Root size (MiB)",
            Self::BootSize => "FAT32 boot size (MiB)",
            Self::Hostname => "Hostname",
            Self::Timezone => "Timezone",
            Self::Disk => "Target disk",
            Self::Frpc => "frpc configuration file / URL",
            Self::NetworkMac => "Explicit Ethernet MAC",
            Self::Ipv4Address => "Static IPv4 address/prefix",
            Self::Ipv4Gateway => "IPv4 gateway",
            Self::Ipv6Address => "Static IPv6 address/prefix",
            Self::Ipv6Gateway => "IPv6 gateway",
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
        self == Self::Frpc
    }
    pub fn option(self) -> &'static str {
        match self {
            Self::Username => "--username",
            Self::SshPort => "--ssh-port",
            Self::RootSize => "--root-size-mib",
            Self::BootSize => "--boot-size-mib",
            Self::Hostname => "--hostname",
            Self::Timezone => "--timezone",
            Self::Disk => "--target-disk",
            Self::Frpc => "--frpc-config",
            Self::NetworkMac => "--network-mac",
            Self::Ipv4Address => "--ipv4-address",
            Self::Ipv4Gateway => "--ipv4-gateway",
            Self::Ipv6Address => "--ipv6-address",
            Self::Ipv6Gateway => "--ipv6-gateway",
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
            Self::Username => "1-32 ASCII letters, digits, _ or -; begin with a letter or _.",
            Self::SshPort => "Use a port from 1 to 65535. SSH default: 22.",
            Self::RootSize => {
                "Blank fills the remaining disk. Enter at least 512 MiB to leave unused space after root."
            }
            Self::BootSize => {
                "512-4096 MiB; blank uses 512. FAT32 mounted at /boot; generations must fit."
            }
            Self::Hostname => {
                "One ASCII label, 1-63 letters, digits or hyphens. Blank: use the existing hostname, then the target system name."
            }
            Self::Timezone => {
                "Optional IANA name, for example Etc/UTC, Asia/Shanghai or America/New_York."
            }
            Self::Disk => {
                "Whole-disk path such as /dev/vda, never a partition. Blank requests later detection; no disk scan has run here."
            }
            Self::Frpc => {
                "Optional file path or URL. This editor does not read or download the configuration."
            }
            Self::NetworkMac => {
                "Blank inherits source policy. Set a NIC MAC to supply an explicit policy for that interface."
            }
            Self::Ipv4Address => {
                "Address and prefix, such as 192.0.2.10/24. Leave blank when using DHCPv4."
            }
            Self::Ipv4Gateway => "Optional default gateway for the static IPv4 address.",
            Self::Ipv6Address => {
                "Address and prefix, such as 2001:db8::10/64. Leave blank when using DHCPv6 or router advertisements."
            }
            Self::Ipv6Gateway => {
                "Optional default gateway for the static IPv6 address. Link-local is accepted on the selected interface."
            }

            Self::DnsServers => {
                "1-3 upstream IPv4/IPv6 addresses, for example 1.1.1.1,2606:4700:4700::1111. Reachability is not tested here."
            }
            Self::DnsSearch => {
                "Optional: up to 6 ASCII domains, for example example.internal,lab.example. No protocol or path."
            }
            Self::Mtu => {
                "Optional: 1280-9000 for non-loopback interfaces. Use the provider's supported MTU; no automatic probing. Blank uses target defaults, not guaranteed source MTU inheritance."
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
                "64-65536 MiB; 1024 MiB=1 GiB, 2048 MiB=2 GiB. Target /swapfile, mode 0600 and activation at target boot."
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
pub(super) const ADDRESS_FIELDS: &[Field] = &[
    Field::NetworkMac,
    Field::Ipv4Address,
    Field::Ipv4Gateway,
    Field::Ipv6Address,
    Field::Ipv6Gateway,
];
pub(super) const TCP_FIELDS: &[Field] = &[
    Field::TcpBuffer,
    Field::Somaxconn,
    Field::SynBacklog,
    Field::NetdevBacklog,
    Field::MtuProbing,
    Field::Fastopen,
    Field::Syncookies,
];
