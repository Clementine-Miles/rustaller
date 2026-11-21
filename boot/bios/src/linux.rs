use crate::{
    Result,
    fat::{Fat, u32at},
    machine,
};
use alloc::{format, vec};
use core::ptr;
use rustaller_boot_core::{CONFIG_PATH, Config, Entry, MAX_CONFIG_BYTES};

fn select(cfg: &Config) -> &Entry {
    let keys = b"123456789abcdefg";
    if cfg.boot.serial {
        machine::serial_init(cfg.boot.serial_baudrate)
    }
    machine::message("RUSTALLER-BOOT-BIOS-1\r\n", cfg.boot.serial);
    for (e, k) in cfg.entries.iter().zip(keys) {
        machine::message(
            &format!(
                "[{}] {} {}{}\r\n",
                *k as char,
                e.name,
                e.version.as_deref().unwrap_or(""),
                if e.name == cfg.boot.default {
                    " (default)"
                } else {
                    ""
                }
            ),
            cfg.boot.serial,
        )
    }
    for _ in 0..u32::from(cfg.boot.timeout_seconds) * 20 {
        let key = machine::key(cfg.boot.serial);
        if key == 13 {
            break;
        }
        if let Some(e) = keys
            .iter()
            .position(|k| *k == key)
            .and_then(|i| cfg.entries.get(i))
        {
            return e;
        }
        machine::wait();
    }
    cfg.default_entry()
}
fn config(fat: &mut Fat, path: &str) -> Result<Config> {
    let file = fat.file(path)?;
    if file.size as usize > MAX_CONFIG_BYTES {
        return Err("oversize boot.toml");
    }
    let mut buf = vec![0; file.size as usize];
    fat.bytes(file, 0, &mut buf)?;
    Config::parse(core::str::from_utf8(&buf).map_err(|_| "boot.toml encoding")?)
        .map_err(|_| "invalid boot.toml")
}
pub fn boot(count: u32) -> Result<()> {
    const HEAP: u32 = 8 * 1024 * 1024;
    let map = machine::map(count)?;
    let heap = machine::allocate(map, HEAP, 4096, 1 << 32, &[])?;
    machine::init_heap(heap, HEAP);
    // Stage 1 is still resident at 0x7c00. Its installer patches these
    // unaligned u64 fields only after validating the actual partition table.
    let mut fat = Fat::open(
        unsafe { ptr::read_unaligned((0x7c00 + 418) as *const u64) },
        unsafe { ptr::read_unaligned((0x7c00 + 426) as *const u64) },
    )?;
    let cfg = config(&mut fat, CONFIG_PATH).or_else(|e| {
        machine::message("Opening recovery configuration\r\n", true);
        config(&mut fat, "/rustaller/boot.toml.previous").map_err(|_| e)
    })?;
    let entry = select(&cfg);
    let kernel = fat.file(&entry.kernel)?;
    let mut header = [0u8; 4096];
    fat.bytes(kernel, 0, &mut header)?;
    if !rustaller_boot_core::valid_bzimage(&header) {
        return Err("unsupported x86-64 Linux bzImage");
    }
    let setup = if header[0x1f1] == 0 {
        4
    } else {
        u32::from(header[0x1f1])
    };
    let offset = (setup + 1) * 512;
    let len = kernel
        .size
        .checked_sub(offset)
        .ok_or("truncated Linux setup")?;
    let init_size = u32at(&header, 0x260);
    let align = u32at(&header, 0x230);
    let cmd = cfg.cmdline().map_err(|_| "invalid command line")?;
    if len == 0
        || init_size < len
        || !align.is_power_of_two()
        || align < 4096
        || cmd.len() > u32at(&header, 0x238) as usize
    {
        return Err("invalid Linux memory requirements");
    }
    // Prefer the advertised physical load address. Fall back to a suitably
    // aligned placement only for a relocatable kernel.
    let pref = u64::from_le_bytes(header[0x258..0x260].try_into().unwrap());
    let used = [(heap, HEAP)];
    let dest = pref
        .checked_add(u64::from(init_size))
        .and_then(|end| machine::allocate(map, init_size, align, end, &used).ok())
        .filter(|p| u64::from(*p) == pref)
        .or_else(|| machine::allocate(map, init_size, align, 1 << 32, &used).ok())
        .ok_or("no kernel workspace")?;
    let payload = unsafe { machine::ram(dest, len) };
    fat.bytes(kernel, offset, payload)?;
    let mut files = vec![];
    let mut total = 0u32;
    for path in &entry.initrds {
        let f = fat.file(path)?;
        let off = total.next_multiple_of(4);
        total = off
            .checked_add(f.size)
            .filter(|n| *n <= 512 * 1024 * 1024)
            .ok_or("initrd stream too large")?;
        files.push((off, f));
    }
    let rd = machine::allocate(
        map,
        total,
        4096,
        u64::from(u32at(&header, 0x22c)) + 1,
        &[(heap, HEAP), (dest, init_size)],
    )?;
    let stream = unsafe { machine::ram(rd, total) };
    let mut end = 0;
    for (off, file) in files {
        stream[end..off as usize].fill(0);
        end = (off + file.size) as usize;
        fat.bytes(file, 0, &mut stream[off as usize..end])?;
    }
    let params = unsafe { machine::ram(0x6000, 4096) };
    params.fill(0);
    let header_end = 0x202 + header[0x201] as usize;
    if !(0x264..=0x290).contains(&header_end) {
        return Err("unsupported setup header length");
    }
    params[0x1f1..header_end].copy_from_slice(&header[0x1f1..header_end]);
    params[0x210] = 0xff;
    for (i, n) in [(0x214, dest), (0x218, rd), (0x21c, total), (0x228, 0x7000)] {
        params[i..i + 4].copy_from_slice(&n.to_le_bytes());
    }
    params[0x1e8] = count as u8;
    unsafe {
        ptr::copy_nonoverlapping(
            map.as_ptr().cast::<u8>(),
            params.as_mut_ptr().add(0x2d0),
            map.len() * 20,
        );
    }
    params[6] = 3;
    params[7] = 80;
    params[14] = 25;
    params[15] = 1; // VGA text screen_info
    let line = unsafe { machine::ram(0x7000, cmd.len() as u32 + 1) };
    line[..cmd.len()].copy_from_slice(cmd.as_bytes());
    line[cmd.len()] = 0;
    machine::message("Starting Linux\r\n", cfg.boot.serial);
    // Protected-mode entry requires paging off, interrupts disabled, flat
    // GDT selectors 0x10/0x18 and ESI pointing at the zero page.
    unsafe { machine::enter_linux(dest) }
}
