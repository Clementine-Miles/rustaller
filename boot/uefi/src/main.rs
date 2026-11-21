//! Fallback-path application for the x86-64 Linux EFI stub.
#![cfg_attr(target_os = "uefi", no_main)]
#![cfg_attr(target_os = "uefi", no_std)]
#[cfg(target_os = "uefi")]
extern crate alloc;
#[cfg(target_os = "uefi")]
mod firmware;
#[cfg(target_os = "uefi")]
mod initrd;

// A workspace check can compile this crate on the host. Release tooling checks
// the actual target and validates the resulting PE/COFF EFI application.
#[cfg(not(target_os = "uefi"))]
fn main() {
    eprintln!("build rustaller-boot-uefi for x86_64-unknown-uefi");
    std::process::exit(1);
}
