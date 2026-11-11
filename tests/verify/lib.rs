#![cfg(test)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn bash() -> String {
    std::env::var("TEST_BASH").unwrap_or_else(|_| "bash".into())
}
fn source(file: &str) -> String {
    fs::read_to_string(root().join(file)).unwrap()
}
fn definition(text: &str, name: &str) -> String {
    let start = text
        .find(&format!("\n{name}() {{"))
        .map(|p| p + 1)
        .or_else(|| text.strip_prefix(&format!("{name}() {{")).map(|_| 0))
        .unwrap_or_else(|| panic!("missing function {name}"));
    let remainder = &text[start..];
    let mut end = remainder.len();
    let mut offset = 0;
    let mut heredoc: Option<String> = None;
    for line in remainder.split_inclusive('\n') {
        if let Some(delimiter) = heredoc.as_deref() {
            if line.trim_end_matches('\n') == delimiter {
                heredoc = None;
            }
            offset += line.len();
            continue;
        }
        if let Some(marker) = line.split("<<'").nth(1)
            && let Some(delimiter) = marker.split('\'').next()
        {
            heredoc = Some(delimiter.to_owned());
            offset += line.len();
            continue;
        }
        if offset > 0 && !line.starts_with(char::is_whitespace) && line.contains("() {") {
            end = offset;
            break;
        }
        if line.starts_with("# 脚本入口") {
            end = offset;
            break;
        }
        offset += line.len();
    }
    // Only definitions, not adjacent top-level initialization (first function in upstream).
    let selected = remainder[..end].trim_end();
    let close = selected
        .rfind("\n}")
        .expect("missing top-level function close")
        + 2;
    selected[..close].to_owned()
}
fn upstream(file: &str) -> String {
    let out = Command::new("git")
        .current_dir(root())
        .args([
            "show",
            &format!("98f1e21013233d42750cc2e7a298a0818461d686:{file}"),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "upstream baseline must remain in Git");
    String::from_utf8(out.stdout).unwrap()
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
struct Temp(PathBuf);
impl Temp {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "reinstall-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn run(defs: &str, body: &str) -> std::process::Output {
    Command::new(bash())
        .args([
            "--noprofile",
            "--norc",
            "-c",
            &format!("set -e; {defs}\n{body}"),
        ])
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .unwrap()
}
fn ok(out: std::process::Output) -> String {
    assert!(
        out.status.success(),
        "status {:?}\nstdout {}\nstderr {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn defs(file: &str, names: &[&str]) -> String {
    let text = source(file);
    names
        .iter()
        .map(|n| definition(&text, n))
        .collect::<Vec<_>>()
        .join("\n")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_asset_map_and_compatibility_marker_are_consistent() {
        let assets = [
            "trans.sh",
            "initrd-network.sh",
            "fix-eth-name.sh",
            "logviewer.html",
        ];
        for removed in [
            "ttys.sh",
            "get-frpc-url.sh",
            "fix-eth-name.service",
            "fix-eth-name.initd",
            "frpc.service",
        ] {
            assert!(
                !root().join(removed).exists(),
                "merged runtime asset {removed} remains"
            );
        }
        let callers = [source("reinstall.sh"), source("trans.sh")];
        let combined = callers.join("\n");
        for asset in assets {
            assert!(
                root().join(asset).is_file(),
                "missing runtime asset {asset}"
            );
            assert!(
                combined.contains(asset),
                "unreferenced runtime asset {asset}"
            );
            for doc in ["README.md", "README.en.md", "docs/ARCHITECTURE.md"] {
                assert!(source(doc).contains(asset), "{doc} omits {asset}");
            }
        }
        for text in &callers {
            for line in text
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
            {
                for part in line.split("$confhome/").skip(1) {
                    let name = part
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || ".-_".contains(*c))
                        .collect::<String>();
                    if !name.is_empty() {
                        assert!(
                            assets.contains(&name.as_str()),
                            "unknown runtime asset {name}"
                        );
                    }
                }
            }
        }
        let front = callers[0]
            .lines()
            .find(|line| line.starts_with("SCRIPT_VERSION="))
            .unwrap();
        let runtime = callers[1]
            .lines()
            .find(|line| line.starts_with("SCRIPT_VERSION="))
            .unwrap();
        assert_eq!(front, runtime);
        let marker = front.strip_prefix("SCRIPT_VERSION=").unwrap();
        for doc in ["README.md", "README.en.md", "docs/ARCHITECTURE.md"] {
            assert!(source(doc).contains(marker));
        }
    }
    #[test]
    fn x86_tty_helper_and_network_service_ordering() {
        let ttys = defs("trans.sh", &["get_ttys"]);
        let temp = Temp::new("ttys");
        let dev = temp.0.join("dev");
        fs::create_dir_all(&dev).unwrap();
        std::os::unix::fs::symlink("/dev/null", dev.join("ttyS0")).unwrap();
        let ttys = ttys.replace(
            "local tty_root=/dev",
            &format!("local tty_root={}", quote(dev.to_str().unwrap())),
        );
        let setup = format!("stty(){{ return 0; }}; {ttys}");
        assert_eq!(
            ok(run(&setup, "get_ttys console=")),
            "console=ttyS0,115200n8 console=tty0"
        );
        assert_eq!(ok(run(&setup, "get_ttys log:")), "log:ttyS0");
        assert!(!source("trans.sh").contains("ttyAMA0"));
        assert!(!root().join("get-xda.sh").exists());
        let systemd = source("tests/fixtures/fix-eth-name-systemd.service");
        assert!(systemd.contains("Before=networking.service"));
        assert!(systemd.contains("Before=systemd-networkd.service"));
        for retired in ["NetworkManager", "network.service", "cloud-init-local"] {
            assert!(!systemd.contains(retired));
        }
        assert!(
            source("tests/fixtures/fix-eth-name-openrc.initd")
                .contains("before net networking dhcpcd")
        );
    }
    #[test]
    fn generated_service_templates_match_previous_resources_byte_for_byte() {
        let writers = defs(
            "trans.sh",
            &[
                "write_fix_eth_name_systemd_service",
                "write_fix_eth_name_openrc_service",
                "write_frpc_systemd_service",
            ],
        );
        let temp = Temp::new("service-templates");
        for (writer, fixture, output) in [
            (
                "write_fix_eth_name_systemd_service",
                "tests/fixtures/fix-eth-name-systemd.service",
                "fix-eth-name.service",
            ),
            (
                "write_fix_eth_name_openrc_service",
                "tests/fixtures/fix-eth-name-openrc.initd",
                "fix-eth-name.initd",
            ),
            (
                "write_frpc_systemd_service",
                "tests/fixtures/frpc.service",
                "frpc.service",
            ),
        ] {
            let path = temp.0.join(output);
            ok(run(
                &writers,
                &format!("{writer} {}", quote(path.to_str().unwrap())),
            ));
            assert_eq!(
                fs::read(path).unwrap(),
                fs::read(root().join(fixture)).unwrap(),
                "generated output differs for {writer}"
            );
        }
    }
    #[test]
    fn add_systemd_service_generates_and_enables_local_units() {
        let code = defs(
            "trans.sh",
            &[
                "write_fix_eth_name_systemd_service",
                "write_frpc_systemd_service",
                "add_systemd_service",
            ],
        );
        let temp = Temp::new("generated-systemd-units");
        let os_dir = temp.0.join("target");
        fs::create_dir_all(os_dir.join("usr/lib/systemd/system-preset")).unwrap();
        let trace = temp.0.join("chroot.trace");
        let setup = format!(
            "chroot(){{ printf '%s\\n' \"$*\" >>{}; }}; error_and_exit(){{ exit 77; }}; {code}",
            quote(trace.to_str().unwrap())
        );
        for service in ["fix-eth-name", "frpc"] {
            ok(run(
                &setup,
                &format!(
                    "add_systemd_service {} {service}",
                    quote(os_dir.to_str().unwrap())
                ),
            ));
        }
        assert_eq!(
            fs::read_to_string(trace).unwrap(),
            format!(
                "{} systemctl enable fix-eth-name.service\n{} systemctl enable frpc.service\n",
                os_dir.display(),
                os_dir.display()
            )
        );
        for (service, fixture) in [
            ("fix-eth-name", "fix-eth-name-systemd.service"),
            ("frpc", "frpc.service"),
        ] {
            assert_eq!(
                fs::read(os_dir.join(format!("etc/systemd/system/{service}.service"))).unwrap(),
                fs::read(root().join("tests/fixtures").join(fixture)).unwrap()
            );
            assert_eq!(
                fs::read_to_string(
                    os_dir.join(format!("usr/lib/systemd/system-preset/01-{service}.preset"))
                )
                .unwrap(),
                format!("enable {service}.service\n")
            );
        }
    }
    #[test]
    fn auxiliary_network_fixes_only_handle_generated_target_formats() {
        let text = source("fix-eth-name.sh");
        for retired in [
            "fix_rh_sysconfig",
            "fix_suse_sysconfig",
            "fix_network_manager",
            "fix_netplan",
            "/etc/sysconfig/",
            "/etc/NetworkManager/",
            "/etc/netplan/",
            "openeuler",
        ] {
            assert!(!text.contains(retired), "obsolete target backend {retired}");
        }
        assert!(text.contains("fix_ifupdown"));
        assert!(text.contains("fix_systemd_networkd"));
    }
    #[test]
    fn nic_lookup_matches_upstream_including_azure_slave() {
        let names = ["to_lower", "get_ethx_by_mac"];
        let old = names
            .iter()
            .map(|name| definition(&upstream("fix-eth-name.sh"), name))
            .collect::<Vec<_>>()
            .join("\n");
        let new = defs("fix-eth-name.sh", &names);
        let mock = "ip(){ printf '%s\\n' '2: ens3: <UP> link/ether 00:11:22:33:44:55' '3: enp5s0: <UP> link/ether 00:11:22:33:44:66' '4: ens7: <UP> master ens3 link/ether 00:11:22:33:44:55'; };";
        for args in [
            "00:11:22:33:44:55",
            "00:11:22:33:44:55 slave",
            "00:11:22:33:44:66",
            "AA:BB:CC:DD:EE:FF",
        ] {
            let a = run(
                &format!("{mock}\n{old}"),
                &format!("get_ethx_by_mac {args}"),
            );
            let b = run(
                &format!("{mock}\n{new}"),
                &format!("get_ethx_by_mac {args}"),
            );
            assert_eq!(a.status.success(), b.status.success());
            assert_eq!(a.stdout, b.stdout, "MAC lookup: {args}");
        }
    }
    #[test]
    fn ifupdown_first_boot_fix_matches_upstream_byte_for_byte() {
        let temp = Temp::new("first-boot-ifupdown");
        let input = "auto lo\niface lo inet loopback\n# mac 00:11:22:33:44:55\nauto eth0\niface eth0 inet static\n    address 192.0.2.1/32\niface eth0 inet6 static\n    address 2001:db8::1/128\n    post-up ip route add default via 2001:db8::ff dev eth0\n# mac 00:11:22:33:44:66\nallow-hotplug eth1\niface eth1 inet dhcp\n";
        let mock = "get_ethx_by_mac(){ case $1 in *55) echo ens3;; *66) echo enp5s0;; *) return 1;; esac; };";
        let mut outputs = Vec::new();
        for (label, code) in [
            (
                "old",
                definition(&upstream("fix-eth-name.sh"), "fix_ifupdown"),
            ),
            ("new", defs("fix-eth-name.sh", &["fix_ifupdown"])),
        ] {
            let etc = temp.0.join(label).join("etc");
            fs::create_dir_all(etc.join("network")).unwrap();
            let file = etc.join("network/interfaces");
            fs::write(&file, input).unwrap();
            let isolated = code.replace("/etc/", &format!("{}/", etc.display()));
            ok(run(&format!("{mock}\n{isolated}"), "fix_ifupdown"));
            outputs.push(fs::read(file).unwrap());
        }
        assert_eq!(outputs[0], outputs[1]);
        assert!(
            String::from_utf8(outputs[1].clone())
                .unwrap()
                .contains("allow-hotplug enp5s0")
        );
    }
    #[test]
    fn networkd_first_boot_fix_matches_upstream_byte_for_byte() {
        let temp = Temp::new("first-boot-networkd");
        let mock = "get_ethx_by_mac(){ case $1 in *55) echo ens3;; *66) echo enp5s0;; *) return 1;; esac; };";
        let mut outputs = Vec::new();
        for (label, code) in [
            (
                "old",
                definition(&upstream("fix-eth-name.sh"), "fix_systemd_networkd"),
            ),
            ("new", defs("fix-eth-name.sh", &["fix_systemd_networkd"])),
        ] {
            let etc = temp.0.join(label).join("etc");
            let dir = etc.join("systemd/network");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("10-cloud-init-eth0.network"),
                "[Match]\nMACAddress=00:11:22:33:44:55\nName=eth0\n[Network]\nDHCP=yes\n",
            )
            .unwrap();
            fs::write(
                dir.join("10-cloud-init-eth1.network"),
                "[Match]\nMACAddress=00:11:22:33:44:66\n[Network]\nAddress=2001:db8::1/128\n",
            )
            .unwrap();
            let isolated = code.replace("/etc/", &format!("{}/", etc.display()));
            ok(run(&format!("{mock}\n{isolated}"), "fix_systemd_networkd"));
            outputs.push([
                fs::read(dir.join("10-ens3.network")).unwrap(),
                fs::read(dir.join("10-enp5s0.network")).unwrap(),
            ]);
            assert!(!dir.join("10-cloud-init-eth0.network").exists());
        }
        assert_eq!(outputs[0], outputs[1]);
    }
    #[test]
    fn frpc_service_is_only_the_preserved_modern_variant() {
        let old = upstream("frpc.service");
        let begin = old.find("[X-Service-Old]").unwrap();
        let modern = old.find("[X-Service-New]").unwrap();
        let expected = format!(
            "{}{}",
            &old[..begin],
            old[modern..].replace("[X-Service-New]", "[Service]")
        );
        assert_eq!(source("tests/fixtures/frpc.service"), expected);
        let file = "trans.sh";
        assert!(source(file).contains("write_frpc_systemd_service() {"));
        assert!(
            !source(file).contains("X-Service-"),
            "legacy template selector remains in {file}"
        );
    }
    #[test]
    fn auxiliary_examples_editor_and_log_backend_are_trimmed() {
        assert!(!source("frpc-example.toml").contains("rdp"));
        assert!(!source("frpc-example.toml").contains("3389"));
        for retired in ["windows", "bat,cmd,ps1", "utf-16-le", "utf-8-bom"] {
            assert!(!source(".editorconfig").contains(retired));
        }
        assert!(!root().join("logviewer-nginx.conf").exists());
        let trans = source("trans.sh");
        assert!(!trans.contains("setup_nginx"));
        assert!(!trans.contains("setup_lighttpd"));
        assert!(trans.contains("setup_websocketd"));
        for doc in ["README.md", "README.en.md"] {
            assert!(
                !source(doc).contains("logviewer-nginx.conf"),
                "obsolete deployment asset in {doc}"
            );
        }
    }
    #[test]
    fn logviewer_only_changes_cdn_not_application_code() {
        let expected = upstream("logviewer.html").replace(
            "https://mirrors.sustech.edu.cn/cdnjs/",
            "https://cdnjs.cloudflare.com/",
        );
        assert!(
            source("logviewer.html") == expected,
            "logviewer changed beyond the CDN URL"
        );
    }
    #[test]
    fn scripts_parse_without_execution() {
        for file in [
            "reinstall.sh",
            "trans.sh",
            "initrd-network.sh",
            "fix-eth-name.sh",
        ] {
            let out = Command::new(bash())
                .args(["-n"])
                .arg(root().join(file))
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{file}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    #[test]
    fn retired_symbols_and_assets_are_absent() {
        for file in ["reinstall.sh", "trans.sh", "initrd-network.sh"] {
            let text = source(file);
            for symbol in [
                "is_in_china",
                "is_in_windows",
                "is_windows",
                "force_cn",
                "confhome_cn",
                "windows-driver-utils",
                "install_nixos",
                "install_fnos",
                "install_aosc",
                "setos_ubuntu",
                "setos_kali",
                "setos_windows",
                "wmic",
                "cygpath",
                "rdp_port",
                "no_auto_drivers",
                "custom_infs",
            ] {
                assert!(!text.contains(symbol), "{file} still contains {symbol}");
            }
        }
        for file in [
            "reinstall.bat",
            "windows.xml",
            "windows-setup.bat",
            "wmic.ps1",
            "download-via-browser.js",
            "deprecated/ubuntu.yaml",
            ".github/workflows/run_reinstall.yml",
            ".github/workflows/sync_to_cnb.yml",
        ] {
            assert!(!root().join(file).exists(), "retired asset {file} remains");
        }
    }
    #[test]
    fn four_targets_and_live_interfaces() {
        let code = defs("reinstall.sh", &["verify_os_name", "to_lower"]);
        let prelude = "error(){ :; }; usage_and_exit(){ exit 64; }; resolve_latest_stable(){ case $1 in alpine) latest_releasever=3.99;; debian) latest_releasever=99;; esac; };";
        for (input, expected) in [
            ("alpine", "alpine:3.99"),
            ("debian", "debian:99"),
            ("arch", "arch:"),
            ("gentoo", "gentoo:"),
            ("netboot.xyz", "netboot.xyz:"),
            ("iso", "iso:"),
            ("reset", "reset:"),
        ] {
            let actual = ok(run(
                &format!("{prelude}\n{code}"),
                &format!("verify_os_name '{input}'; printf '%s:%s' \"$distro\" \"$releasever\""),
            ));
            assert_eq!(actual, expected);
        }
        for input in [
            "windows",
            "ubuntu",
            "nixos",
            "kali",
            "centos",
            "dd",
            "alpine 3.21",
            "debian 12",
            "arch 2026",
            "iso invalid",
            "gentoo systemd",
        ] {
            let out = run(
                &format!("{prelude}\n{code}"),
                &format!("verify_os_name '{input}'"),
            );
            assert!(!out.status.success(), "unexpectedly accepted {input}");
        }
    }
    #[test]
    fn current_stable_versions_are_resolved_from_official_metadata() {
        let code = defs("reinstall.sh", &["resolve_latest_stable"]);
        let mocks = "error_and_exit(){ printf '%s' \"$*\" >&2; exit 65; }; basearch=x86_64; curl(){ case $* in *alpinelinux*) printf 'version: 3.99.2\nversion: 3.99.2\n';; *debian*) printf 'Origin: Debian\nVersion: 99.1\nCodename: example\n';; *) exit 70;; esac; };";
        assert_eq!(
            ok(run(
                &format!("{mocks}\n{code}"),
                "resolve_latest_stable alpine; printf '%s' \"$latest_releasever\""
            )),
            "3.99"
        );
        assert_eq!(
            ok(run(
                &format!("{mocks}\n{code}"),
                "resolve_latest_stable debian; printf '%s:%s' \"$latest_releasever\" \"$latest_codename\""
            )),
            "99:example"
        );
        for body in [
            "curl(){ :; }; resolve_latest_stable alpine",
            "curl(){ printf 'Version: bad\nCodename: bad\n'; }; resolve_latest_stable debian",
            "curl(){ return 1; }; resolve_latest_stable alpine",
        ] {
            assert!(
                !run(&format!("{mocks}\n{code}"), body).status.success(),
                "malformed metadata accepted"
            );
        }
    }
    #[test]
    fn debian_uses_alpine_debootstrap_and_networkd() {
        let text = source("trans.sh");
        assert!(text.contains("debootstrap --arch=amd64 --variant=minbase"));
        assert!(text.contains("--include=ca-certificates,debconf-utils"));
        assert!(text.contains("systemd-networkd.service"));
        assert!(text.contains("systemd-resolved.service"));
        assert!(text.contains("apt-get install -y --no-install-recommends"));
        assert!(text.contains("write_debian_fstab"));
        assert!(!text.contains("preseed"));
        assert!(!text.contains("debian-installer"));
    }
    #[test]
    fn debootstrap_invocation_is_amd64_stable_minbase_and_mockable() {
        let code = defs("trans.sh", &["bootstrap_debian_rootfs"]);
        let mocks = "apk(){ printf 'apk %s\\n' \"$*\"; }; debootstrap(){ printf 'debootstrap %s\\n' \"$*\"; };";
        assert_eq!(
            ok(run(
                &format!("{mocks}\n{code}"),
                "codename=trixie; mirror=https://deb.debian.org/debian; bootstrap_debian_rootfs /os",
            )),
            "apk add debootstrap\ndebootstrap --arch=amd64 --variant=minbase --include=ca-certificates,debconf-utils trixie /os https://deb.debian.org/debian\n"
        );
    }
    #[test]
    fn debian_apt_sources_are_codename_pinned_and_include_security_components() {
        let code = defs("trans.sh", &["write_debian_apt_sources"]);
        let temp = Temp::new("debian-sources");
        let os_dir = temp.0.join("os");
        ok(run(
            &code,
            &format!(
                "codename=trixie; mirror=https://deb.debian.org/debian; write_debian_apt_sources {};",
                quote(os_dir.to_str().unwrap())
            ),
        ));
        assert_eq!(
            fs::read_to_string(os_dir.join("etc/apt/sources.list")).unwrap(),
            "# Debian stable repositories\ndeb https://deb.debian.org/debian trixie main contrib non-free non-free-firmware\ndeb https://deb.debian.org/debian trixie-updates main contrib non-free non-free-firmware\ndeb https://security.debian.org/debian-security trixie-security main contrib non-free non-free-firmware\n"
        );
        let invalid = Temp::new("debian-sources-invalid");
        let invalid_dir = invalid.0.join("os");
        assert!(
            !run(
                &code,
                &format!(
                    "codename=trixie; mirror='file:///tmp/evil'; write_debian_apt_sources {}",
                    quote(invalid_dir.to_str().unwrap())
                )
            )
            .status
            .success()
        );
        assert!(!invalid_dir.join("etc/apt/sources.list").exists());
    }
    #[test]
    fn debian_fstab_uses_real_root_and_efi_uuids() {
        let code = defs("trans.sh", &["write_debian_fstab"]);
        let temp = Temp::new("debian-fstab");
        let os_dir = temp.0.join("os");
        let setup = r#"
            is_efi(){ return 0; }
            is_xda_gt_2t(){ return 0; }
            xda(){ case "$1" in 1) echo nvme0n1p1;; 2) echo nvme0n1p2;; esac; }
            blkid(){ case "$5" in /dev/nvme0n1p2) echo 1111-AAAA;; /dev/nvme0n1p1) echo 2222-BBBB;; *) return 1;; esac; }
            error_and_exit(){ printf '%s\n' "$*" >&2; exit 65; }
        "#;
        ok(run(
            &format!("{setup}\n{code}"),
            &format!(
                "os_dir={}; write_debian_fstab \"$os_dir\"",
                quote(os_dir.to_str().unwrap())
            ),
        ));
        assert_eq!(
            fs::read_to_string(os_dir.join("etc/fstab")).unwrap(),
            "# <file system> <mount point> <type> <options> <dump> <pass>\nUUID=1111-AAAA / ext4 defaults,errors=remount-ro 0 1\nUUID=2222-BBBB /boot/efi vfat umask=0077 0 1\n"
        );
    }
    #[test]
    fn debian_kernel_flag_selects_cloud_or_generic_amd64() {
        let code = defs("trans.sh", &["debian_kernel_package"]);
        assert_eq!(
            ok(run(&code, "no_cloud_kernel=0; debian_kernel_package")),
            "linux-image-cloud-amd64\n"
        );
        assert_eq!(
            ok(run(&code, "no_cloud_kernel=1; debian_kernel_package")),
            "linux-image-amd64\n"
        );
    }
    #[test]
    fn debian_grub_package_preseed_avoids_implicit_install_targets() {
        let code = defs("trans.sh", &["prepare_debian_grub_config"]);
        for (mode, expected) in [
            (
                "efi",
                "grub-efi-amd64 grub-efi/install_devices_empty boolean true",
            ),
            ("efi", "grub-efi-amd64 grub-efi/install_devices multiselect"),
            (
                "bios",
                "grub-pc grub-pc/install_devices multiselect /dev/vda",
            ),
        ] {
            let temp = Temp::new("debian-grub-preseed");
            let trace = temp.0.join("trace");
            let mocks = format!(
                "trace={}; chroot(){{ printf '%s\\n' \"$*\" >>\"$trace\"; cat >>\"$trace\"; }}; is_efi(){{ [ \"$mode\" = efi ]; }}; {code}",
                quote(trace.to_str().unwrap())
            );
            ok(run(
                &mocks,
                &format!("mode={mode}; xda=vda; prepare_debian_grub_config /os"),
            ));
            assert!(fs::read_to_string(trace).unwrap().contains(expected));
        }
    }
    #[test]
    fn debian_grub_install_is_explicit_for_bios_and_uefi_without_nvram_side_effects() {
        let code = defs("trans.sh", &["install_debian_grub"]);
        for (mode, expected) in [
            (
                "efi",
                "grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=debian --no-nvram --removable",
            ),
            ("bios", "grub-install --target=i386-pc /dev/vda"),
        ] {
            let temp = Temp::new("debian-grub");
            let os_dir = temp.0.join("os");
            let trace = temp.0.join("trace");
            let mocks = format!(
                "trace={}; chroot(){{ printf '%s\\n' \"$*\" >>\"$trace\"; }}; is_efi(){{ [ \"$mode\" = efi ]; }}; get_ttys(){{ echo 'console=ttyS0,115200n8 console=tty0'; }}; {code}",
                quote(trace.to_str().unwrap())
            );
            ok(run(
                &mocks,
                &format!(
                    "mode={mode}; xda=vda; install_debian_grub {}",
                    quote(os_dir.to_str().unwrap())
                ),
            ));
            let calls = fs::read_to_string(trace).unwrap();
            assert!(calls.lines().any(|line| line.contains(expected)), "{calls}");
            assert!(calls.contains("grub-mkconfig -o /boot/grub/grub.cfg"));
            assert!(
                fs::read_to_string(os_dir.join("etc/default/grub"))
                    .unwrap()
                    .contains("console=ttyS0,115200n8 console=tty0")
            );
        }
    }
    #[test]
    fn debian_install_orchestration_is_mocked_and_complete() {
        let code = defs(
            "trans.sh",
            &[
                "install_debian",
                "debian_kernel_package",
                "write_debian_apt_sources",
                "write_debian_fstab",
                "prepare_debian_grub_config",
                "install_debian_grub",
            ],
        );
        for mode in ["bios", "efi"] {
            let temp = Temp::new(&format!("debian-install-{mode}"));
            let os_dir = temp.0.join("os");
            let trace = temp.0.join("trace");
            let mut setup = format!("mode={mode}; trace={};", quote(trace.to_str().unwrap()));
            setup.push_str(
                r#"
                info(){ :; }
                error_and_exit(){ echo "$*" >&2; exit 65; }
                mount_part_basic_layout(){ echo "mount-layout $*" >>"$trace"; mkdir -p "$1/boot/efi"; }
                bootstrap_debian_rootfs(){ echo "debootstrap $codename $1 $mirror" >>"$trace"; }
                cp_resolv_conf(){ mkdir -p "$1/etc"; : >"$1/etc/resolv.conf"; }
                mount_pseudo_fs(){ echo "mount-pseudo $1" >>"$trace"; }
                apk(){ echo "apk $*" >>"$trace"; }
                chroot(){ echo "chroot $*" >>"$trace"; cat >>"$trace"; }
                get_ucode_firmware_pkgs(){ echo 'firmware-linux amd64-microcode'; }
                setup_systemd_networkd(){ echo "networkd $1" >>"$trace"; }
                rm_resolv_conf(){ rm -f "$1/etc/resolv.conf"; }
                basic_init(){ echo "basic-init $1" >>"$trace"; }
                get_ttys(){ echo 'console=ttyS0,115200n8'; }
                is_efi(){ [ "$mode" = efi ]; }
                is_xda_gt_2t(){ return 1; }
                xda(){ case "$1" in 1) echo nvme0n1p1;; 2) echo nvme0n1p2;; esac; }
                blkid(){
                    case "$5" in
                        /dev/nvme0n1p2) echo 1111-AAAA;;
                        /dev/nvme0n1p1) if [ "$mode" = efi ]; then echo 2222-BBBB; else echo 1111-AAAA; fi;;
                        *) return 1;;
                    esac
                }
                "#,
            );
            setup.push_str(&code);
            let out = run(
                &setup,
                &format!(
                    "distro=debian; codename=trixie; mirror=https://deb.debian.org/debian; no_cloud_kernel=0; username=root; xda=vda; install_debian {}",
                    quote(os_dir.to_str().unwrap())
                ),
            );
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let calls = fs::read_to_string(&trace).unwrap();
            assert!(calls.contains("debootstrap trixie"));
            assert!(calls.contains("apt-get update"));
            assert!(calls.contains("apt-get install -y --no-install-recommends"));
            assert!(calls.contains("linux-image-cloud-amd64"));
            assert!(calls.contains("systemd-sysv systemd-resolved openssh-server"));
            assert!(calls.contains(if mode == "efi" {
                "grub-efi-amd64 efibootmgr dosfstools"
            } else {
                "grub-pc"
            }));
            assert!(calls.contains("networkd "));
            assert!(calls.contains("basic-init "));
            assert!(calls.contains("grub-mkconfig -o /boot/grub/grub.cfg"));
            assert!(!calls.contains("swapon"));
            assert!(!calls.contains("swapoff"));
            let expected_fstab = if mode == "efi" {
                "# <file system> <mount point> <type> <options> <dump> <pass>\nUUID=1111-AAAA / ext4 defaults,errors=remount-ro 0 1\nUUID=2222-BBBB /boot/efi vfat umask=0077 0 1\n"
            } else {
                "# <file system> <mount point> <type> <options> <dump> <pass>\nUUID=1111-AAAA / ext4 defaults,errors=remount-ro 0 1\n"
            };
            assert_eq!(
                fs::read_to_string(os_dir.join("etc/fstab")).unwrap(),
                expected_fstab
            );
            assert_eq!(
                fs::read_link(os_dir.join("etc/resolv.conf")).unwrap(),
                PathBuf::from("/run/systemd/resolve/stub-resolv.conf")
            );
        }
    }
    #[test]
    fn systemd_networkd_setup_uses_mocked_generator_and_enables_services() {
        let helpers = defs(
            "trans.sh",
            &[
                "setup_systemd_networkd",
                "mkdir_clear",
                "del_head_empty_lines_inplace",
            ],
        );
        let temp = Temp::new("networkd-setup");
        let os_dir = temp.0.join("os");
        let trace = temp.0.join("trace");
        let mut setup = format!("trace={};", quote(trace.to_str().unwrap()));
        setup.push_str(r#"
            apk(){ printf 'apk %s\n' "$*" >>"$trace"; }
            chroot(){ printf 'chroot %s\n' "$*" >>"$trace"; }
            useradd(){ return 1; }
            create_cloud_init_network_config(){ printf 'network' >"$1"; }
            cloud-init(){
                local dest=
                while [ "$#" -gt 0 ]; do
                    if [ "$1" = -d ]; then dest=$2; shift 2; else shift; fi
                done
                mkdir -p "$dest/etc/systemd/network"
                printf '# Generated by cloud-init\n[Match]\nName=ens3\n[Network]\nDHCP=yes\n' >"$dest/etc/systemd/network/10-cloud-init-eth0.network"
            }
        "#);
        setup.push_str(&helpers);
        ok(run(
            &setup,
            &format!(
                "setup_systemd_networkd {} {}",
                quote(os_dir.to_str().unwrap()),
                quote(temp.0.join("networkd-config").to_str().unwrap())
            ),
        ));
        let calls = fs::read_to_string(trace).unwrap();
        assert!(calls.contains("systemctl enable systemd-networkd.service"));
        assert!(calls.contains("systemctl enable systemd-resolved.service"));
        assert!(calls.contains("apk add cloud-init"));
        assert!(calls.contains("apk del cloud-init"));
        assert_eq!(
            fs::read_to_string(os_dir.join("etc/systemd/network/10-cloud-init-eth0.network"))
                .unwrap(),
            "[Match]\n[Network]\nDHCP=yes\n"
        );
        assert!(!temp.0.join("networkd-config").exists());
    }
    #[test]
    fn china_is_not_part_of_network_argument_protocol() {
        let text = source("initrd-network.sh");
        assert!(text.contains("ipv6_extra_addrs=$6"));
        assert!(text.contains("ipv4_dns1='1.1.1.1'"));
        let caller = definition(&source("reinstall.sh"), "get_ip_conf_cmd");
        assert!(!caller.contains("is_in_china"));
        assert!(caller.contains("'$ipv6_extra_addrs'"));
    }
    #[test]
    fn fork_assets_fail_closed() {
        let text = source("reinstall.sh");
        assert!(
            !text.contains("confhome=https://raw.githubusercontent.com/bin456789/reinstall/main")
        );
        assert!(text.contains("REINSTALL_CONFHOME"));
        let code = defs("reinstall.sh", &["init_confhome"]);
        let mock = "error_and_exit(){ exit 65; };";
        for value in [
            "",
            "http://example.com/reinstall",
            "https://raw.githubusercontent.com/bin456789/reinstall/main",
            "https://example.com/asset'bad",
        ] {
            let out = run(
                &format!("{mock}\n{code}"),
                &format!("confhome={value:?}; init_confhome"),
            );
            assert!(!out.status.success(), "bad asset base accepted: {value}");
        }
        assert_eq!(
            ok(run(
                &format!("{mock}\n{code}"),
                "confhome=https://example.com/private-reinstall/; init_confhome; printf '%s' \"$confhome\""
            )),
            "https://example.com/private-reinstall"
        );
        assert!(source("trans.sh").contains("PERSONAL-LINUX-ONLY"));
        assert!(text.contains("PERSONAL-LINUX-ONLY"));
    }
    #[test]
    fn unsupported_trans_target_is_rejected_before_installation() {
        let text = source("trans.sh");
        let validation = text
            .find("validate_target\n")
            .expect("missing validation call");
        let start = text.find("mount / -o remount").unwrap();
        assert!(validation < start);
        let code = defs("trans.sh", &["validate_target"]);
        for target in ["alpine", "debian", "arch", "gentoo"] {
            assert!(
                run(
                    &format!("error_and_exit(){{ exit 65; }}; {code}"),
                    &format!("distro={target:?}; validate_target")
                )
                .status
                .success(),
                "valid target rejected: {target}"
            );
        }
        for target in ["windows", "ubuntu", "dd", "nixos", "any", ""] {
            assert!(
                !run(
                    &format!("error_and_exit(){{ exit 65; }}; {code}"),
                    &format!("distro={target:?}; validate_target")
                )
                .status
                .success()
            );
        }
    }
    #[test]
    fn architecture_gate_accepts_only_exact_x86_64_before_boot_mutation() {
        let code = defs("reinstall.sh", &["init_basearch", "to_lower"]);
        for arch in [
            "aarch64", "arm64", "i686", "i386", "amd64", "x64", "unknown",
        ] {
            let out = run(
                &format!("{code}; error_and_exit(){{ exit 65; }}; uname(){{ echo $mock_arch; }}"),
                &format!("mock_arch={arch:?}; init_basearch"),
            );
            assert!(!out.status.success(), "unsupported arch accepted: {arch}");
        }
        let out = run(
            &format!("{code}; error_and_exit(){{ exit 65; }}; uname(){{ echo $mock_arch; }}"),
            "mock_arch=x86_64; init_basearch; printf '%s:%s' \"$basearch\" \"$basearch_alt\"",
        );
        assert_eq!(ok(out), "x86_64:amd64");
        let text = source("reinstall.sh");
        let main = &text[text.rfind("# 脚本入口").unwrap()..];
        assert!(main.find("init_basearch").unwrap() < main.find("verify_os_args\n").unwrap());
        assert!(
            main.find("init_basearch").unwrap() < main.find("remove_exist_reinstall\n").unwrap()
        );
    }
    #[test]
    fn automatic_target_routes_use_alpine_but_manual_entries_stay_separate() {
        let code = defs("reinstall.sh", &["set_boot_targets"]);
        let trace = Temp::new("boot-route");
        let env = format!(
            "setos(){{ printf '%s:%s:%s\\n' \"$1\" \"$2\" \"$3\" >>{}; }};",
            quote(trace.0.join("calls").to_str().unwrap())
        );
        for (target, release, expected) in [
            ("alpine", "3.24", "nextos:alpine:3.24\\n"),
            ("debian", "13", "finalos:debian:13\\nnextos:alpine:3.24\\n"),
            ("arch", "", "finalos:arch:\\nnextos:alpine:3.24\\n"),
            ("gentoo", "", "finalos:gentoo:\\nnextos:alpine:3.24\\n"),
            ("netboot.xyz", "", "nextos:netboot.xyz:\\n"),
            ("iso", "", ""),
            ("reset", "", ""),
        ] {
            let path = trace.0.join("calls");
            let _ = fs::remove_file(&path);
            ok(run(
                &format!("{env}\n{code}"),
                &format!("distro={target:?}; releasever={release:?}; set_boot_targets 3.24"),
            ));
            assert_eq!(
                fs::read_to_string(path).unwrap_or_default(),
                expected.replace("\\n", "\n"),
                "route {target}"
            );
        }
        let text = source("reinstall.sh");
        let main = &text[text.rfind("# 脚本入口").unwrap()..];
        assert!(main.find("reset_and_exit\n").unwrap() < main.find("set_boot_targets").unwrap());
        assert!(
            main.find("boot_local_iso_and_exit\n").unwrap()
                < main.find("set_boot_targets").unwrap()
        );
    }
    #[test]
    fn ram_floors_match_no_swap_install_policy() {
        let host = defs("reinstall.sh", &["ram_floor_for_target"]);
        let live = defs("trans.sh", &["minimum_ram_for_target", "check_install_ram"]);
        for (target, expected) in [
            ("alpine", "512"),
            ("debian", "512"),
            ("arch", "1024"),
            ("gentoo", "2048"),
        ] {
            assert_eq!(
                ok(run(&host, &format!("ram_floor_for_target {target};"))).trim(),
                expected
            );
            assert_eq!(
                ok(run(&live, &format!("minimum_ram_for_target {target};"))).trim(),
                expected
            );
        }
        let check = defs("trans.sh", &["minimum_ram_for_target", "check_install_ram"]);
        for (target, floor) in [
            ("alpine", 512),
            ("debian", 512),
            ("arch", 1024),
            ("gentoo", 2048),
        ] {
            for (ram, accepted) in [(floor, true), (floor - 1, false)] {
                let setup = format!(
                    "get_approximate_ram_size(){{ echo {ram}; }}; error_and_exit(){{ exit 65; }}; {check}"
                );
                assert_eq!(
                    run(&setup, &format!("distro={target}; check_install_ram"))
                        .status
                        .success(),
                    accepted,
                    "RAM={ram} for {target}"
                );
            }
        }
        let trans = defs("trans.sh", &["trans"]);
        assert!(trans.find("check_install_ram").unwrap() < trans.find("clear_previous").unwrap());
        assert!(trans.find("check_install_ram").unwrap() < trans.find("create_part").unwrap());
        let host_main = source("reinstall.sh");
        let host_main = &host_main[host_main.rfind("# 脚本入口").unwrap()..];
        assert!(
            host_main.find("check_ram\n").unwrap()
                < host_main.find("remove_exist_reinstall\n").unwrap()
        );
    }
    #[test]
    fn installer_does_not_mutate_swap_state() {
        let commands = ["create_swap", "swapon", "swapoff", "mkswap", "/os/swapfile"];
        for file in ["reinstall.sh", "trans.sh"] {
            for line in source(file)
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
            {
                for command in commands {
                    assert!(
                        !line.contains(command),
                        "swap mutation remains in {file}: {line}"
                    );
                }
            }
        }
    }
    #[test]
    fn real_metadata_fixtures_include_yaml_indentation() {
        let code = defs("reinstall.sh", &["resolve_latest_stable"]);
        for (target, file, expected) in [
            (
                "alpine",
                "tests/fixtures/alpine-latest-releases.yaml",
                "3.24",
            ),
            ("debian", "tests/fixtures/debian-stable-Release", "13"),
        ] {
            let metadata = source(file);
            let actual = ok(run(
                &format!(
                    "error_and_exit(){{ exit 65; }}; basearch=x86_64; curl(){{ printf '%s' {}; }}; {code}",
                    quote(&metadata)
                ),
                &format!("resolve_latest_stable {target}; printf '%s' \"$latest_releasever\""),
            ));
            assert_eq!(actual, expected);
        }
    }
    #[test]
    fn pure_helpers_match_upstream_byte_for_byte() {
        let original = upstream("reinstall.sh");
        for (name, body) in [
            (
                "mask2cidr",
                "for mask in 255.255.255.255 255.255.255.0 255.255.0.0 255.0.0.0 128.0.0.0 0.0.0.0; do mask2cidr \"$mask\"; done",
            ),
            ("trim", "printf '  example  \\n\\texample\\t\\n' | trim"),
            (
                "is_port_valid",
                "for port in 0 1 22 65535 65536 bad; do if is_port_valid \"$port\"; then printf yes; else printf no; fi; done",
            ),
        ] {
            let mut old_defs = definition(&original, name);
            let mut new_defs = defs("reinstall.sh", &[name]);
            if name == "is_port_valid" {
                old_defs.push_str(&format!("\n{}", definition(&original, "is_digit")));
                new_defs.push_str(&format!("\n{}", defs("reinstall.sh", &["is_digit"])));
            }
            assert_eq!(
                ok(run(&old_defs, body)).as_bytes(),
                ok(run(&new_defs, body)).as_bytes(),
                "{name}"
            );
        }
    }
    #[test]
    fn network_argument_output_matches_upstream_except_removed_geography_field() {
        let original = upstream("reinstall.sh");
        let names = [
            "get_ip_conf_cmd",
            "is_found_ipv4_netconf",
            "is_found_ipv6_netconf",
        ];
        let old_defs = names
            .iter()
            .map(|n| definition(&original, n))
            .collect::<Vec<_>>()
            .join("\n");
        let new_defs = defs("reinstall.sh", &names);
        for body in [
            "ipv4_mac=aa; ipv4_addr=192.0.2.1/32; ipv4_gateway=192.0.2.254; ipv6_mac=aa; ipv6_addr=2001:db8::1/128; ipv6_gateway=fe80::1; ipv6_extra_addrs=2001:db8::2/128",
            "ipv4_mac=aa; ipv4_addr=192.0.2.1/32; ipv4_gateway=192.0.2.254; ipv6_mac=bb; ipv6_addr=2001:db8::1/128; ipv6_gateway=fe80::1; ipv6_extra_addrs=2001:db8::2/128",
            "ipv4_mac=aa; ipv4_addr=192.0.2.1/32; ipv4_gateway=192.0.2.254",
            "ipv6_mac=aa; ipv6_addr=2001:db8::1/128; ipv6_gateway=fe80::1; ipv6_extra_addrs=2001:db8::2/128",
        ] {
            let body = format!(
                "collect_netconf(){{ :; }}; is_in_china(){{ return 1; }}; {body}; get_ip_conf_cmd"
            );
            let expected = ok(run(&old_defs, &body)).replace(" 'false'", "");
            assert_eq!(ok(run(&new_defs, &body)).as_bytes(), expected.as_bytes());
        }
    }
    #[test]
    fn ifupdown_output_matches_upstream_for_preserved_network_modes() {
        let temp = Temp::new("net-parity");
        let old = definition(&upstream("trans.sh"), "create_ifupdown_config");
        let new = defs("trans.sh", &["create_ifupdown_config"]);
        let mocks = "distro=alpine; get_eths(){ echo eth0; }; get_netconf_to(){ case $1 in mac_addr) mac_addr=00:11:22:33:44:55;; ipv4_addr) ipv4_addr=192.0.2.1/32;; ipv4_gateway) ipv4_gateway=192.0.2.254;; ipv6_addr) ipv6_addr=2001:db8::1/128;; ipv6_gateway) ipv6_gateway=2001:db8::ff;; ipv6_extra_addrs) ipv6_extra_addrs=2001:db8::2/128,2001:db8::3/128;; esac; }; get_current_dns(){ return 1; }; is_need_manual_set_dnsv6(){ return 1; }; should_disable_accept_ra(){ return 1; }; should_disable_autoconf(){ return 1; };";
        for (i, mode) in [
            "is_dhcpv4(){ return 0; }; is_staticv4(){ return 1; }; is_slaac(){ return 0; }; is_dhcpv6(){ return 1; }; is_staticv6(){ return 1; };",
            "is_dhcpv4(){ return 1; }; is_staticv4(){ return 0; }; is_slaac(){ return 1; }; is_dhcpv6(){ return 1; }; is_staticv6(){ return 0; };",
            "is_dhcpv4(){ return 0; }; is_staticv4(){ return 1; }; is_slaac(){ return 1; }; is_dhcpv6(){ return 0; }; is_staticv6(){ return 1; };",
        ].iter().enumerate() {
            let a = temp.0.join(format!("old-{i}")); let b = temp.0.join(format!("new-{i}"));
            ok(run(&format!("{mocks}\n{old}"), &format!("{mode} create_ifupdown_config {}", quote(a.to_str().unwrap()))));
            ok(run(&format!("{mocks}\n{new}"), &format!("{mode} create_ifupdown_config {}", quote(b.to_str().unwrap()))));
            assert_eq!(fs::read(a).unwrap(), fs::read(b).unwrap(), "network mode {i}");
        }
    }
    #[test]
    fn removed_upstream_functions_have_no_surviving_references() {
        for file in ["reinstall.sh", "trans.sh"] {
            let old = upstream(file);
            let new = source(file);
            for line in old.lines() {
                let Some(name) = line.strip_suffix("() {") else {
                    continue;
                };
                if name.trim() != name || name.is_empty() || new.contains(&format!("\n{name}() {{"))
                {
                    continue;
                }
                for (n, line) in new.lines().enumerate() {
                    if line.trim_start().starts_with('#') {
                        continue;
                    }
                    assert!(
                        !line
                            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                            .any(|word| word == name),
                        "{file}:{} references removed {name}: {line}",
                        n + 1
                    );
                }
            }
        }
    }
    #[test]
    fn local_iso_paths_and_arguments_fail_closed() {
        let temp = Temp::new("iso-args");
        let iso = temp.0.join("live.iso");
        fs::write(&iso, b"argument-only fixture").unwrap();
        let code = defs(
            "reinstall.sh",
            &[
                "assert_safe_iso_path",
                "protect_iso_source",
                "validate_iso_args",
            ],
        );
        let setup = format!(
            "error_and_exit(){{ exit 65; }}; tmp={}; iso={}; {code}",
            quote(temp.0.join("managed").to_str().unwrap()),
            quote(iso.to_str().unwrap())
        );
        assert_eq!(
            ok(run(
                &setup,
                "validate_iso_args; printf '%s' \"$iso_config\""
            )),
            "/boot/grub/loopback.cfg"
        );
        for args in [
            "iso=https://example.com/live.iso",
            "iso=/nonexistent/live.iso",
            "iso_kernel=/boot/vmlinuz",
            "iso_kernel=/boot/vmlinuz; iso_initrd=/boot/initrd; iso_config=/boot/grub/loopback.cfg",
            "iso_cmdline=boot=live",
            "iso_kernel=/boot/vmlinuz; iso_initrd=/boot/initrd; iso_cmdline='boot=live; reboot'",
            "iso_kernel='/boot/bad name'; iso_initrd=/initrd",
            "iso_config=/boot/../evil",
            "iso_kernel=/vmlinuz; iso_initrd=/initrd; iso_cmdline='$(reboot)'",
        ] {
            assert!(
                !run(&setup, &format!("{args}; validate_iso_args"))
                    .status
                    .success(),
                "accepted {args}"
            );
        }
        assert!(run(&setup, "iso_kernel=/boot/vmlinuz; iso_initrd=/boot/initrd; iso_cmdline='boot=live findiso=%ISO_PATH%'; validate_iso_args").status.success());
        let protected = run(
            &setup,
            &format!(
                "tmp={}; protect_iso_source",
                quote(temp.0.to_str().unwrap())
            ),
        );
        assert!(!protected.status.success());
        assert!(iso.exists());
        let main = source("reinstall.sh");
        let entry = &main[main.find("# 脚本入口").unwrap()..];
        assert!(
            entry.find("    protect_iso_source\n").unwrap()
                < entry.find("mkdir_clear \"$tmp\"").unwrap()
        );
    }
    #[test]
    fn iso_grub_output_matches_known_fixture_byte_for_byte() {
        let code = defs("reinstall.sh", &["render_local_iso_grub", "get_entry_name"]);
        let setup = format!(
            "BOOT_ENTEY_START_MARK='### BEGIN reinstall.sh ###'; BOOT_ENTEY_END_MARK='### END reinstall.sh ###'; distro=iso; iso_uuid=1111-2222; iso_path=/images/live.iso; get_function_content(){{ echo '# grubenv helper fixture'; }}; {code}"
        );
        let actual = ok(run(
            &setup,
            "iso_config=/boot/grub/loopback.cfg; render_local_iso_grub",
        ));
        assert_eq!(
            actual.as_bytes(),
            source("tests/fixtures/local-iso-config.cfg").as_bytes()
        );
        let actual = ok(run(
            &setup,
            "iso_kernel=/boot/vmlinuz; iso_initrd=/boot/initrd; iso_cmdline='boot=live findiso=/images/live.iso'; render_local_iso_grub",
        ));
        let expected = source("tests/fixtures/local-iso-config.cfg").replace("    configfile (loop)/boot/grub/loopback.cfg\n", "    linux (loop)/boot/vmlinuz boot=live findiso=/images/live.iso\n    initrd (loop)/boot/initrd\n");
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }
    #[test]
    fn failed_iso_preflight_never_changes_boot_entries() {
        let code = defs("reinstall.sh", &["boot_local_iso_and_exit"]);
        let out = run(
            &format!(
                "is_use_local_extlinux(){{ return 1; }}; prepare_local_iso(){{ exit 65; }}; remove_exist_reinstall(){{ echo BUG; }}; install_grub_linux_efi(){{ echo BUG; }}; {code}"
            ),
            "boot_local_iso_and_exit",
        );
        assert_eq!(out.status.code(), Some(65));
        assert!(out.stdout.is_empty());
    }
    #[test]
    fn iso_staging_is_one_shot_and_preserves_original_image() {
        let code = defs("reinstall.sh", &["boot_local_iso_and_exit"]);
        for mode in ["bios", "efi"] {
            let temp = Temp::new(mode);
            let tmp = temp.0.join("work");
            fs::create_dir(&tmp).unwrap();
            let iso = temp.0.join("live.iso");
            fs::write(&iso, b"immutable ISO fixture").unwrap();
            let target = temp.0.join("grub/custom.cfg");
            let trace = temp.0.join("trace");
            let setup = format!(
                "tmp={}; target_cfg={}; iso={}; trace={}; grub=mock-grub; reinstall_____=reinstall; is_use_local_extlinux(){{ return 1; }}; prepare_local_iso(){{ echo prepare >>\"$trace\"; }}; render_local_iso_grub(){{ echo render >>\"$trace\"; echo 'fixture config'; }}; remove_exist_reinstall(){{ echo remove >>\"$trace\"; }}; install_grub_linux_efi(){{ echo efi-bootnext >>\"$trace\"; }}; mock-grub-reboot(){{ echo bios-bootnext >>\"$trace\"; }}; info(){{ :; }}; get_entry_name(){{ echo 'reinstall (iso)'; }}; is_efi(){{ {}; }}; {code}",
                quote(tmp.to_str().unwrap()),
                quote(target.to_str().unwrap()),
                quote(iso.to_str().unwrap()),
                quote(trace.to_str().unwrap()),
                if mode == "efi" {
                    "return 0"
                } else {
                    "return 1"
                }
            );
            ok(run(&setup, "boot_local_iso_and_exit"));
            assert_eq!(fs::read(&iso).unwrap(), b"immutable ISO fixture");
            assert_eq!(fs::read(&target).unwrap(), b"fixture config\n");
            assert_eq!(
                fs::read_to_string(trace).unwrap(),
                format!("prepare\nrender\nremove\n{mode}-bootnext\n")
            );
        }
    }
    #[test]
    fn frpc_keeps_ipv6_fallback_without_geographic_detection() {
        let code = defs("trans.sh", &["is_ipv6_only", "frpc_wget", "get_frpc_url"]);
        let setup = format!(
            "uname(){{ echo x86_64; }}; wget(){{ case $* in *checksums*) printf 'abc  frp_0.99.1_linux_amd64.tar.gz\\n';; *) printf '  Location: https://github.com/fatedier/frp/releases/tag/v0.99.1\\n' >&2;; esac; }}; {code}; frpc_wget(){{ wget \"$@\"; }};"
        );
        assert_eq!(
            ok(run(
                &setup,
                "is_ipv6_only(){ return 1; }; get_frpc_url linux"
            )),
            "https://github.com/fatedier/frp/releases/download/v0.99.1/frp_0.99.1_linux_amd64.tar.gz\n"
        );
        assert_eq!(
            ok(run(
                &setup,
                "is_ipv6_only(){ return 0; }; get_frpc_url linux"
            )),
            "https://mirrors.nju.edu.cn/github-release/fatedier/frp/v0.99.1/frp_0.99.1_linux_amd64.tar.gz\n"
        );
        assert!(!run(&setup, "get_frpc_url unsupported").status.success());
        for arch in ["aarch64", "arm64", "i686"] {
            let arch_setup = format!(
                "uname(){{ echo {arch}; }}; wget(){{ echo BUG; }}; {code}; frpc_wget(){{ wget \"$@\"; }};"
            );
            let out = run(&arch_setup, "get_frpc_url linux");
            assert!(
                !out.status.success() && out.stdout.is_empty(),
                "unsupported architecture was not rejected before network access: {arch}"
            );
        }
    }
    #[test]
    fn actual_busybox_ash_parses_runtime_scripts_without_execution() {
        let binary = std::env::var("TEST_BUSYBOX").unwrap_or_else(|_| "busybox".into());
        for file in ["trans.sh", "initrd-network.sh", "fix-eth-name.sh"] {
            let out = Command::new(&binary)
                .args(["ash", "-n"])
                .arg(root().join(file))
                .output()
                .expect("Install BusyBox or set TEST_BUSYBOX");
            assert!(
                out.status.success(),
                "ash: {file}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    #[test]
    fn preliminary_option_scan_does_not_reinterpret_option_values() {
        let text = source("reinstall.sh");
        let start = text.find("# 第一遍扫描，验证要安装的系统和版本").unwrap();
        let end = text.find("# Protect the already-downloaded ISO").unwrap();
        let code = &text[start..end];
        let setup = "init_basearch(){ :; }; init_confhome(){ :; }; verify_os_name(){ distro=$1; }; to_lower(){ tr '[:upper:]' '[:lower:]'; }; ORIGINAL_OPTS=\"'--password' '--iso' '--username' '--debug' '--no-cloud-kernel' '--hold' '2' '--' 'arch'\";";
        assert_eq!(
            ok(run(
                setup,
                &format!(
                    "{code}\nprintf '%s:%s:%s' \"${{confhome:-unset}}\" \"${{iso:-unset}}\" \"$distro\""
                )
            )),
            "unset:unset:arch"
        );
    }
    #[test]
    fn iso_paths_allow_common_btrfs_subvolume_names() {
        let code = defs("reinstall.sh", &["assert_safe_iso_path"]);
        assert!(
            run(
                &format!("error_and_exit(){{ exit 65; }}; {code}"),
                "assert_safe_iso_path /@/images/live.iso"
            )
            .status
            .success()
        );
    }
    #[test]
    fn failed_storage_probe_does_not_mount_the_iso() {
        let code = defs(
            "reinstall.sh",
            &["prepare_local_iso", "assert_safe_iso_path"],
        );
        let temp = Temp::new("probe-error");
        let setup = format!(
            "tmp={}; iso=/images/live.iso; is_have_cmd(){{ return 0; }}; grub-probe(){{ case $1 in --target=abstraction) return 2;; *) echo abcd-1234;; esac; }}; grub-mkrelpath(){{ echo /images/live.iso; }}; error_and_exit(){{ exit 65; }}; mount(){{ echo BUG; return 2; }}; {code}",
            quote(temp.0.to_str().unwrap())
        );
        let out = run(&setup, "prepare_local_iso");
        assert_eq!(out.status.code(), Some(65));
        assert!(
            out.stdout.is_empty(),
            "mount was attempted after failed storage abstraction probe"
        );
    }
    #[test]
    fn iso_inspection_is_read_only_and_cleans_up_on_invalid_contents() {
        let code = defs(
            "reinstall.sh",
            &["prepare_local_iso", "assert_safe_iso_path"],
        );
        for (label, contents, accepted) in [
            (
                "linux-config",
                "mkdir -p \"$4/boot/grub\" \"$4/sources\"; printf config >\"$4/boot/grub/loopback.cfg\"",
                true,
            ),
            ("missing-config", "mkdir -p \"$4/boot/grub\"", false),
            (
                "nonlinux-media",
                "mkdir -p \"$4/sources\"; printf data >\"$4/sources/boot.wim\"",
                false,
            ),
        ] {
            let temp = Temp::new(label);
            let trace = temp.0.join("trace");
            let setup = format!(
                "tmp={}; trace={}; iso=/images/live.iso; iso_config=/boot/grub/loopback.cfg; is_have_cmd(){{ return 0; }}; grub-probe(){{ case $1 in --target=abstraction) :;; *) echo abcd-1234;; esac; }}; grub-mkrelpath(){{ echo /images/live.iso; }}; error_and_exit(){{ exit 65; }}; mount(){{ [ \"$1\" = -o ] && [ \"$2\" = loop,ro ] || exit 90; echo mount-read-only >>\"$trace\"; {contents}; }}; umount(){{ echo unmount >>\"$trace\"; rm -rf \"$1/boot\" \"$1/sources\"; }}; {code}",
                quote(temp.0.to_str().unwrap()),
                quote(trace.to_str().unwrap())
            );
            let out = run(&setup, "prepare_local_iso");
            assert_eq!(
                out.status.success(),
                accepted,
                "{label}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                fs::read_to_string(trace).unwrap(),
                "mount-read-only\nunmount\n"
            );
            assert!(!temp.0.join("iso-check").exists());
        }
    }
    #[test]
    fn retained_targets_resolve_only_to_x86_64_sources() {
        let code = defs("reinstall.sh", &["setos", "set_var"]);
        let mocks = "ram_size=1024; info(){ :; }; is_virt(){ return 0; }; is_efi(){ return 1; }; test_url(){ :; }; curl(){ printf '20261101T000000Z/stage3-amd64-systemd-20261101T000000Z.tar.xz 12345\\n'; };";
        for (body, expected) in [
            (
                "basearch=x86_64; basearch_alt=amd64; setos nextos alpine 3.99; printf '%s' \"$nextos_vmlinuz\"",
                "http://dl-cdn.alpinelinux.org/alpine/v3.99/releases/x86_64/netboot/vmlinuz-virt",
            ),
            (
                "basearch=x86_64; latest_codename=example; setos finalos debian 13; printf '%s:%s:%s:%s' \"$finalos_distro\" \"$finalos_releasever\" \"$finalos_codename\" \"$finalos_mirror\"",
                "debian:13:example:https://deb.debian.org/debian",
            ),
            (
                "basearch=x86_64; basearch_alt=amd64; setos finalos arch; printf '%s' \"$finalos_mirror\"",
                "https://geo.mirror.pkgbuild.com",
            ),
            (
                "basearch=x86_64; basearch_alt=amd64; setos finalos gentoo; printf '%s' \"$finalos_img\"",
                "https://distfiles.gentoo.org/releases/amd64/autobuilds/20261101T000000Z/stage3-amd64-systemd-20261101T000000Z.tar.xz",
            ),
            (
                "is_efi(){ return 1; }; basearch=x86_64; setos nextos netboot.xyz; printf '%s' \"$nextos_vmlinuz\"",
                "https://boot.netboot.xyz/ipxe/netboot.xyz.lkrn",
            ),
            (
                "is_efi(){ return 0; }; setos nextos netboot.xyz; printf '%s' \"$nextos_efi\"",
                "https://boot.netboot.xyz/ipxe/netboot.xyz.efi",
            ),
        ] {
            assert_eq!(ok(run(&format!("{mocks}\n{code}"), body)), expected);
        }
        assert!(
            !run(
                &format!("{mocks}\n{code}"),
                "error_and_exit(){ exit 65; }; setos finalos unsupported"
            )
            .status
            .success()
        );
    }
    #[test]
    fn runtime_assets_do_not_reference_deleted_templates() {
        for file in ["reinstall.sh", "trans.sh"] {
            let text = source(file);
            for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
                assert!(!line.contains("$confhome/deprecated/"), "{file}: {line}");
                assert!(!line.contains("$confhome/windows"), "{file}: {line}");
            }
        }
        assert!(
            source("reinstall.sh")
                .contains("download_and_extract_apk \"$nextos_releasever\" dhcpcd")
        );
        assert!(source("trans.sh").contains("get_drivers() {"));
    }
    #[test]
    fn help_is_safe_and_successful_without_root_or_network() {
        let out = Command::new(bash())
            .arg(root().join("reinstall.sh"))
            .arg("--help")
            .output()
            .unwrap();
        let output = ok(out);
        assert!(output.contains("iso"));
        assert!(output.contains("netboot.xyz"));
        assert!(!output.contains("windows"));
    }
}
