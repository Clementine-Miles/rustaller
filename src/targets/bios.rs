//! Install the two BIOS stages only inside the extents created by DiskLayout.
use super::boot::Result;
use crate::shared::storage::{DiskLayout, PartitionRole};
use std::{
    fs,
    os::unix::fs::{FileExt, FileTypeExt, OpenOptionsExt},
    path::Path,
};

fn u32at(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn u64at(b: &[u8], p: usize) -> u64 {
    u64::from_le_bytes(b[p..p + 8].try_into().unwrap())
}

pub(super) fn validate_stages(mbr: &[u8], stage: &[u8]) -> Result<()> {
    if mbr.len() != 440
        || &mbr[400..408] != b"RBBOOT01"
        || mbr[408..].iter().any(|b| *b != 0)
        || !(512..=0x69000).contains(&stage.len())
        || !stage.windows(21).any(|x| x == b"RUSTALLER-BOOT-BIOS-1")
    {
        return Err("invalid release BIOS stage or schema".into());
    }
    Ok(())
}
pub(super) fn install(layout: &DiskLayout, work: &Path) -> Result<()> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(0x20000)
        .open(layout.device())
        .map_err(|e| e.to_string())?; // O_NOFOLLOW
    if !file
        .metadata()
        .map_err(|e| e.to_string())?
        .file_type()
        .is_block_device()
    {
        return Err("expected validated BIOS block device".into());
    }
    let boot = layout
        .partitions()
        .iter()
        .find(|p| p.role == PartitionRole::Boot)
        .ok_or("missing boot partition")?;
    let end = boot.end_mib.ok_or("unbounded boot partition")?;
    embed(
        &file,
        layout.table() == "gpt",
        boot.start_mib * 2048,
        (end - boot.start_mib) * 2048,
        &fs::read(work.join("bios-mbr.bin")).map_err(|e| e.to_string())?,
        &fs::read(work.join("bios-stage2.bin")).map_err(|e| e.to_string())?,
    )
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}
fn read(file: &fs::File, offset: u64, bytes: &mut [u8]) -> Result<()> {
    file.read_exact_at(bytes, offset).map_err(|e| e.to_string())
}

fn gpt_embedding(file: &fs::File, start: u64, len: u64) -> Result<(u64, u64)> {
    let invalid = "BIOS GPT does not match the planned embedding and FAT32 partitions";
    let mut h = [0; 512];
    read(file, 512, &mut h)?;
    let size = u32at(&h, 12) as usize;
    if &h[..8] != b"EFI PART"
        || !(92..=512).contains(&size)
        || u64at(&h, 24) != 1
        || u64at(&h, 72) != 2
    {
        return Err(invalid.into());
    }
    let crc = u32at(&h, 16);
    h[16..20].fill(0);
    if crc32(&h[..size]) != crc {
        return Err(invalid.into());
    }
    let (n, width) = (u32at(&h, 80) as usize, u32at(&h, 84) as usize);
    if !(3..=256).contains(&n) || width != 128 {
        return Err(invalid.into());
    }
    let mut table = vec![0; n * width];
    read(file, 1024, &mut table)?;
    if crc32(&table) != u32at(&h, 88) {
        return Err(invalid.into());
    }
    let e: Vec<_> = table.chunks_exact(width).collect();
    const BIOS: [u8; 16] = [
        0x48, 0x61, 0x68, 0x21, 0x49, 0x64, 0x6f, 0x6e, 0x74, 0x4e, 0x65, 0x65, 0x64, 0x45, 0x46,
        0x49,
    ];
    const BASIC: [u8; 16] = [
        0xa2, 0xa0, 0xd0, 0xeb, 0xe5, 0xb9, 0x33, 0x44, 0x87, 0xc0, 0x68, 0xb6, 0xb7, 0x26, 0x99,
        0xc7,
    ];
    const LINUX: [u8; 16] = [
        0xaf, 0x3d, 0xc6, 0x0f, 0x83, 0x84, 0x72, 0x47, 0x8e, 0x79, 0x3d, 0x69, 0xd8, 0x47, 0x7d,
        0xe4,
    ];
    if e[0][..16] != BIOS
        || !(e[1][..16] == BASIC || e[1][..16] == LINUX)
        || e[2][..16] != LINUX
        || e[3..].iter().any(|v| v[..16] != [0; 16])
        || u64at(e[0], 32) != 2048
        || u64at(e[0], 40) != 4095
        || start != 4096
        || u64at(e[1], 32) != start
        || u64at(e[1], 40) != start + len - 1
        || u64at(e[2], 32) != start + len
        || u64at(e[2], 40) < start + len
        || u64at(e[2], 40) > u64at(&h, 48)
        || u64at(&h, 40) > 2048
    {
        return Err(invalid.into());
    }
    Ok((2048, 2048))
}

