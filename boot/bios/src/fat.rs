//! Read-only, partition-bounded FAT32 over BIOS INT 13h extensions.
use crate::Result;
use crate::machine::read_sectors;
use core::ptr;

pub struct Fat {
    start: u64,
    sectors: u32,
    fat: u32,
    data: u32,
    per_cluster: u8,
    clusters: u32,
    root: u32,
    cached_fat_sector: u32,
    cache: [u8; 512],
}
#[derive(Clone, Copy)]
pub struct File {
    cluster: u32,
    pub size: u32,
    directory: bool,
}
pub fn u16at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes(b[i..i + 2].try_into().unwrap())
}
pub fn u32at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}

fn read(lba: u64, sectors: u32, to: &mut [u8]) -> Result<()> {
    if sectors == 0 || sectors > 64 || to.len() > sectors as usize * 512 {
        return Err("invalid EDD read");
    }
    // The real-mode thunk reads at most 32 KiB into reserved low conventional
    // memory. Caller-owned Rust slices cannot overlap this bounce area.
    unsafe {
        if read_sectors(lba as u32, (lba >> 32) as u32, sectors) != 0 {
            return Err("BIOS disk read");
        }
        ptr::copy_nonoverlapping(0x8000 as *const u8, to.as_mut_ptr(), to.len());
    }
    Ok(())
}

