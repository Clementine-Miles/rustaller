# Installation source and verification boundaries

The four automatic targets all start from Alpine Live is executed, but the source of the target root system is different. Currently there are no additional four sets ISO Download framework; user specifies local ISO of GRUB loopback The interface remains independent。

| Target/stage | Use resources | official HTTPS source | due chain of trust |
|---|---|---|---|
| public Alpine Live | x86_64 netboot Kernel、initramfs、modloop | `dl-cdn.alpinelinux.org/alpine` | official netboot tar signature、SHA256，from verified tar extract |
| Alpine Target | Stable version APK storehouse | Same as above | APK Trust keys and package signatures |
| Debian Target | debootstrap minbase、APT storehouse | `deb.debian.org/debian`、`security.debian.org/debian-security` | Debian archive keyring → InRelease/Release.gpg → Package Index and Package Summary |
| Arch Target | pacstrap、core/extra software warehouse | `geo.mirror.pkgbuild.com` | Credible `archlinux-keyring`，Package signature is required, database signature is optional |
| Arch Optional CachyOS | Official warehouse installation script, optimized warehouse and linux-cachyos | `mirror.cachyos.org` | fixed archive SHA256、Complete master key fingerprint, native pacman package signature |
| Gentoo Target | amd64 systemd stage3、software warehouse | `distfiles.gentoo.org` | Credible Gentoo release key → signature index/DIGESTS → selected stage3 SHA512 |

GitHub Release Distributions, distribution resources and packages come from their respective official sources. The static entry does not contain the complete distribution or installation media; the keys and tools required for authentication are established at each installation stage。

## Optional CachyOS storehouse

only Arch explicit selection CachyOS Used in kernel[Official warehouse plan](https://wiki.cachyos.org/features/optimized_repos/)。The resource is `https://mirror.cachyos.org/cachyos-repo.tar.xz`，fixed SHA256 `a1e3a74bdd12c10ac7317079e3e8c8e7d5557d818e4d4021364005a336b6afdc`，Signing master key fingerprint `882DCFE48E2051D48E2562ABF3B607488DB35A47`。Stop when the digest does not match, and the fixed value must be updated after re-auditing.。

Download and verify archives before liquidation/full key, then only in quarantine Arch The target executes the audited official script: convert the short key ID Boot replacement with fixed full fingerprint, using non-interactive pacman，and make configuration write errors fail gracefully. Script basis glibc loader Reports are available ISA choose v3/v4 or applicable znver4 warehouse; below v3 Use official baseline `[cachyos]`。Package signing remains enabled and arbitrary scripts are not allowed URL。

first-boot `rustaller-cachyos-update.service` Repair on network card、network-online and networkd-wait-online After execution `pacman -Syu --noconfirm`、`mkinitcpio -P`、`grub-mkconfig`。If a failure occurs, keep the mark and try again; remove the mark only after all successes, and no additional automatic restart will be required.。standard/LTS Do not enable this repository。

## Rust Source parsing and read-only online auditing

`src/host/sources.rs` maintain official addresses and four resource types centrally, reusing existing Alpine/Debian Stable version analysis. it only accepts Linux x86_64 Target：

- Alpine：Choose the only one `alpine-netboot` record, bind branch、full version、x86_64 file name and SHA256；check official `.sha256` and YAML consistent. No digests are made for individual kernel files URL。
- Debian：examine Origin、stable、amd64 and only version/codename，Will InRelease Pin to selected codename；Check that the distribution identity in the signed message is consistent with the original metadata。
- Arch：Explicitly output the software repository and `Required DatabaseOptional`/`archlinux-keyring` request, do not regard it as ISO or stage3 download。
- Gentoo：from clearsigned Select a unique timestamp directory in the index and `stage3-amd64-systemd` file; corresponding to clearsigned DIGESTS Select the file’s only SHA512。reject other architectures、OpenRC、Directory traversal, external sites URL、duplicate records and invalid summaries。

Execute on development machine：

```console
cargo xtask audit-sources
```

Audit system `curl --disable` Disable `.curlrc`，inheritance agent/CA、Keep TLS，only allowed HTTPS/HTTPS redirects with bounded timeouts and retry counts; only GET most every time 1 MiB metadata/Digest, large files and signatures only HEAD；Failure will not roll back to unknown images or old versions.。

Output distinguished reachability, summary parsing/sidecar Consistent and yet to be executed signatures/Complete file verification。ASCII armor Parsing without verification OpenPGP；Audit does not download keys, install software or execute partitions/guide. The actual installation must first verify the trust key, signature and complete payload before decompressing and using it; the current driver is connected, but the read-only audit does not prove that the installation was successful.。

## 2026-10-25 Online observation

The following is the metadata obtained from this query, not the hard-coded installation version. Stablize/The scroll pointer is updated each time the source parser re-parses, and the exact resource should be bound thereafter。

| Query | Observations |
|---|---|
| Alpine latest-stable x86_64 YAML | 3.24.3、branch v3.24、2026-10-21；netboot tar SHA256 for `0228553464c9588ffeb63ec04d26abb5f79405006b74574034394199af47dab4` |
| Alpine netboot tar `.sha256` / `.asc` | Available；`.sha256` and YAML consistent |
| Alpine HTTPS virt Kernel/initramfs/modloop | three HEAD All requests were successful |
| Alpine single file `netboot/vmlinuz-virt.sha256` | 404；It cannot be assumed that such sidecar |
| Debian stable Release | 13.7、trixie；stable and trixie-security of InRelease Reachable |
| Arch core.db | Reachable；core.db.sig for 404，Therefore, the database signature cannot be regarded as a required attachment. |
| Arch Official image SHA256 list | have 2026.10.01 ISO/bootstrap；these two files are not current pacstrap Resources required for the path |
| Gentoo latest-stage3-amd64-systemd | `20261018T124555Z/stage3-amd64-systemd-20261018T124555Z.tar.xz`，297255400 byte |
| Gentoo correspond stage3、DIGESTS、`.asc` | HEAD/Small file request successful；DIGESTS for clearsigned，Include SHA512/BLAKE2B |
| helper Fedora 44 GRUB EFI document | Original HTTPS CloudFront address HEAD success；Fedora Not an installation target |

The above historical audit is not downloaded/hash complete tar/ISO，Not verified either OpenPGP。Kernel package page check coverage Alpine virt/lts、Debian cloud/generic、Arch lts、Gentoo kernel-bin，Eventually all returned 200；Debian generic first 503，Bounded retry successful. Web reachability does not replace package signing/summary。

## reserve Shell Issues to be replaced in the baseline

Shell The file is a behavioral comparison baseline, and its installation entry will not be modified or actually run in this round.：

- `reinstall.sh` of Alpine netboot The address is still in use HTTP，Notes Live School time is later than initrd start up. The final implementation should differentiate host side HTTPS Download and Live Clock initialization, cannot be turned off TLS Verification solution。
- `trans.sh` of Arch pacman configuration specifies `SigLevel = Never`。final Rust The installation link must not inherit it and needs to establish a trusted keyring and requires package signing; the absence of a database signature does not constitute a reason to turn off package signing.。
- Gentoo stage3 Currently, you can decompress it directly after downloading, and there is no corresponding consumption. DIGESTS/sign. The final implementation must first verify trusted signatures and integrity stage3 summary。
- old `test_url` accessibility/Format checking is not cryptographic verification；Rust Stable version text parsing itself is not signature verification。

These are reserved Shell Known issues with and cannot be copied to Rust。Parsing or read-only auditing cannot prove a fix, and the full chain of trust and installation must be independently verified.。
