//! Build freestanding firmware separately from Linux deployment binaries.
use std::{ffi::OsStr, fs, path::Path, process::Command};

fn checked(cmd: &mut Command, what: &str) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    if !status.success() {
        return Err(format!("{what} failed with {status}"));
    }
    Ok(())
}
pub fn build_bios(root: &Path, cargo: &OsStr, target_dir: &Path, out: &Path) -> Result<(), String> {
    let target = "i686-unknown-linux-gnu";
    checked(
        Command::new(cargo)
            .current_dir(root)
            .args([
                "rustc",
                "--locked",
                "--release",
                "--target",
                target,
                "-p",
                "rustaller-boot-bios",
                "--",
                "-C",
                "relocation-model=static",
                "-C",
                "link-arg=-nostdlib",
                "-C",
                "link-arg=-static",
                "-C",
                "link-arg=-no-pie",
                "-C",
            ])
            .arg(format!(
                "link-arg=-Wl,-T,{}",
                root.join("boot/bios/layout.ld").display()
            ))
            .args(["-C", "link-arg=-Wl,--build-id=none"]),
        "BIOS Rust stage",
    )?;
    let obj = out.join("bios-mbr.o");
    checked(
        Command::new("as")
            .arg("--32")
            .arg(root.join("boot/bios/stage1.S"))
            .arg("-o")
            .arg(&obj),
        "BIOS bootstrap assembly",
    )?;
    checked(
        Command::new("ld")
            .args(["-m", "elf_i386", "-Ttext", "0x7c00", "--oformat", "binary"])
            .arg(&obj)
            .arg("-o")
            .arg(out.join("bios-mbr.bin")),
        "BIOS bootstrap link",
    )?;
    let elf = target_dir.join(target).join("release/rustaller-boot-bios");
    let header = fs::read(&elf).map_err(|e| e.to_string())?;
    if header.get(..5) != Some(b"\x7fELF\x01")
        || header.get(18..20) != Some(&[3, 0])
        || header.get(24..28) != Some(&0x10000u32.to_le_bytes())
    {
        return Err("expected i386 BIOS ELF linked at 0x10000".into());
    }
    checked(
        Command::new("objcopy")
            .args(["-O", "binary"])
            .arg(elf)
            .arg(out.join("bios-stage2.bin")),
        "BIOS stage extraction",
    )?;
    let mbr = fs::read(out.join("bios-mbr.bin")).map_err(|e| e.to_string())?;
    let stage = fs::read(out.join("bios-stage2.bin")).map_err(|e| e.to_string())?;
    if mbr.len() != 440
        || mbr.get(400..408) != Some(b"RBBOOT01")
        || !(512..=0x69000).contains(&stage.len())
        || !crate::contains_bytes(&stage, b"RUSTALLER-BOOT-BIOS-1")
    {
        return Err("invalid BIOS artifact size or schema".into());
    }
    Ok(())
}

pub fn verify_efi(bytes: &[u8]) -> Result<(), String> {
    let word = |p| {
        bytes
            .get(p..p + 2)
            .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
    };
    let pe = bytes
        .get(0x3c..0x40)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()) as usize)
        .ok_or("missing PE header")?;
    if !bytes.starts_with(b"MZ")
        || bytes.get(pe..pe + 4) != Some(b"PE\0\0")
        || word(pe + 4) != Some(0x8664)
        || word(pe + 24) != Some(0x20b)
        || word(pe + 24 + 68) != Some(10)
        || !crate::contains_bytes(bytes, b"RUSTALLER-BOOT-UEFI-1")
    {
        return Err("expected Rustaller schema 1 x86-64 EFI application".into());
    }
    Ok(())
}