fn embed(file: &fs::File, gpt: bool, start: u64, len: u64, mbr: &[u8], stage: &[u8]) -> Result<()> {
    validate_stages(mbr, stage)?;
    let mut sector = [0; 512];
    read(file, 0, &mut sector)?;
    if sector[510..] != [0x55, 0xaa] {
        return Err("missing MBR signature".into());
    }
    let p = &sector[446..510];
    let (lba, capacity) = if gpt {
        if p[4] != 0xee || u32at(p, 8) != 1 || p[16..].iter().any(|b| *b != 0) {
            return Err("invalid GPT protective MBR".into());
        }
        gpt_embedding(file, start, len)?
    } else {
        if start != 2048
            || p[0] != 0x80
            || !matches!(p[4], 0xb | 0xc)
            || u64::from(u32at(p, 8)) != start
            || u64::from(u32at(p, 12)) != len
            || p[20] != 0x83
            || u64::from(u32at(p, 24)) != start + len
            || u32at(p, 28) == 0
            || p[32..].iter().any(|b| *b != 0)
        {
            return Err("unexpected BIOS/MBR partition table".into());
        }
        (1, start - 1)
    };
    let sectors = stage.len().div_ceil(512);
    if sectors as u64 > capacity {
        return Err("BIOS stage exceeds embedding area".into());
    }
    let mut code = mbr.to_vec();
    code[408..416].copy_from_slice(&lba.to_le_bytes());
    code[416..418].copy_from_slice(&(sectors as u16).to_le_bytes());
    code[418..426].copy_from_slice(&start.to_le_bytes());
    code[426..434].copy_from_slice(&len.to_le_bytes());
    let mut padded = stage.to_vec();
    padded.resize(sectors * 512, 0);
    let mut verify = vec![0; padded.len()];
    file.write_all_at(&padded, lba * 512)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    read(file, lba * 512, &mut verify)?;
    if verify != padded {
        return Err("BIOS stage read-back mismatch".into());
    }
    // Publish stage 1 last. Only the first 440 bytes are replaced.
    file.write_all_at(&code, 0)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    let mut after = [0; 512];
    read(file, 0, &mut after)?;
    if after[..440] != code || after[440..] != sector[440..] {
        return Err("BIOS boot sector read-back mismatch".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_bootstrap_preserves_disk_identity_and_rejects_unexpected_partitions() {
        // A private regular file exercises the embedding algorithm. The live
        // adapter independently requires an identity-validated block device.
        let name = std::env::temp_dir().join(format!("rustaller-bios-test-{}", std::process::id()));
        let disk = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&name)
            .unwrap();
        fs::remove_file(&name).unwrap();
        disk.set_len(8 * 1024 * 1024).unwrap();
        let mut sector = [0u8; 512];
        sector[440..444].copy_from_slice(b"disk");
        sector[510..].copy_from_slice(&[0x55, 0xaa]);
        sector[446] = 0x80;
        sector[450] = 0xc;
        sector[454..458].copy_from_slice(&2048u32.to_le_bytes());
        sector[458..462].copy_from_slice(&4096u32.to_le_bytes());
        sector[466] = 0x83;
        sector[470..474].copy_from_slice(&6144u32.to_le_bytes());
        sector[474..478].copy_from_slice(&4096u32.to_le_bytes());
        disk.write_all_at(&sector, 0).unwrap();
        let mut mbr = [0u8; 440];
        mbr[400..408].copy_from_slice(b"RBBOOT01");
        let mut stage = [0x90; 777];
        stage[..21].copy_from_slice(b"RUSTALLER-BOOT-BIOS-1");
        assert!(embed(&disk, true, 2048, 4096, &mbr, &stage).is_err());
        assert!(embed(&disk, false, 2048, 8192, &mbr, &stage).is_err());
        let mut actual = [0; 512];
        read(&disk, 0, &mut actual).unwrap();
        assert_eq!(actual, sector);
        embed(&disk, false, 2048, 4096, &mbr, &stage).unwrap();
        read(&disk, 0, &mut actual).unwrap();
        assert_eq!(actual[440..], sector[440..]);
        assert_eq!(u64at(&actual, 408), 1);
        assert_eq!(actual[416], 2);
        let mut payload = [0; 1024];
        read(&disk, 512, &mut payload).unwrap();
        assert_eq!(payload[..777], stage);
        assert!(payload[777..].iter().all(|b| *b == 0));
    }
}
