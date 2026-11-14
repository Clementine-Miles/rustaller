# Pre-installation configuration and one-time restart target

configuration passes TUI/CLI write schema 6 plan。**After confirming the disk identity, a real installation will be performed; only checking the required `--inspect`。** four goals BIOS/UEFI Full acceptance is still pending, see[acceptance records](ACCEPTANCE.md)。Native ssh-keygen The specified local file will be written immediately。

## Execution order

The target processes of the four automatic installations are adjusted as follows:：

```text
in the original system TUI：Complete all configurations, public key preparation, and final review
  → Verify disk identity, existing network、RAM、kexec Permissions and kernel capabilities
  → verify official resource signatures/summary, preparation Alpine Live match worker
  → Put private plan into initrd，Don’t stuff passwords into kernel parameters
  → kexec switch to Alpine memory environment（SSH Will disconnect without going through firmware/GRUB reboot）
  → Live Verify disk again/RAM，Install rootfs、Account、SSH、Network, Kernel and Boot
  → Write target DNS/Network configuration, optionally create target swapfile，Complete inspection
  → Reboot for the last time and enter the new system with complete configuration.
```

Rust pass kexec Enter Live，Only final restart after successful installation verification; does not return silently Shell Two restart processes. original system PID 1 must be systemd/openrc-init；preflight requires kernel kexec configuration; not disabled/lockdown and available tools, actual loading and platform compatibility still need to be accepted。ISO/netboot.xyz independent interactive medium；`hold=2` eventually restarted by the user。

## Accounts, hostnames, and root file systems

Create non- root User must set another root Account password；TUI Enter twice and mask, interact CLI Ask privately when parameters are missing. Non-interactive calls must provide `--root-password`，Its value may be exposed to shell History or process parameters, interactive input is preferred。root Passwords and user passwords/Public key authentication is independent and is not enabled. root SSH。

The host name will first inherit the effective name of the original system. If it cannot be detected, the release name will be used; explicit `--hostname` priority. See details[target configuration](POSTINSTALL.md)。

`--filesystem xfs|ext4` Select the root file system after the entire disk installation, the default XFS；TUI lie in Disk and boot。Live Check selected modules before liquidation，fstab、initramfs and GRUB configure according to the selection。ESP still FAT32，Gentoo of systemd/The kernel route remains unchanged。

## DNS Select and enter

TUI Optional Automatic、Cloudflare、Google、Quad9 or Custom；CLI of `--dns` for customization IP。Automatic mode does not submit static values; custom drafts are retained when switching providers, but only the current options are submitted. closure IPv6 The default is to include only IPv4，Otherwise, submit at most two IPv4 and a IPv6。

Custom Accept comma separated 1–3 indivual IP，Remove peripheral spaces item by item; reject empty items, duplicate addresses, domain names, control characters, and loopbacks/multicast/Unspecified address and unscoped link local IPv6。Search domains are independently verified. Will not silently swallow input errors or change invalid custom values ​​to defaults。

## Network reference script choices

2026-10-26 Read only `https://box.dmitstock.com`：534595 byte，SHA-256 `b70844acb181b9b6c82f0904b682bdfb5908d188c3b73328568edc21e6766fda`。Not executed, copied, or used as a runtime dependency; remote content may change。

Preserves options mappable to quad-target native configurations：

| configuration | default / scope | effect |
|---|---|---|
| DNS | Automatic, public preset or custom 1–3 indivual IPv4/IPv6 | static parser and DHCP/RA DNS Override control |
| search domain | None; most 6 indivual ASCII domain, total 255 byte | DNS search path |
| congestion control | default / bbr / cubic | BBR Must match fq，and verify the target kernel configuration |
| default queue | default / fq / fq_codel | Verify the target kernel configuration and prepare native loading configuration in module form |
| IPv6 | inherit / enable / disable | Only controls the new system IPv6 Disabled flag; do not force modification RA/Forward/address |
| Address priority | default / ipv4 | Debian/Arch/Gentoo of glibc gai.conf；Alpine/musl hide and reject ipv4 Options |
| network card MTU | Leave blank and use；1280–9000 | Only modify non- lo Interface, does not run speed test/detection |
| TCP/socket buffer limit | Leave blank and use；1–64 MiB | set up socket maximum sum TCP Triplet, no fixed size memory reserved |
| somaxconn / SYN / netdev queue | Leave blank and use；128–262144 | Corresponding monitoring/SYN/Packet receiving backlog queue |
| TCP MTU probing | Leave blank and use；0 / 1 / 2 | closure / probe when black-holed / Always detect |
| TCP Fast Open | Leave blank and use；0–3 | closure / client / Server / Both, applications must still support |
| SYN cookies | Leave blank and use；0 / 1 | Explicitly turn off or on |

TCP tuning has no fixed preset. Every retained value is an explicit manual option; leaving it blank preserves the target kernel default. Configuration generation does not prove activation or performance.

Not included in network configuration: dynamic speed measurement/BDP/HTB、NAT66 random outbound selection、IPv6 Address pools, full firewall rules, provider-specific ”defaults”, obsolete `tcp_adv_win_scale`、`kernel.panic=-1`、force RA=2、Automatically restart current network services, etc. The kernel only provides[clearly listed choices](POSTINSTALL.md)。

configuration applies only to the target，Live Use the original system network. refuse to disable IPv6 and IPv6 DNS Same election, and must prove IPv4 path available。TUI Manual is not available yet IP/Route replacement；IPv6-only Separate acceptance with provider-specific network。

### Native target pipeline

