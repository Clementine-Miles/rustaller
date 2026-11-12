pub mod codec;
pub mod command;
pub mod config;
pub mod deployment;
mod fsutil;
pub mod host {
    pub mod args;
    pub mod boot;
    pub mod current;
    pub mod disk;
    pub mod handoff;
    pub mod hostname;
    pub mod initrd;
    pub mod iso;
    pub mod password;
    pub mod preflight;
    pub mod sources;
    pub mod ssh_keygen;
}
pub mod model;
pub mod plan_file;
pub mod postinstall;
pub mod shared {
    pub mod configure;
    pub mod dns;
    pub mod network;
    pub mod storage;
    pub mod swapfile;
}
pub mod targets {
    pub mod cachyos;
    pub mod debian;
    pub mod install;
    pub mod packages;
    pub mod rolling;
}

pub const RUNTIME_VERSION: &str = "RUSTALLER-CONFIG-20261112-06";
pub const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub mod worker {
    pub mod args;
    pub mod firstboot;
    pub mod initrd_network;
    pub mod install;
    pub mod live;
    pub mod log_stream;
}

include!(concat!(env!("OUT_DIR"), "/embedded_worker.rs"));

pub fn worker_bytes() -> &'static [u8] {
    // Keep the payload address observable even before every host workflow uses
    // it: LTO can otherwise fold the startup emptiness check to a constant and
    // discard the complete worker from the release executable.
    std::hint::black_box(EMBEDDED_WORKER)
}