impl Fat {
    pub fn open(start: u64, length: u64) -> Result<Self> {
        let mut b = [0; 512];
        read(start, 1, &mut b)?;
        let fat = u16at(&b, 14) as u32;
        let sectors = u32at(&b, 32);
        let fat_len = u32at(&b, 36);
        let spc = b[13];
        let data = fat_len
            .checked_mul(b[16].into())
            .and_then(|n| n.checked_add(fat))
            .ok_or("FAT geometry")?;
        if u16at(&b, 510) != 0xaa55
            || u16at(&b, 11) != 512
            || !spc.is_power_of_two()
            || spc > 128
            || !matches!(b[16], 1 | 2)
            || fat == 0
            || u16at(&b, 17) != 0
            || u16at(&b, 22) != 0
            || u16at(&b, 40) & 0x80 != 0
            || data >= sectors
            || u64::from(sectors) > length
            || start.checked_add(length).is_none()
        {
            return Err("unsupported FAT32 volume");
        }
        let clusters = (sectors - data) / u32::from(spc);
        let root = u32at(&b, 44);
        if !(65525..0x0ffffff5).contains(&clusters)
            || u64::from(clusters + 2) > u64::from(fat_len) * 128
            || !(2..clusters + 2).contains(&root)
        {
            return Err("invalid FAT32 allocation table");
        }
        Ok(Self {
            start,
            sectors,
            fat,
            data,
            per_cluster: spc,
            clusters,
            root,
            cached_fat_sector: u32::MAX,
            cache: [0; 512],
        })
    }
    fn sector(&self, cluster: u32) -> Result<u32> {
        if !(2..self.clusters + 2).contains(&cluster) {
            return Err("invalid FAT cluster");
        }
        Ok(self.data + (cluster - 2) * u32::from(self.per_cluster))
    }
    fn next(&mut self, cluster: u32) -> Result<Option<u32>> {
        self.sector(cluster)?;
        let sec = self.fat + cluster / 128;
        if self.cached_fat_sector != sec {
            read(self.start + u64::from(sec), 1, &mut self.cache)?;
            self.cached_fat_sector = sec;
        }
        let n = u32at(&self.cache, ((cluster % 128) * 4) as usize) & 0x0fffffff;
        if n >= 0x0ffffff8 {
            return Ok(None);
        }
        self.sector(n)?;
        Ok(Some(n))
    }
    pub fn file(&mut self, path: &str) -> Result<File> {
        if !rustaller_boot_core::valid_boot_path(path) {
            return Err("invalid boot path");
        }
        let mut f = File {
            cluster: self.root,
            size: 0,
            directory: true,
        };
        for name in path[1..].split('/') {
            if !f.directory {
                return Err("non-directory in boot path");
            }
            f = self.find(f.cluster, name.as_bytes())?;
        }
        if f.directory || f.size == 0 || f.size > 512 * 1024 * 1024 {
            return Err("invalid boot file");
        }
        // A chain must end at the declared file length. This rejects loops,
        // free sectors, truncated files and unexpected trailing allocations.
        let mut c = f.cluster;
        for _ in 1..f.size.div_ceil(u32::from(self.per_cluster) * 512) {
            c = self.next(c)?.ok_or("short FAT chain")?;
        }
        if self.next(c)?.is_some() {
            return Err("cyclic or overlong FAT chain");
        }
        Ok(f)
    }
    fn find(&mut self, mut cluster: u32, name: &[u8]) -> Result<File> {
        let mut buf = [0; 512];
        let mut long = [0xffff; 260];
        let (mut pending, mut checksum) = (0xffu8, 0u8);
        // Limit malformed or cyclic directories to 8192 sectors.
        let mut budget = 8192;
        loop {
            let sec = self.sector(cluster)?;
            for off in 0..u32::from(self.per_cluster) {
                if budget == 0 {
                    return Err("directory exceeds bound");
                }
                budget -= 1;
                read(self.start + u64::from(sec + off), 1, &mut buf)?;
                for e in buf.as_chunks::<32>().0 {
                    if e[0] == 0 {
                        return Err("file absent on boot volume");
                    }
                    if e[0] == 0xe5 {
                        pending = 0xff;
                        continue;
                    }
                    if e[11] == 0x0f {
                        let order = e[0] & 0x1f;
                        if e[0] & 0x40 != 0 {
                            pending = order;
                            checksum = e[13];
                            long.fill(0xffff);
                        }
                        if !(1..=20).contains(&order)
                            || pending != order
                            || checksum != e[13]
                            || e[12] != 0
                            || u16at(e, 26) != 0
                        {
                            pending = 0xff;
                            continue;
                        }
                        for (i, p) in [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
                            .into_iter()
                            .enumerate()
                        {
                            long[(usize::from(order) - 1) * 13 + i] = u16at(e, p);
                        }
                        pending -= 1;
                        continue;
                    }
                    let sum = e[..11]
                        .iter()
                        .fold(0u8, |s, b| s.rotate_right(1).wrapping_add(*b));
                    let n = long
                        .iter()
                        .position(|v| matches!(v, 0 | 0xffff))
                        .unwrap_or(long.len());
                    let matched = pending == 0
                        && sum == checksum
                        && name.len() == n
                        && name
                            .iter()
                            .zip(&long)
                            .all(|(a, b)| *b < 128 && a.eq_ignore_ascii_case(&(*b as u8)));
                    pending = 0xff;
                    let mut short = [0; 12];
                    let mut n = 0;
                    for &c in &e[..8] {
                        if c != b' ' {
                            short[n] = c;
                            n += 1;
                        }
                    }
                    if e[8] != b' ' {
                        short[n] = b'.';
                        n += 1;
                        for &c in &e[8..11] {
                            if c != b' ' {
                                short[n] = c;
                                n += 1;
                            }
                        }
                    }
                    if e[11] & 8 == 0 && (matched || name.eq_ignore_ascii_case(&short[..n])) {
                        let c = (u32::from(u16at(e, 20)) << 16) | u32::from(u16at(e, 26));
                        self.sector(c)?;
                        return Ok(File {
                            cluster: c,
                            size: u32at(e, 28),
                            directory: e[11] & 0x10 != 0,
                        });
                    }
                }
            }
            cluster = self.next(cluster)?.ok_or("file absent on boot volume")?;
        }
    }
    pub fn bytes(&mut self, f: File, offset: u32, mut out: &mut [u8]) -> Result<()> {
        if !offset.is_multiple_of(512) || u64::from(offset) + out.len() as u64 > u64::from(f.size) {
            return Err("invalid boot file range");
        }
        let mut skip = offset / 512;
        let spc = u32::from(self.per_cluster);
        let mut c = f.cluster;
        let mut remaining = self.clusters.min(f.size.div_ceil(spc * 512));
        while !out.is_empty() {
            if remaining == 0 {
                return Err("short FAT chain");
            }
            remaining -= 1;
            if skip >= spc {
                skip -= spc;
            } else {
                let sec = self.sector(c)?;
                let mut i = skip;
                skip = 0;
                while i < spc && !out.is_empty() {
                    let sectors = (spc - i).min(64).min(out.len().div_ceil(512) as u32);
                    if sec + i + sectors > self.sectors {
                        return Err("read outside FAT volume");
                    }
                    let n = out.len().min(sectors as usize * 512);
                    read(self.start + u64::from(sec + i), sectors, &mut out[..n])?;
                    out = &mut out[n..];
                    i += sectors;
                }
            }
            if !out.is_empty() {
                c = self.next(c)?.ok_or("short FAT chain")?;
            }
        }
        Ok(())
    }
}
