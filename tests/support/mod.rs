//! Private, isolated directories shared by the independent integration crates.
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs, path::PathBuf};
#[allow(dead_code)]
pub const PUBLIC_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE2kmWV2TOsArVwWboLjrmQX8eZ7Dw6P/7uF6m2L9TM+ rustaller-test";

static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct Scratch(pub PathBuf);
// Each integration crate compiles this module independently.
#[allow(dead_code)]
impl Scratch {
    pub fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustaller-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }
    pub fn with_etc(label: &str) -> Self {
        let dir = Self::new(label);
        fs::create_dir(dir.0.join("etc")).unwrap();
        dir
    }
    pub fn write(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    pub fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).unwrap()
    }
    pub fn service(&self, name: &str) {
        let relative = format!("etc/init.d/{name}");
        self.write(&relative, "fixture; never executed\n");
        fs::set_permissions(self.0.join(relative), fs::Permissions::from_mode(0o755)).unwrap();
    }
    pub fn zonefile(&self, name: &str, bytes: &[u8]) {
        let path = self.0.join("usr/share/zoneinfo").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
