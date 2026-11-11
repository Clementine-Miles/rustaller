use rustaller::host::boot::{
    BootError, BootExtra, GrubBootEntry, build_cmdline, build_extra_cmdline, build_finalos_cmdline,
    build_nextos_cmdline, entry_name, render_grub_config,
};
use rustaller::model::{BootMode, InstallPlan, Target};
use std::collections::BTreeMap;

#[test]
fn grub_entry_names_and_alpine_live_cmdline_preserve_legacy_fields() {
    assert_eq!(
        entry_name("alpine", "3.22.1", Some(1)),
        "rustaller (alpine 3.22.1 Live OS)"
    );
    assert_eq!(
        entry_name("debian", "stable", None),
        "rustaller (debian stable)"
    );
    assert_eq!(
        build_nextos_cmdline(
            "https://dl-cdn.alpinelinux.org/alpine/v3.22/main",
            "/boot/modloop-lts"
        )
        .unwrap(),
        "alpine_repo=https://dl-cdn.alpinelinux.org/alpine/v3.22/main modloop=/boot/modloop-lts console=ttyS0,115200n8 console=tty0"
    );
}

#[test]
fn alpine_boot_values_reject_missing_hosts_queries_and_path_traversal() {
    assert!(build_nextos_cmdline("https://", "/boot/modloop-lts").is_err());
    assert!(build_nextos_cmdline("https:///v3.22/main", "/boot/modloop-lts").is_err());
    assert!(
        build_nextos_cmdline("https://mirror.example/repo?token=x", "/boot/modloop-lts").is_err()
    );
    assert!(build_nextos_cmdline("https://mirror.example/repo", "/boot/../modloop").is_err());
}

#[test]
fn finalos_and_extra_cmdline_keep_order_quoting_and_mirrorlist_handling() {
    let finalos = BTreeMap::from([
        ("distro".into(), "debian".into()),
        ("mirrorlist".into(), "not-emitted-here".into()),
        ("releasever".into(), "stable".into()),
    ]);
    let mut plan = InstallPlan::new(Target::Debian);
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.username = Some("admin".into());
    plan.ssh_port = Some(2222);
    plan.web_port = Some(8080);
    plan.no_cloud_kernel = true;
    plan.force_boot_mode = Some(BootMode::Efi);
    let extra = BootExtra {
        main_disk: Some("deadbeef".into()),
        web_path: Some("/private path".into()),
        finalos_mirrorlist: Some("https://mirror.example/list?a=1&b=2".into()),
        nextos_mirrorlist: Some("ignored".into()),
        link_grub_dir: true,
    };

    let finalos_cmdline = build_finalos_cmdline(&finalos).unwrap();
    let extra_cmdline = build_extra_cmdline(&plan, &extra).unwrap();
    assert_eq!(
        finalos_cmdline,
        " finalos_distro=debian finalos_releasever=stable"
    );
    assert_eq!(
        extra_cmdline,
        " extra_confhome='https://assets.example/reinstall' extra_force_boot_mode=efi extra_no_cloud_kernel=1 extra_main_disk=deadbeef extra_username=admin extra_ssh_port=2222 extra_web_port=8080 extra_web_path='/private path' extra_mirrorlist='https://mirror.example/list?a=1&b=2' extra_link_grub_dir=1"
    );
    assert_eq!(
        build_cmdline("next", &finalos_cmdline, &extra_cmdline),
        format!("next {finalos_cmdline} {extra_cmdline}")
    );
}

#[test]
fn grub_config_renders_a_validated_bios_entry_and_shared_grubenv_helper() {
    let entry = GrubBootEntry {
        name: "rustaller (debian stable)".into(),
        kernel_path: "/rustaller-vmlinuz".into(),
        initrd_path: Some("/rustaller-initrd".into()),
        linux_command: "linux".into(),
        initrd_command: "initrd".into(),
        cmdline: "alpine_repo=https://mirror.example/alpine modloop=/boot/modloop-lts console=tty0"
            .into(),
        btrfs_root: true,
        include_grubenv_helper: true,
    };
    assert_eq!(
        render_grub_config(&entry).unwrap(),
        concat!(
            "### BEGIN reinstall.sh ###\n",
            "if ! [ -s $prefix/grubenv ]; then\n",
            "    for dir in /boot/grub /boot/grub2 /grub /grub2; do\n",
            "        set grubenv=\"($root)$dir/grubenv\"\n",
            "        if [ -s $grubenv ]; then\n",
            "            load_env --file $grubenv\n",
            "            if [ \"${next_entry}\" ]; then\n",
            "                set default=\"${next_entry}\"\n",
            "                set next_entry=\n",
            "                save_env --file $grubenv next_entry\n",
            "            else\n",
            "                set default=\"0\"\n",
            "            fi\n",
            "            return\n",
            "        fi\n",
            "    done\n",
            "fi\n",
            "set timeout_style=menu\n",
            "set timeout=5\n",
            "menuentry \"rustaller (debian stable)\" --unrestricted {\n",
            "    insmod lvm\n",
            "    set btrfs_relative_path=n\n",
            "    insmod all_video\n",
            "    search --no-floppy --file --set=root /rustaller-vmlinuz\n",
            "    linux /rustaller-vmlinuz alpine_repo=https://mirror.example/alpine modloop=/boot/modloop-lts console=tty0\n",
            "    initrd /rustaller-initrd\n",
            "}\n",
            "### END reinstall.sh ###\n"
        )
    );
}

#[test]
fn grub_config_rejects_untrusted_paths_commands_and_config_injection() {
    let entry = GrubBootEntry {
        name: "bad\" entry".into(),
        kernel_path: "/rustaller-vmlinuz".into(),
        initrd_path: None,
        linux_command: "linux".into(),
        initrd_command: "initrd".into(),
        cmdline: String::new(),
        btrfs_root: false,
        include_grubenv_helper: false,
    };
    assert!(matches!(
        render_grub_config(&entry),
        Err(BootError::UnsafeValue(_))
    ));
    let mut entry = entry;
    entry.name = "valid".into();
    entry.kernel_path = "/tmp/../etc/passwd".into();
    assert!(matches!(
        render_grub_config(&entry),
        Err(BootError::InvalidBootPath)
    ));
    entry.kernel_path = "/rustaller-vmlinuz".into();
    entry.linux_command = "sh".into();
    assert!(matches!(
        render_grub_config(&entry),
        Err(BootError::InvalidBootCommand)
    ));
}

#[test]
fn command_line_rejects_unrepresentable_values_instead_of_emitting_grub_syntax() {
    let mut plan = InstallPlan::new(Target::Debian);
    plan.confhome = Some("https://assets.example/reinstall".into());
    plan.username = Some("bad'user".into());
    let error = build_extra_cmdline(&plan, &BootExtra::default()).unwrap_err();
    assert!(matches!(error, BootError::UnsafeValue(_)));
}
