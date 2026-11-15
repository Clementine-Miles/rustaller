mod support;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod root_password {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    static SIGINFO_SEEN: AtomicBool = AtomicBool::new(false);
    extern "C" fn previous_handler(_: i32, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
        SIGINFO_SEEN.store(!info.is_null(), Ordering::Relaxed);
    }
    fn signal_action(number: i32) -> libc::sigaction {
        let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
        assert_eq!(
            unsafe { libc::sigaction(number, std::ptr::null(), &mut action) },
            0
        );
        action
    }

    #[test]
    fn password_prompt_child() {
        let Ok(expected) = std::env::var("RUSTALLER_PASSWORD_TEST_CHILD") else {
            return;
        };
        // Preserve nontrivial handlers, flags and masks, not merely the handler
        // pointer. All changes are inside this subprocess with its private PTY.
        let mut previous = Vec::new();
        for number in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let mut custom = unsafe { std::mem::zeroed::<libc::sigaction>() };
            custom.sa_sigaction = previous_handler as *const () as libc::sighandler_t;
            custom.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER;
            assert_eq!(unsafe { libc::sigemptyset(&mut custom.sa_mask) }, 0);
            assert_eq!(
                unsafe { libc::sigaddset(&mut custom.sa_mask, libc::SIGUSR1) },
                0
            );
            assert_eq!(
                unsafe { libc::sigaction(number, &custom, std::ptr::null_mut()) },
                0
            );
            previous.push((number, signal_action(number)));
        }
        let result = rustaller::host::password::prompt_root_password();
        for (number, before) in previous {
            let after = signal_action(number);
            assert_eq!(before.sa_sigaction, after.sa_sigaction);
            assert_eq!(before.sa_flags, after.sa_flags);
            for signal in 1..=64 {
                assert_eq!(
                    unsafe { libc::sigismember(&before.sa_mask, signal) },
                    unsafe { libc::sigismember(&after.sa_mask, signal) }
                );
            }
        }
        assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
        assert!(SIGINFO_SEEN.load(Ordering::Relaxed));
        match expected.as_str() {
            "success" => assert_eq!(result.unwrap().expose(), "ROOT-SECRET-SENTINEL"),
            "mismatch" => assert_eq!(result.unwrap_err(), "The root passwords do not match"),
            "cancel" => assert_eq!(result.unwrap_err(), "Root password entry cancelled"),
            _ => panic!("unknown test mode"),
        }
    }
    fn attrs(file: &File) -> libc::termios {
        // musl only writes the kernel's 19 control characters; initialize the
        // remaining libc slots before comparing the complete public structure.
        let mut value = MaybeUninit::zeroed();
        assert_eq!(
            unsafe { libc::tcgetattr(file.as_raw_fd(), value.as_mut_ptr()) },
            0
        );
        unsafe { value.assume_init() }
    }
    fn wait_for(master: &mut File, seen: &mut Vec<u8>, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if String::from_utf8_lossy(seen).contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "prompt timed out: {}",
                String::from_utf8_lossy(seen)
            );
            let mut pfd = libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut pfd, 1, 50) } > 0 {
                let mut b = [0; 1024];
                let n = master.read(&mut b).unwrap();
                seen.extend_from_slice(&b[..n]);
            }
        }
    }
    fn run_case(mode: &str, interrupt: bool) {
        let (mut master, mut slave) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let mut master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        let original = attrs(&slave);
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "root_password::password_prompt_child",
                "--nocapture",
            ])
            .env("RUSTALLER_PASSWORD_TEST_CHILD", mode)
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
        let mut seen = Vec::new();
        wait_for(&mut master, &mut seen, "Root password: ");
        assert_eq!(attrs(&slave).c_lflag & libc::ECHO, 0);
        master.write_all(b"ROOT-SECRET-SENTINEL").unwrap();
        if interrupt {
            assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        } else {
            master.write_all(b"\r").unwrap();
            wait_for(&mut master, &mut seen, "Confirm root password: ");
            master
                .write_all(if mode == "success" {
                    b"ROOT-SECRET-SENTINEL\r"
                } else {
                    b"OTHER-SECRET\r"
                })
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "child failed: {}",
                    String::from_utf8_lossy(&seen)
                );
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                panic!("password prompt did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        loop {
            let mut pfd = libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut pfd, 1, 0) } <= 0 {
                break;
            }
            let mut b = [0; 1024];
            let n = master.read(&mut b).unwrap();
            if n == 0 {
                break;
            }
            seen.extend_from_slice(&b[..n]);
        }
        let restored = attrs(&slave);
        assert_eq!(original.c_lflag, restored.c_lflag);
        assert_eq!(original.c_iflag, restored.c_iflag);
        assert_eq!(original.c_cc, restored.c_cc);
        assert!(!String::from_utf8_lossy(&seen).contains("ROOT-SECRET-SENTINEL"));
    }
    #[test]
    fn confirmed_password_is_hidden_and_terminal_restored() {
        run_case("success", false);
    }
    #[test]
    fn mismatched_password_restores_terminal() {
        run_case("mismatch", false);
    }
    #[test]
    fn external_sigint_cancels_and_restores_terminal() {
        run_case("cancel", true);
    }
}

#[cfg(all(feature = "tui", target_os = "linux"))]
mod ui_terminal {

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
        let mut input = keygen_input(path_bytes);
        input.extend([
            (
                b"NATIVE-PASSPHRASE-SENTINEL\r".as_slice(),
                Some("Enter same passphrase again"),
            ),
            (
                b"NATIVE-PASSPHRASE-SENTINEL\r".as_slice(),
                Some("client-key.pub"),
            ),
            (b"\x03".as_slice(), None),
        ]);
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
    fn keygen_input(path_bytes: &[u8]) -> Vec<(&[u8], Option<&str>)> {
        vec![
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
        ]
    }
    #[test]
    fn interrupted_native_keygen_leaves_the_terminal_restored() {
        let scratch = crate::support::Scratch::new("keygen-interrupt");
        let path = scratch.0.join("client-key");
        let path = path.to_str().unwrap();
        let mut input = keygen_input(path.as_bytes());
        input.push((b"\x03", None));
        run_in_terminal_with_signal(&input, true);
        assert!(!std::path::Path::new(path).exists());
    }
    fn run_in_terminal(input: &[(&[u8], Option<&str>)]) -> String {
        run_in_terminal_with_signal(input, false)
    }
    fn run_in_terminal_with_signal(input: &[(&[u8], Option<&str>)], allow_sigint: bool) -> String {
        use std::os::unix::process::ExitStatusExt;
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
        let binary = std::env::var_os("TEST_RUSTALLER_BIN")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_rustaller").into());
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
            ready
                && steps_ready
                && status.is_some_and(|status| status.success()
                    || allow_sigint && status.signal() == Some(libc::SIGINT)),
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
}
