# Target kernel, hostname and time zone

CLI/TUI Save these settings to `InstallPlan.post_install`，Depend on Alpine Live Install scheduling application。**After the current entrance is confirmed, the real installation will be performed; four goals BIOS/UEFI acceptance remains pending。** This page does not provide online core replacement function for existing systems。

## configuration interface

```console
./rustaller debian --confhome https://your.example/reinstall/pinned-version \
  --kernel generic --hostname my-vps --timezone Etc/UTC
```

Example requires configuring authentication and completing disk identity confirmation; only added when checking `--inspect`。

| Target | `--kernel` choose | Bag/Way |
|---|---|---|
| Alpine | `standard`（default）、`virt`、`lts` | standard/lts for linux-lts，virt for linux-virt；no independence linux-standard Bag |
| Debian | `standard`（default）、`cloud`、`generic` | standard/generic for linux-image-amd64，cloud for linux-image-cloud-amd64 |
| Arch | `standard`（default）、`lts`、`cachyos` | linux、linux-lts、linux-cachyos；CachyOS requires explicitly enabling its repository |
| Gentoo | `auto`、`standard` | sys-kernel/gentoo-kernel-bin；reserve systemd stage3/GRUB/dracut route |

`auto` The first three are analyzed as standard；Gentoo still gentoo-kernel-bin。Alpine old cloud/generic respectively as virt/lts Aliases reserved。`--no-cloud-kernel` for Alpine/Debian Select the general kernel, and not auto of `--kernel` mutually exclusive。

Only the above options are accepted, arbitrary package names are not supported、URL、script hook or extra init system；CachyOS Are exceptions for explicitly authorized repositories. When no time zone is specified, no additional time zone package will be installed or the time zone will be overwritten.。

host name 1–63 Bit ASCII Letters, numbers, hyphens, the beginning and end must be letters or numbers. Time zone must be relative IANA name, such as `Etc/UTC`；Reject absolute paths, points/Empty path segments and control characters exist to target zoneinfo Subject to。ISO/netboot.xyz/reset/`hold=1` Target configuration not accepted。

not specified hostname When, first read the original system `/etc/hostname`，Read again `/proc/sys/kernel/hostname`；efficient FQDN Take the first tag and ignore it localhost/Invalid value, last used `alpine`、`debian`、`arch` or `gentoo`。If the current system configuration is left blank, it still means that it will not be modified.。

## Arch CachyOS

`--kernel cachyos` use[Official warehouse installation plan](https://wiki.cachyos.org/features/optimized_repos/)；Official script press glibc Available ISA Detect and select optimized warehouses to avoid just looking CPU model regardless of operating system support. Installer fixes resource digests and trust keys, retains package signature checks, only in isolation Arch Execute in target。

Perform a full package update and rebuild when the target is powered on for the first time. initramfs/GRUB；If it fails, the pending execution status is retained for retry. After success, repeated updates will be stopped and no additional automatic restart will be performed.。standard/LTS Not enabled CachyOS。This path is related to each ISA Warehouse still to be real VM acceptance。

## Application order and boundaries

The picture below retains the early stage schema 4 process diagram; currently schema for 6，The old plan is rejected and the actual calling sequence ends with `targets/install.rs` Subject to：

```text
CLI / TUI
  → InstallPlan.post_install
  → schema 4 Plan coding and private document handover
  → Live Calibration plan/Architecture/RAM
  → Target adapter selects the official kernel and required time zone package
  → Finish rootfs、Kernel/initramfs、GRUB、Network and SSH
  → shared::configure::apply_basic_configuration
  → first boot of the target system
```

`src/postinstall.rs` validate fields and prepare packages/File data; time zone required `tzdata`，Gentoo correspond `sys-libs/timezone-data`。`src/shared/configure.rs` Write in the inspected target `/etc/hostname`、`/etc/timezone`、`/etc/localtime`，Does not call the host hostnamectl/timedatectl。

Reject host before writing `/`、symbolic link root/Parent directories, path escapes and rescue plans. Time zone files must be limited to target zoneinfo tree, size limited and contains TZif header; allow target internal `UTC → Etc/UTC` link. Temporary files include rename File-by-file atomic publishing, cleaning up its own temporary files on failure; no cross-file transactions, the caller must exclusively occupy the target directory。

## Related configuration

`InstallPlan.system` Save root file system, files, tools/document/cache、DNS、network and optional swapfile。DNS From basic configuration application, network and swapfile Handled by respective components，Alpine Prepare to start the service offline. See[Configuration before installation](CONFIGURATION.md)and [minimal](MINIMAL.md)。

Native ssh-keygen belong host：The private key remains in the designated path, the password does not enter the plan, only the public key is handed over. Installation does not tune the network by default；CachyOS No additional kernel repositories or any command hooks are added.。
