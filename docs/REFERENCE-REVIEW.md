# debi / rustaller control review

read bohanwood/debi and bin456789/reinstall of README/Installation script; entry not executed, not modified Shell Four resources or fixed benchmark。

## Reference materials and trade-offs

- [bohanwood/debi README](https://github.com/bohanwood/debi/blob/master/README.md)
  and [debi.sh](https://github.com/bohanwood/debi/blob/master/debi.sh)：around minimal
  Debian、explicit network/Account/Hostname configuration, native installer and inspectable installation configuration organization process。
  This project draws on the principles of configuration integrity and native tools and continues to retain debootstrap and Alpine Live；
  Not reintroduced Debian Installer Or additional firmware restart。
- [reinstall README](https://github.com/bin456789/reinstall/blob/main/README.md)
  and [trans.sh](https://github.com/bin456789/reinstall/blob/main/trans.sh)：emphasize
  VPS Special routing, disk identity、BIOS/EFI、Device driver and installation environment recoverability. This project already has
  MAC binding/networkd on-link Routing, disk identity checking, recovery SSH；Make up for one upstream this round
  explicitly dealt with Hyper-V PCI driver path; do not infer compatibility from platform names。
- Do not copy README The ”full support” statement does not extend to Windows/ARM，No reduction in signature verification，
  Do not enable any script hooks and do not port region detection。EFI Stub Canceled。

Read this time Git blob SHA（Not a warehouse commit）：

| document | blob SHA |
|---|---|
| debi README.md | 129dcfd2e52c9ada725ba7bbc62f05f37229b43c |
| debi debi.sh | 10ebcd2141f23271f59eb46e4174539f74fc9293 |
| reinstall README.md | b22b3a2ea9ec92315424bf3864695be3e8832aa5 |
| reinstall trans.sh | 345718c2e6e6277fd694f102cd547f3fc15e6bc6 |

## Fixed

1. Write root before kernel configuration/ESP UUID fstab，for final native initramfs rebuild reads; retain the final rebuild without claiming retrospective guarantees bootstrap hook。
2. write for new goals localhost、127.0.1.1 hostname and IPv6 loopback；Don’t change current entrance hosts。
3. Alpine Module added pci-hyperv，Replenish VMBus；The configuration cannot generate it when the target kernel does not contain a driver.。
4. Finally with chroot `grub-probe --target=fs /` Require XFS，Use together `grub-script-check` Verify configuration; failure will not restart, but passing does not mean actual startup。
5. The document clearly shows that the example is a real installation entry. Only check the required `--inspect`。

## verification and remaining limits

277 workspace + 49 Shell Test passed; contains wrong file system, invalid GRUB configuration、hostname
Regression testing of parsing and repeated writing。Rustfmt、Clippy、ShellCheck、Bash/ash and diff examine
pass. Log in /workspace/scratch/reference-check.log。

Not running new full VM Install。P0 Live Repair retest, four releases BIOS/UEFI Installation matrix、
XFS swap Actual activation, provider specific network and real KVM/Hyper-V/VMware acceptance remains pending。
This round is a targeted review of the configuration and installation finalization path, not a defect-free certification of the entire project.。