`InstallPlan` plus the validated deployment manifest → pure `shared::network::render_network_configuration` → installed-kernel and complete output/service validation → per-file publication and readback → final target verification. Network files are never reparsed to add DNS or MTU.

- `shared::network` owns ifupdown/dhcpcd/udhcpc or MAC-matched networkd output, DNS overrides, MTU, IPv6 policy, sysctl, modules and glibc GAI policy. `DnsConfig` remains the sole typed DNS policy.
- Static DNS owns a regular resolver and suppresses DHCPv4/DHCPv6/RA DNS and domains. Automatic systemd DNS replaces bootstrap DNS with the native resolved link after package operations; automatic Alpine DNS remains with native lease clients. Unknown resolver symlinks are rejected.
- Captured DHCP/RA addresses and gateways are snapshots, not static boot routes. Static provider gateways retain on-link routing for /32 and /128 addresses. IPv6 disable removes all generated IPv6 addresses, routes, DHCPv6 and RA.
- `shared::configure` owns bounded file reads, path/ownership checks, complete-batch validation and publication. Each regular file is replaced and read back; replacement is atomic per file, not across the batch. The caller must exclusively own the target tree.
- Requested BBR/qdisc and explicitly enabled IPv6 or SYN cookies require installed-kernel capability evidence; module payloads are checked before publication. Arch can infer module capabilities from its authenticated inventory, but cannot infer SYN cookies without Kconfig. Files written offline do not establish runtime activation; boot acceptance is still pending.
- Existing package/OpenRC service policy, isolated XFS/ext4 swapfile allocation and hostname/timezone configuration remain in their existing Rust modules. No live administration or independent recovery supervisor is installed.

See [reference matrix and evidence limits](DMITBOX-REVIEW.md).

## SSH Key generation

menu：`Account and SSH → Authentication settings → SSH public key → Public key management → Generate key`。

| Options | default / Support value |
|---|---|
| type | ed25519；Also optional RSA |
| KDF Number of rounds `-a` | 512；1–65536 |
| encryption algorithm `-Z` | aes256-ctr；Also aes256-gcm@openssh.com、chacha20-poly1305@openssh.com，requires native tool support |
| Comment `-C` | system `date +%Y%m%d` Result; editable |
| RSA Number of digits `-b` | 4096；2048/3072/4096/8192，only RSA show |
| save location `-f` | Absolute path of the local machine; defaults to the current user `.ssh/id_rustaller_ed25519` |

Parameters are independently passed into the system ssh-keygen，not pass Shell。Exit while generating raw/Alternate screen where native tool asks for password and restores after completion TUI；Passwords are not passed through fields, plans, environments or argv，Empty password None KDF Protect. Missing tools are not automatically installed。

The private staging directory is 0700；Deny overrides, symbolic links, and parent directories writable by others. private key 0600、public key 0644，Only public keys join the certification program。**This is the client login key, which must be backed up to your own client before liquidation.。**Commonly add public key support files、URL、github:/gitlab:。

## Target swapfile

reserve GRUB BIOS/UEFI。Deductions before liquidation ESP/Aligned, root at least 512 MiB；This is just the lower limit for formatting。`mkfs.xfs -K` Skip entire volume discard，Do not modify the mount/TRIM Strategy。dd After synchronization, check the length and the number of allocated blocks, and the target still needs to be actually activated for acceptance.。

menu：`Target swapfile → enable → size MiB / swappiness`。not created by default; added at the user’s explicit request Rust options; retain Shell Baseline None swap Behavior。

- fixed target `/swapfile`，64–65536 MiB。`bs=1M count=2048` yes **2048 MiB = 2 GiB**；1 GiB use 1024 MiB。
- Only accept independent mounts and match the plan XFS/ext4 Target root; check free space first and leave at least 512 MiB。This margin is the minimum protection and is not a guarantee of sufficient space for subsequent updates; it should be executed after the software package is installed.。
- Live call in GNU dd（bs=1M、count=selected MiB、conv=notrunc,fsync、status=progress）Write complete zeros to avoid sparse files；0600；Call the system only for this newly created, not yet enabled target file `mkswap`。
- Do not overwrite existing swapfile、Don’t change what you have swap Entry; target after success fstab add once `/swapfile none swap defaults 0 0`。
- Optional swappiness for 0–200，Write target sysctl document。systemd pass fstab generator enable；Alpine Enable goals offline OpenRC swap Services (and necessary sysctl）。
- **original system/Live Not executed swapon、swapoff or modify it fstab**。is enabled during normal target startup without another reboot. Minimum installation RAM Not because of swapfile Change。

`CommandRunner` overall situation swap Interception remains in effect; only specific mkswap Called with exact exception. Test with mock Intercept formatting without actually calling mkswap/swapon。Multiple file publishing occurs I/O partial target configuration may remain on error; the installer must stop and provide inspection details。

## CLI Examples and execution boundaries

```console
./rustaller debian --confhome https://your.example/reinstall/pinned-version \
  --minimal --kernel cloud --hostname my-vps --timezone Etc/UTC \
  --ssh-key /root/client.pub --ssh-port 2222 \
  --dns 1.1.1.1 --dns 2606:4700:4700::1111 \
  --tcp-congestion bbr --qdisc fq --tcp-buffer-mib 16 \
  --tcp-mtu-probing 1 --tcp-fastopen 3 --tcp-syncookies 1 \
  --mtu 1450 --swapfile-mib 2048 --swappiness 10
```

The example will be entered after confirmation Live Liquidation; only inspection required `--inspect`。Can be added `--filesystem ext4` choose ext4；Create non- root Users need to be independent root password。schema 6 Reject the old plan; key generation is performed immediately by the native tool and is not handed over to worker。
