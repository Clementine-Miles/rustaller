// The linker script is supplied by xtask. Re-link if its layout changes.
fn main() {
    println!("cargo:rerun-if-changed=layout.ld");
}
