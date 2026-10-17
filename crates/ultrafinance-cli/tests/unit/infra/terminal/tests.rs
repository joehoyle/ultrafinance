use super::*;
use std::{
    io::Read,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

fn pty() -> (OwnedFd, OwnedFd) {
    let mut master = -1;
    let mut slave = -1;
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
}

fn attrs(fd: libc::c_int) -> libc::termios {
    let mut state = std::mem::MaybeUninit::uninit();
    assert_eq!(unsafe { libc::tcgetattr(fd, state.as_mut_ptr()) }, 0);
    unsafe { state.assume_init() }
}

fn run_session(fd: libc::c_int, fail: bool) -> Result<(), &'static str> {
    let _guard = TerminalState::from_fds(fd, fd);
    let mut raw = attrs(fd);
    unsafe {
        libc::cfmakeraw(&mut raw);
        assert_eq!(libc::tcsetattr(fd, libc::TCSANOW, &raw), 0);
    }
    if fail { Err("connection lost") } else { Ok(()) }
}

#[test]
fn tty_modes_and_mouse_reporting_are_restored_on_success_and_error() {
    for fail in [false, true] {
        let (master, slave) = pty();
        let initial = attrs(slave.as_raw_fd());
        let result = run_session(slave.as_raw_fd(), fail);
        assert_eq!(result.is_err(), fail);
        let restored = attrs(slave.as_raw_fd());
        assert_eq!(restored.c_iflag, initial.c_iflag);
        assert_eq!(restored.c_oflag, initial.c_oflag);
        assert_eq!(restored.c_cflag, initial.c_cflag);
        // Restoring canonical input may set the kernel's pending-retype flag.
        assert_eq!(
            restored.c_lflag & !libc::PENDIN,
            initial.c_lflag & !libc::PENDIN
        );
        assert_eq!(restored.c_cc, initial.c_cc);
        let mut output = std::fs::File::from(master);
        let mut bytes = vec![0; CLEAR_MOUSE.len()];
        output.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, CLEAR_MOUSE);
    }
}

#[test]
fn redirected_output_is_not_polluted_with_terminal_escape_codes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("output");
    let output = std::fs::File::create(&path).unwrap();
    let (_master, slave) = pty();
    drop(TerminalState::from_fds(
        slave.as_raw_fd(),
        output.as_raw_fd(),
    ));
    assert!(std::fs::read(path).unwrap().is_empty());
}
