//! Hidden, confirmed host-terminal input for the separate root account password.
//! Linux x86_64 is the supported runtime ABI. No secret enters subprocess argv.
use crate::model::SecretString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

const LIMIT: usize = 8192;
static PROMPT_LOCK: Mutex<()> = Mutex::new(());
static INTERRUPTED: AtomicI32 = AtomicI32::new(0);
const SIGNALS: [i32; 3] = [2, 15, 1]; // SIGINT, SIGTERM, SIGHUP

// Linux asm-generic termbits kernel ABI, used by TCGETS/TCSETS rather than the
// libc termios structure (whose layout differs between libc implementations).
#[repr(C)]
#[derive(Clone, Copy)]
struct KernelTermios {
    input: u32,
    output: u32,
    control: u32,
    local: u32,
    line: u8,
    chars: [u8; 19],
}
unsafe extern "C" {
    fn ioctl(fd: i32, request: std::os::raw::c_ulong, ...) -> i32;
    fn signal(number: i32, handler: usize) -> usize;
    fn sigaction(
        number: i32,
        action: *const std::ffi::c_void,
        previous: *mut std::ffi::c_void,
    ) -> i32;
}

// The public libc sigaction structure differs between glibc and musl. Preserve
// it opaquely instead of interpreting either layout. Both Linux x86_64 layouts
// fit within 256 bytes and require at most pointer alignment.
#[repr(C, align(16))]
struct SavedAction([u8; 256]);
extern "C" fn interrupted(number: i32) {
    INTERRUPTED.store(number, Ordering::Relaxed);
}

struct TerminalGuard {
    terminal: File,
    original: KernelTermios,
    handlers: Vec<(i32, SavedAction)>,
}
impl TerminalGuard {
    fn new(terminal: File) -> io::Result<Self> {
        let mut original = std::mem::MaybeUninit::<KernelTermios>::uninit();
        // SAFETY: TCGETS writes the documented kernel termios layout through a
        // valid pointer; terminal stays open for this guard's entire lifetime.
        if unsafe { ioctl(terminal.as_raw_fd(), 0x5401, original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful TCGETS initialized all fields used below.
        let original = unsafe { original.assume_init() };
        let mut guard = Self {
            terminal,
            original,
            handlers: Vec::new(),
        };
        INTERRUPTED.store(0, Ordering::Relaxed);
        for number in SIGNALS {
            let mut previous = SavedAction([0; 256]);
            // SAFETY: previous is aligned and large enough for the supported
            // libc sigaction ABI. A null action queries without changing it.
            if unsafe { sigaction(number, std::ptr::null(), previous.0.as_mut_ptr().cast()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: handler has the C signal-handler ABI and only performs a
            // lock-free atomic store. The complete previous action, including
            // masks and SA_SIGINFO, is restored rather than just its pointer.
            if unsafe { signal(number, interrupted as *const () as usize) } == usize::MAX {
                return Err(io::Error::last_os_error());
            }
            guard.handlers.push((number, previous));
        }
        let mut hidden = original;
        // Disable canonical editing, echo and terminal-generated signals. A
        // literal Ctrl-C/D/Z cancels without terminating before restoration.
        hidden.local &= !(0x1 | 0x2 | 0x8 | 0x40 | 0x8000); // ISIG ICANON ECHO ECHONL IEXTEN
        hidden.input &= !(0x40 | 0x80 | 0x100 | 0x400); // INLCR IGNCR ICRNL IXON
        hidden.chars[6] = 0; // VMIN
        hidden.chars[5] = 1; // VTIME: cancellation checked at least every 100ms
        // SAFETY: TCSETS receives a valid initialized kernel termios value.
        if unsafe { ioctl(guard.terminal.as_raw_fd(), 0x5402, &hidden) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(guard)
    }
    fn read_secret(&mut self, prompt: &str) -> Result<SecretString, String> {
        self.terminal
            .write_all(prompt.as_bytes())
            .map_err(|_| "Cannot write password prompt")?;
        self.terminal
            .flush()
            .map_err(|_| "Cannot write password prompt")?;
        let mut value = PrivateBytes(Vec::new());
        loop {
            if INTERRUPTED.load(Ordering::Relaxed) != 0 {
                return Err("Root password entry cancelled".into());
            }
            let mut byte = [0];
            match self.terminal.read(&mut byte) {
                Ok(0) => continue,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err("Cannot read root password from the terminal".into()),
            }
            match byte[0] {
                b'\r' | b'\n' => break,
                3 | 4 | 26 => return Err("Root password entry cancelled".into()),
                8 | 127 => {
                    if let Some(last) = value.0.last().copied() {
                        *value.0.last_mut().expect("last byte") = 0;
                        value.0.pop();
                        if last & 0xc0 == 0x80 {
                            while value.0.last().is_some_and(|b| b & 0xc0 == 0x80) {
                                *value.0.last_mut().expect("continuation byte") = 0;
                                value.0.pop();
                            }
                            if let Some(byte) = value.0.last_mut() {
                                *byte = 0;
                            }
                            value.0.pop();
                        }
                    }
                }
                21 => value.0.fill(0),
                b if b < 32 => return Err("Root password cannot contain control characters".into()),
                b => {
                    if value.0.len() == LIMIT {
                        return Err("Root password exceeds 8192 bytes".into());
                    }
                    value.0.push(b);
                }
            }
            if byte[0] == 21 {
                value.0.clear();
            }
        }
        self.terminal
            .write_all(b"\n")
            .map_err(|_| "Cannot write password prompt")?;
        let text =
            std::str::from_utf8(&value.0).map_err(|_| "Root password must be valid UTF-8")?;
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err("Root password must be nonempty and contain no control characters".into());
        }
        Ok(SecretString::new(text))
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // SAFETY: restoring TCSETSF uses the original initialized structure on
        // our still-open tty. It also flushes unread secret input before echo.
        unsafe {
            ioctl(self.terminal.as_raw_fd(), 0x5404, &self.original);
        }
        let _ = self.terminal.write_all(b"\n");
        for (number, previous) in self.handlers.iter().rev() {
            // SAFETY: previous was populated by this libc's sigaction() for
            // this same signal. Its alignment and lifetime remain valid.
            unsafe {
                sigaction(*number, previous.0.as_ptr().cast(), std::ptr::null_mut());
            }
        }
    }
}
struct PrivateBytes(Vec<u8>);
impl Drop for PrivateBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

/// Prompt on the controlling terminal, confirming the password without echo.
/// Cancellation or mismatched entries returns a fixed, secret-free error.
pub fn prompt_root_password() -> Result<SecretString, String> {
    let _lock = PROMPT_LOCK
        .lock()
        .map_err(|_| "Root password prompt unavailable")?;
    let terminal = OpenOptions::new().read(true).write(true).open("/dev/tty")
        .map_err(|_| "A controlling terminal is required to enter the root password; use --root-password for noninteractive installation")?;
    let mut terminal =
        TerminalGuard::new(terminal).map_err(|_| "Cannot protect terminal password input")?;
    let first = terminal.read_secret("Root password: ")?;
    let second = terminal.read_secret("Confirm root password: ")?;
    if INTERRUPTED.load(Ordering::Relaxed) != 0 {
        return Err("Root password entry cancelled".into());
    }
    if first.expose() != second.expose() {
        return Err("The root passwords do not match".into());
    }
    Ok(first)
}
