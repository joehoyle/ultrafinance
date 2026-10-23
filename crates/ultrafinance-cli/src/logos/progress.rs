//! One stderr renderer for market scans and concurrent discovery jobs.
use std::{
    io::{self, IsTerminal, Write},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone)]
enum Phase {
    Selecting,
    Discovering {
        total: usize,
        completed: usize,
        failed: usize,
        workers: u32,
    },
}

pub struct Progress {
    phase: Arc<Mutex<Phase>>,
    stop: Option<mpsc::Sender<()>>,
    renderer: Option<JoinHandle<()>>,
}
impl Progress {
    pub fn new(market: String) -> Self {
        let phase = Arc::new(Mutex::new(Phase::Selecting));
        let state = phase.clone();
        let (stop, receiver) = mpsc::channel();
        let renderer = thread::spawn(move || {
            let terminal = io::stderr().is_terminal();
            let started = Instant::now();
            let mut last = Instant::now();
            let mut visible = false;
            let mut previous_phase = false;
            let mut frame = 0;
            loop {
                let finished = match receiver.recv_timeout(Duration::from_millis(200)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
                    Err(mpsc::RecvTimeoutError::Timeout) => false,
                };
                let phase = state.lock().unwrap().clone();
                let discovering = matches!(phase, Phase::Discovering { .. });
                if terminal
                    || !visible
                    || discovering != previous_phase
                    || last.elapsed() >= Duration::from_secs(15)
                    || finished
                {
                    let mut message = format_status(&market, &phase, started.elapsed(), frame);
                    if terminal {
                        message = message
                            .chars()
                            .take(terminal_width().saturating_sub(1))
                            .collect();
                    }
                    let mut output = io::stderr().lock();
                    if terminal {
                        let _ = write!(output, "\r\x1b[2K{message}");
                        if finished {
                            let _ = writeln!(output);
                        }
                    } else {
                        let _ = writeln!(output, "{message}");
                    }
                    let _ = output.flush();
                    last = Instant::now();
                    visible = true;
                    previous_phase = discovering;
                }
                frame += 1;
                if finished {
                    break;
                }
            }
        });
        Self {
            phase,
            stop: Some(stop),
            renderer: Some(renderer),
        }
    }
    pub fn discover(&self, total: usize, workers: u32) {
        *self.phase.lock().unwrap() = Phase::Discovering {
            total,
            completed: 0,
            failed: 0,
            workers,
        };
    }
    pub fn record(&self, success: bool) {
        if let Phase::Discovering {
            completed, failed, ..
        } = &mut *self.phase.lock().unwrap()
        {
            *completed += 1;
            if !success {
                *failed += 1;
            }
        }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(renderer) = self.renderer.take() {
            let _ = renderer.join();
        }
    }
}

fn terminal_width() -> usize {
    #[cfg(unix)]
    {
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut size) } == 0
            && size.ws_col > 0
        {
            return size.ws_col as usize;
        }
    }
    100
}

fn format_status(market: &str, phase: &Phase, elapsed: Duration, frame: usize) -> String {
    match phase {
        Phase::Selecting => format!(
            "{} Selecting websites for {market} | {}s",
            ['|', '/', '-', '\\'][frame % 4],
            elapsed.as_secs()
        ),
        Phase::Discovering {
            total,
            completed,
            failed,
            workers,
        } => {
            let percent = if *total == 0 {
                100.0
            } else {
                *completed as f64 / *total as f64 * 100.0
            };
            let filled = if *total == 0 {
                16
            } else {
                ((*completed as f64 / *total as f64) * 16.0) as usize
            }
            .min(16);
            format!(
                "Logos {market} [{}{}] {completed}/{total} ({percent:.1}%) | {} saved, {failed} failed | {workers} workers | {}s",
                "=".repeat(filled),
                " ".repeat(16 - filled),
                completed.saturating_sub(*failed),
                elapsed.as_secs()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_reports_partial_failure_and_empty_market_without_website_details() {
        let phase = Phase::Discovering {
            total: 10,
            completed: 6,
            failed: 2,
            workers: 4,
        };
        let status = format_status("CA", &phase, Duration::from_secs(12), 0);
        assert!(status.contains("6/10 (60.0%)"));
        assert!(status.contains("4 saved, 2 failed"));
        assert!(status.contains("4 workers | 12s"));
        let empty = Phase::Discovering {
            total: 0,
            completed: 0,
            failed: 0,
            workers: 4,
        };
        assert!(format_status("CA", &empty, Duration::ZERO, 0).contains("0/0 (100.0%)"));
    }
}
