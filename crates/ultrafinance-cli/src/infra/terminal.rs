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
