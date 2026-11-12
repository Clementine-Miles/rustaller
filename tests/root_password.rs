#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
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
        .args(["--exact", "password_prompt_child", "--nocapture"])
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
