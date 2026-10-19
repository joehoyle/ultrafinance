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

#[test]
fn grouped_interactive_child_can_use_a_controlling_terminal() {
    let script = r#"
import fcntl,os,pty,subprocess,sys,termios
master,slave=pty.openpty()
def setup():
    os.setsid()
    fcntl.ioctl(0,termios.TIOCSCTTY,0)
    os.tcsetpgrp(0,os.getpgrp())
process=subprocess.Popen([sys.argv[1],'--exact','infra::terminal::tests::foreground_group_helper','--ignored','--nocapture'],stdin=slave,stdout=subprocess.PIPE,stderr=subprocess.PIPE,preexec_fn=setup)
try:
    out,err=process.communicate(timeout=10)
except subprocess.TimeoutExpired:
    process.kill()
    process.communicate()
    raise
finally:
    os.close(master)
    os.close(slave)
sys.stdout.buffer.write(out)
sys.stderr.buffer.write(err)
sys.exit(process.returncode)
"#;
    let output = std::process::Command::new("python3")
        .args(["-c", script])
        .arg(std::env::current_exe().unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "invoked by grouped_interactive_child_can_use_a_controlling_terminal in an isolated PTY"]
fn foreground_group_helper() {
    let original = unsafe { libc::tcgetpgrp(0) };
    assert_eq!(original, unsafe { libc::getpgrp() });
    let mut process = super::super::import_db::spawn(
        std::process::Command::new("python3").args(["-c", "import os,tty; tty.setraw(0); assert os.tcgetpgrp(0)==os.getpgrp(); print('foreground-ok')"])
            .stdout(std::process::Stdio::piped()),
    ).unwrap();
    {
        let _terminal = TerminalState::capture();
        let _foreground = ForegroundGroup::give_to(process.0.id()).unwrap().unwrap();
        let mut output = String::new();
        process
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(process.0.wait().unwrap().success());
        assert!(output.contains("foreground-ok"));
    }
    assert_eq!(unsafe { libc::tcgetpgrp(0) }, original);
}
