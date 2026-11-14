# Four systems minimal Design and configuration

minimal Preserve boot, network、SSH With native package maintenance capabilities, package planning, configuration and cleanup plans have been implemented；**whole minimal Install/Startup and system size have not yet been accepted。**

| system | basic route | Streamline scope and retention items |
|---|---|---|
| Alpine | alpine-base / BusyBox / OpenRC；default linux-lts，Optional virt/LTS；OpenSSH、dhcpcd、CA、GRUB、Selected file system tools | Not actively selecting documents/language/desktop/Compilation tools; maintains necessary firmware, module and package databases |
| Debian | debootstrap minbase，APT Do not install recommends；default generic，Optional cloud；systemd/networkd/resolved、OpenSSH、CA、GRUB | Already the minimum basic route；dpkg Exclude documents/man/info/translate, preserve copyright |
| Arch | pacstrap base、archlinux-keyring、pacman、systemd/networkd、OpenSSH、CA、linux/linux-lts or explicitly CachyOS、GRUB | pacman NoExtract Exclude documents/Translate and retain copyright; maintain signature verification, reject repository level SigLevel Never Waiting for bypass |
| Gentoo | systemd stage3、gentoo-kernel-bin/dracut、GRUB | Keep Portage/Python/tool chain; use FEATURES=nodoc noman noinfo，Do not change init/Kernel installation route |

container rootfs、Neither the kernel metapackage nor the compressed download means it is bootable. VPS the complete volume of。2026-10-25 Official package page observation：Alpine linux-virt about 49.4 MiB、Arch linux about 161.6 MB；Gentoo stage3 Hundreds of downloads have been made MB，None of them contain all dependencies/guide/File system overhead。

refer to：[Alpine](https://pkgs.alpinelinux.org/package/v3.24/main/x86_64/linux-virt)、[Debian](https://packages.debian.org/trixie/linux-image-cloud-amd64)、[Arch](https://archlinux.org/packages/core/x86_64/linux/)。This is a historical packet page observation, not a system measurement or load certification; see Trust Boundary [SOURCES.md](SOURCES.md)。

configuration option：

| Options | Behavior |
|---|---|
| `--profile standard\|minimal` / `--minimal` | default standard；minimal Start cutting plan |
| `--with-tools` | Optional curl/less，Gentoo Use the corresponding package atom |
| `--keep-docs` | minimal Preserve documents and translations |
| `--keep-package-cache` | minimal Keep download cache |
| `--kernel` | For supported kernel selections, see[mapping](POSTINSTALL.md)；Debian default standard/generic |
| `--filesystem xfs\|ext4` | default XFS，minimal Do not change file system selection |
| DNS、network stack、swapfile | See [Configuration before installation](CONFIGURATION.md)；swapfile Will increase disk usage and is turned off by default |

TUI Save draft files by target，standard Do not submit minimal hidden entries; share schema 6 plan; rescue/ISO/netboot.xyz/reset Target configuration not accepted。

`targets::packages::package_plan` List necessary packages/stage, not a complete dependency closure；`apply_package_policy` Write native filtering configuration and return `CleanupPlan`，Do not delete files directly/cache。debootstrap/stage3 The expanded document must be mounted by the target/Clean after life cycle check, retain copyright files。

Must keep: startup files, hardware required firmware and kernel modules、CA、Time zone data, license/Copyright, package database, software repository and normal update capabilities. Minimum Physics RAM still Alpine/Debian 512 MB、Arch 1024 MB、Gentoo 2048 MB，Not based on download size or swapfile lower the requirement。

acceptance separately records compressed downloads and root/boot Occupy, cache, start RAM、free space; verification SSH、IPv4/IPv6、Renew lease DNS、reboots and package updates. Report partition capacity, usage and download size separately。
