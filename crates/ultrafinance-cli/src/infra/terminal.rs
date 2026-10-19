// ECS Exec's session-manager-plugin can leave local terminal modes enabled
// when tmux disconnects. Restore the local terminal on both success and errors.
#[cfg(unix)]
const CLEAR_MOUSE: &[u8] =
    b"\x1b[?1000l\x1b[?1001l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\x1b[?1015l\x1b[?1016l";

pub(super) struct TerminalState {
    #[cfg(unix)]
    input: libc::c_int,
    #[cfg(unix)]
    output: libc::c_int,
    #[cfg(unix)]
    saved: Option<libc::termios>,
    #[cfg(unix)]
    clear_mouse: bool,
}

impl TerminalState {
    pub(super) fn capture() -> Self {
        #[cfg(unix)]
        {
            Self::from_fds(libc::STDIN_FILENO, libc::STDOUT_FILENO)
        }
        #[cfg(not(unix))]
        {
            Self {}
        }
    }

    #[cfg(unix)]
    fn from_fds(input: libc::c_int, output: libc::c_int) -> Self {
        // Both descriptors are borrowed and remain owned by the caller.
        let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
        let saved = unsafe {
            if libc::tcgetattr(input, saved.as_mut_ptr()) == 0 {
                Some(saved.assume_init())
            } else {
                None
            }
        };
        Self {
            input,
            output,
            saved,
            clear_mouse: unsafe { libc::isatty(output) == 1 },
        }
    }
}

impl Drop for TerminalState {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            if let Some(saved) = &self.saved {
                libc::tcsetattr(self.input, libc::TCSANOW, saved);
            }
            if self.clear_mouse {
                let mut remaining = CLEAR_MOUSE;
                while !remaining.is_empty() {
                    let written =
                        libc::write(self.output, remaining.as_ptr().cast(), remaining.len());
                    if written > 0 {
                        remaining = &remaining[written as usize..];
                    } else if written == -1
                        && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                    {
                        continue;
                    } else {
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
#[path = "../../tests/unit/infra/terminal/tests.rs"]
mod tests;

/// A grouped child still needs foreground ownership for interactive ECS Exec.
/// Keep SIGTTOU blocked on this thread while delegated so progress writes and
/// restoring the original foreground group cannot suspend the launcher.
pub(super) struct ForegroundGroup {
    #[cfg(unix)]
    original: libc::pid_t,
    #[cfg(unix)]
    mask: libc::sigset_t,
}

impl ForegroundGroup {
    pub(super) fn give_to(group: u32) -> std::io::Result<Option<Self>> {
        #[cfg(unix)]
        unsafe {
            if libc::isatty(libc::STDIN_FILENO) != 1 {
                return Ok(None);
            }
            let original = libc::tcgetpgrp(libc::STDIN_FILENO);
            if original == -1 {
                return Err(std::io::Error::last_os_error());
            }
            let mut blocked: libc::sigset_t = std::mem::zeroed();
            let mut mask: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut blocked);
            libc::sigaddset(&mut blocked, libc::SIGTTOU);
            let error = libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut mask);
            if error != 0 {
                return Err(std::io::Error::from_raw_os_error(error));
            }
            if libc::tcsetpgrp(libc::STDIN_FILENO, group as libc::pid_t) != 0 {
                let error = std::io::Error::last_os_error();
                libc::pthread_sigmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
                return Err(error);
            }
            // The plugin may already have hit SIGTTIN/SIGTTOU before the parent
            // handed over the terminal. Resume only our new child group.
            libc::kill(-(group as libc::pid_t), libc::SIGCONT);
            Ok(Some(Self { original, mask }))
        }
        #[cfg(not(unix))]
        {
            let _ = group;
            Ok(None)
        }
    }
}
impl Drop for ForegroundGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::tcsetpgrp(libc::STDIN_FILENO, self.original);
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.mask, std::ptr::null_mut());
        }
    }
}
