#![cfg(all(feature = "tui", target_os = "linux"))]

use std::fs::File;
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

fn attributes(terminal: &File) -> libc::termios {
    let mut termios = MaybeUninit::uninit();
    assert_eq!(
        unsafe { libc::tcgetattr(terminal.as_raw_fd(), termios.as_mut_ptr()) },
        0
    );
    unsafe { termios.assume_init() }
}

#[test]
fn interactive_cancel_restores_terminal_without_entering_host_preflight() {
    cancel_in_terminal(&[b"\x03"]);
}

#[test]
fn nested_password_editor_masks_input_and_cancel_restores_terminal() {
    let output = cancel_in_terminal(&[
        b"\r", // Home -> choose a system
        b"\r",
        b"\r",
        b"\x1b[B",
        b"\r",
        b"\x1b[B",
        b"\r",
        b"\r",
        b"TUI-PRIVATE-PASSWORD-SENTINEL",
        b"\x03",
    ]);
    assert!(output.contains("Password editor"), "{output}");
    assert!(!output.contains("TUI-PRIVATE-PASSWORD-SENTINEL"));
}

fn cancel_in_terminal(input: &[&[u8]]) -> String {
    let steps: Vec<_> = input.iter().map(|bytes| (*bytes, None)).collect();
    run_in_terminal(&steps)
}

#[test]
fn native_keygen_prompts_outside_raw_mode_encrypts_key_and_restores_tui() {
    use std::os::unix::fs::PermissionsExt;
    struct OwnedDirectory(std::path::PathBuf);
    impl Drop for OwnedDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let directory =
        std::env::temp_dir().join(format!("rustaller-native-key-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let _owned = OwnedDirectory(directory.clone());
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.join("client-key");
    let path_bytes = path.to_str().unwrap().as_bytes();
    let input: Vec<(&[u8], Option<&str>)> = vec![
        (b"\r", None), // Home -> choose a system
        (b"\r", None),
        (b"\r", None),
        (b"\x1b[B", None),
        (b"\r", None), // account/auth
        (b"\r", None),
        (b"\x1b[B", None),
        (b"\r", None), // public key method
        (b"\x1b[B", None),
        (b"\r", None),
        (b"\r", None), // keys/keygen
        (b"\x1b[B", None),
        (b"\r", None),
        (b"\x15", None),
        (b"1", None),
        (b"\r", None), // test-only fast KDF
        (b"\x1b[B", None),
        (b"\x1b[B", None),
        (b"\r", None),
        (b"\x15", None),
        (path_bytes, None),
        (b"\r", None),
        (b"\x1b[F", None),
        (b"\x1b[A", None), // Last item is Back; generation is immediately above it.
        (b"\r", Some("Enter passphrase")),
        (
            b"NATIVE-PASSPHRASE-SENTINEL\r",
            Some("Enter same passphrase again"),
        ),
        (b"NATIVE-PASSPHRASE-SENTINEL\r", Some("client-key.pub")),
        (b"\x03", None),
    ];
    let output = run_in_terminal(&input);
    assert!(!output.contains("NATIVE-PASSPHRASE-SENTINEL"));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        Command::new("ssh-keygen")
            .args([
                "-lf",
                path.with_file_name("client-key.pub").to_str().unwrap()
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    // Empty passphrase cannot unlock the key created with the native prompt.
    assert!(
        !Command::new("ssh-keygen")
            .args(["-y", "-P", "", "-f", path.to_str().unwrap()])
            .output()
            .unwrap()
            .status
            .success()
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn current_system_configuration_can_discard_a_draft_and_return_home_without_applying() {
    let output = run_in_terminal(&[
        (b"\x1b[B", None),
        (b"\r", Some("Current system")),
        (b"\x1b[B", None),
        (b"\r", Some("Host settings")),
        (b"\r", Some("Edit field")),
        (b"DISCARD-HOSTNAME-DRAFT", None),
        (b"\r", None),
        (b"\x1b", Some("Save draft and go back")),
        (b"\x1b[B", None),
        (b"\r", None),
        (b"\x1b", None),
        (b"\x03", None),
    ]);
    assert!(output.contains("Discard changes and go back"));
    assert!(output.contains("Configure current system"));
    assert!(!output.contains("Current-system configuration saved"));
}

fn run_in_terminal(input: &[(&[u8], Option<&str>)]) -> String {
    let mut master_fd = -1;
    let mut slave_fd = -1;
    let size = libc::winsize {
        ws_row: 28,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        },
        0
    );
    let master = unsafe { File::from_raw_fd(master_fd) };
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    for file in [&master, &slave] {
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    let before = attributes(&slave);
    let binary = std::env::var_os("TEST_REINSTALL_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_reinstall").into());
    let mut command = Command::new(binary);
    command
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave.try_clone().unwrap()));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    drop(command);
    let mut writer = master.try_clone().unwrap();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observer = Arc::clone(&observed);
    let reader = thread::spawn(move || {
        let mut master = master;
        let mut bytes = Vec::new();
        let mut ready = Some(ready_tx);
        let mut buffer = [0; 4096];
        while let Ok(count) = master.read(&mut buffer) {
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            observer.lock().unwrap().extend_from_slice(&buffer[..count]);
            if let Some(tx) = ready.take() {
                let _ = tx.send(());
            }
        }
        bytes
    });
    let ready = ready_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    let mut steps_ready = true;
    if ready {
        for (bytes, expected) in input {
            writer.write_all(bytes).unwrap();
            writer.flush().unwrap();
            if let Some(expected) = expected {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    if String::from_utf8_lossy(&observed.lock().unwrap()).contains(expected) {
                        break;
                    }
                    if Instant::now() >= deadline {
                        steps_ready = false;
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                if !steps_ready {
                    break;
                }
            } else {
                thread::sleep(Duration::from_millis(40));
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let after = attributes(&slave);
    drop(slave);
    drop(writer);
    let output = String::from_utf8_lossy(&reader.join().unwrap()).into_owned();
    assert!(
        ready && steps_ready && status.is_some_and(|status| status.success()),
        "{output}"
    );
    assert!(output.contains("\x1b[?1049h"));
    assert!(output.contains("\x1b[?1049l"));
    assert!(!output.contains("installer workflow is incomplete"));
    assert_eq!(before.c_lflag, after.c_lflag);
    assert_eq!(before.c_iflag, after.c_iflag);
    assert_eq!(before.c_oflag, after.c_oflag);
    assert_eq!(before.c_cflag, after.c_cflag);
    assert_eq!(before.c_cc, after.c_cc);
    output
}
