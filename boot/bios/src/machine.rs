use crate::Result;
use core::{
    alloc::{GlobalAlloc, Layout},
    arch::{asm, global_asm},
    ptr, slice,
    sync::atomic::{AtomicUsize, Ordering},
};
global_asm!(include_str!("entry.S"), options(att_syntax));
unsafe extern "C" {
    pub fn read_sectors(low: u32, high: u32, count: u32) -> u32;
    fn console_service(service: u32, c: u32) -> u32;
    pub fn enter_linux(address: u32) -> !;
}
pub fn halt() -> ! {
    loop {
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}

// After E820 validation the parser owns one private arena until kernel entry.
// No other processor or interrupt handler runs Rust or uses this memory.
static NEXT: AtomicUsize = AtomicUsize::new(0);
static END: AtomicUsize = AtomicUsize::new(0);
struct Arena;
#[global_allocator]
static ALLOC: Arena = Arena;
unsafe impl GlobalAlloc for Arena {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let old = NEXT.load(Ordering::Relaxed);
        let Some(p) = old.checked_next_multiple_of(l.align()) else {
            return ptr::null_mut();
        };
        let Some(end) = p.checked_add(l.size()) else {
            return ptr::null_mut();
        };
        if old == 0 || end > END.load(Ordering::Relaxed) {
            return ptr::null_mut();
        }
        NEXT.store(end, Ordering::Relaxed);
        p as *mut u8
    }
    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}
pub fn init_heap(start: u32, size: u32) {
    NEXT.store(start as usize, Ordering::Relaxed);
    END.store((start + size) as usize, Ordering::Relaxed);
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct Region {
    pub start: u64,
    pub size: u64,
    pub kind: u32,
}
impl Region {
    fn end(self) -> u64 {
        self.start.saturating_add(self.size)
    }
}
pub fn map(count: u32) -> Result<&'static [Region]> {
    if !(1..=128).contains(&count) {
        return Err("invalid E820 map");
    }
    // entry.S obtained exactly count consecutive packed 20-byte BIOS records.
    Ok(unsafe { slice::from_raw_parts(0x5000 as *const Region, count as usize) })
}
// Pick the highest aligned gap in usable memory below limit. Reserved E820
// entries take precedence even when a firmware supplies overlapping records.
pub fn allocate(
    map: &[Region],
    size: u32,
    align: u32,
    limit: u64,
    used: &[(u32, u32)],
) -> Result<u32> {
    let mut best = 0;
    for &r in map.iter().filter(|r| r.kind == 1) {
        let low = r.start.max(1024 * 1024);
        let mut high = r.end().min(limit).min(0xfffff000);
        while let Some(start) = high
            .checked_sub(u64::from(size))
            .map(|v| v & !(u64::from(align) - 1))
        {
            if start < low {
                break;
            }
            let end = start + u64::from(size);
            let overlap = map
                .iter()
                .filter(|r| r.kind != 1)
                .map(|r| (r.start, r.end()))
                .chain(
                    used.iter()
                        .map(|&(p, n)| (u64::from(p), u64::from(p) + u64::from(n))),
                )
                .filter(|&(p, q)| p < end && start < q)
                .map(|(p, _)| p)
                .min();
            if let Some(p) = overlap {
                high = p;
            } else {
                best = best.max(start as u32);
                break;
            }
        }
    }
    if best == 0 {
        Err("insufficient contiguous E820 RAM")
    } else {
        Ok(best)
    }
}
pub unsafe fn ram<'a>(start: u32, len: u32) -> &'a mut [u8] {
    // Caller has reserved this nonoverlapping physical range under identity
    // mapping, or owns it through the explicit low-memory map in entry.S.
    unsafe { slice::from_raw_parts_mut(start as *mut u8, len as usize) }
}
fn input(port: u16) -> u8 {
    let value: u8;
    unsafe { asm!("in al, dx",in("dx")port,out("al")value,options(nomem,nostack,preserves_flags)) };
    value
}
fn output(port: u16, v: u8) {
    unsafe { asm!("out dx, al",in("dx")port,in("al")v,options(nomem,nostack,preserves_flags)) };
}
pub fn serial_init(baud: u32) {
    let div = 115200 / baud;
    output(0x3f9, 0);
    output(0x3fb, 0x80);
    output(0x3f8, div as u8);
    output(0x3f9, 0);
    output(0x3fb, 3);
    output(0x3fa, 7);
    output(0x3fc, 3);
}
pub fn message(text: &str, serial: bool) {
    for byte in text.bytes() {
        unsafe {
            console_service(2, byte.into());
        }
        if serial {
            for _ in 0..10000 {
                let s = input(0x3fd);
                if s == 0xff {
                    break;
                }
                if s & 32 != 0 {
                    output(0x3f8, byte);
                    break;
                }
            }
        }
    }
}
pub fn key(serial: bool) -> u8 {
    if serial {
        let s = input(0x3fd);
        if s != 255 && s & 1 != 0 {
            return input(0x3f8);
        }
    }
    unsafe { console_service(1, 0) as u8 }
}
pub fn wait() {
    unsafe {
        console_service(3, 0);
    }
}

// The freestanding target has no libc. Volatile byte operations keep LLVM
// from lowering these implementations into calls back to the same symbols.
use core::ffi::c_void;
#[unsafe(no_mangle)]
unsafe extern "C" fn memcpy(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    for i in 0..n {
        unsafe {
            dst.cast::<u8>()
                .add(i)
                .write_volatile(src.cast::<u8>().add(i).read_volatile())
        }
    }
    dst
}
#[unsafe(no_mangle)]
unsafe extern "C" fn memset(dst: *mut c_void, value: i32, n: usize) -> *mut c_void {
    unsafe {
        asm!("rep stosb",inout("edi")dst=>_,in("al")value as u8,inout("ecx")n=>_,options(nostack,preserves_flags))
    };
    dst
}
#[unsafe(no_mangle)]
unsafe extern "C" fn memmove(dst: *mut c_void, src: *const c_void, n: usize) -> *mut c_void {
    if (dst as usize) > src as usize && (dst as usize) - (src as usize) < n {
        for i in (0..n).rev() {
            unsafe {
                dst.cast::<u8>()
                    .add(i)
                    .write_volatile(src.cast::<u8>().add(i).read_volatile())
            }
        }
    } else {
        unsafe {
            memcpy(dst, src, n);
        }
    }
    dst
}
#[unsafe(no_mangle)]
unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> i32 {
    for i in 0..n {
        let x = unsafe { a.cast::<u8>().add(i).read_volatile() };
        let y = unsafe { b.cast::<u8>().add(i).read_volatile() };
        if x != y {
            return i32::from(x) - i32::from(y);
        }
    }
    0
}
#[unsafe(no_mangle)]
unsafe extern "C" fn bcmp(a: *const c_void, b: *const c_void, n: usize) -> i32 {
    unsafe { memcmp(a, b, n) }
}
