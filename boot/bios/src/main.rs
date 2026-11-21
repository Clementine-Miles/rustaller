#![cfg_attr(target_arch = "x86", no_std)]
#![cfg_attr(target_arch = "x86", no_main)]
#[cfg(target_arch = "x86")]
extern crate alloc;
#[cfg(target_arch = "x86")]
mod fat;
#[cfg(target_arch = "x86")]
mod linux;
#[cfg(target_arch = "x86")]
mod machine;
#[cfg(target_arch = "x86")]
type Result<T> = core::result::Result<T, &'static str>;
#[cfg(not(target_arch = "x86"))]
fn main() {
    eprintln!("build the BIOS image with cargo xtask bundle");
    std::process::exit(1);
}

#[cfg(target_arch = "x86")]
#[unsafe(no_mangle)]
extern "C" fn bios_main(count: u32) -> ! {
    machine::message("rustaller-boot\r\n", false);
    if let Err(e) = linux::boot(count) {
        machine::serial_init(115200);
        machine::message(e, true);
    }
    machine::halt()
}
#[cfg(target_arch = "x86")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    machine::message("BIOS loader panic", true);
    machine::halt()
}
