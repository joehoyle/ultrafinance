//! Shared terminal bars. Workers update counters; one renderer owns stderr.
use std::{
    io::{self, IsTerminal, Write},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub(super) enum Unit {
    Bytes,
    Items,
}

pub(super) struct Counter {
    label: &'static str,
    total: AtomicU64,
    unit: Unit,
    position: AtomicU64,
}
impl Counter {
    pub(super) fn inc(&self, count: u64) {
        let _ = self
            .position
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(
                    value
                        .saturating_add(count)
                        .min(self.total().unwrap_or(u64::MAX)),
                )
            });
    }
    pub(super) fn set(&self, position: u64) {
        self.position.fetch_max(
            position.min(self.total().unwrap_or(u64::MAX)),
            Ordering::Relaxed,
        );
    }
    pub(super) fn total(&self) -> Option<u64> {
        let total = self.total.load(Ordering::Relaxed);
        (total != u64::MAX).then_some(total)
    }
    pub(super) fn set_total(&self, total: u64) {
        let _ = self
            .total
            .compare_exchange(u64::MAX, total, Ordering::Relaxed, Ordering::Relaxed);
    }
    pub(super) fn position(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }
}

pub(super) struct Progress {
    rows: Vec<Arc<Counter>>,
    stop: Option<mpsc::Sender<bool>>,
    worker: Option<JoinHandle<()>>,
}
impl Progress {
    pub(super) fn new(rows: &[(&'static str, Option<u64>, Unit)]) -> Self {
        let rows: Vec<_> = rows
            .iter()
            .map(|&(label, total, unit)| {
                Arc::new(Counter {
                    label,
                    total: AtomicU64::new(total.unwrap_or(u64::MAX)),
                    unit,
                    position: AtomicU64::new(0),
                })
            })
            .collect();
        let (sender, receiver) = mpsc::channel();
        let rendered = rows.clone();
        let worker = thread::spawn(move || {
            let terminal = io::stderr().is_terminal();
            let started = Instant::now();
            let mut last = Instant::now();
            let mut visible = false;
            let mut frame = 0;
            loop {
                let outcome = match receiver.recv_timeout(Duration::from_millis(200)) {
                    Ok(success) => Some(success),
                    Err(mpsc::RecvTimeoutError::Disconnected) => Some(false),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                };
                if terminal
                    || !visible
                    || last.elapsed() >= Duration::from_secs(15)
                    || outcome.is_some()
                {
                    let mut output = io::stderr().lock();
                    if terminal && visible {
                        let _ = write!(output, "\x1b[{}A", rendered.len());
                    }
                    for row in &rendered {
                        let mut message = format_row(row, started.elapsed(), frame, outcome);
                        if terminal {
                            message.truncate(terminal_width().saturating_sub(1));
                        }
                        if terminal {
                            let _ = write!(output, "\r\x1b[2K");
                        }
                        let _ = writeln!(output, "{message}");
                    }
                    let _ = output.flush();
                    visible = true;
                    last = Instant::now();
                }
                frame += 1;
                if outcome.is_some() {
                    break;
                }
            }
        });
        Self {
            rows,
            stop: Some(sender),
            worker: Some(worker),
        }
    }
    pub(super) fn counter(&self, row: usize) -> Arc<Counter> {
        self.rows[row].clone()
    }
    pub(super) fn finish(&mut self) {
        self.stop(true);
    }
    fn stop(&mut self, success: bool) {
        if let Some(sender) = self.stop.take() {
            let _ = sender.send(success);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        self.stop(false);
    }
}

fn quantity(value: f64, unit: Unit) -> String {
    match unit {
        Unit::Items => format!("{value:.0}"),
        Unit::Bytes => {
            for (scale, suffix) in [
                (1099511627776.0, "TiB"),
                (1073741824.0, "GiB"),
                (1048576.0, "MiB"),
                (1024.0, "KiB"),
            ] {
                if value >= scale {
                    return format!("{:.1}{suffix}", value / scale);
                }
            }
            format!("{value:.0}B")
        }
    }
}
fn format_row(row: &Counter, elapsed: Duration, frame: usize, outcome: Option<bool>) -> String {
    let suffix = if outcome == Some(false) {
        " stopped"
    } else {
        ""
    };
    let Some(total) = row.total() else {
        let marker = match outcome {
            Some(true) => "done",
            Some(false) => "stopped",
            None => ["|", "/", "-", "\\"][frame % 4],
        };
        return format!("{marker} {} ({}s)", row.label, elapsed.as_secs());
    };
    let done = if outcome == Some(true) {
        total
    } else {
        row.position().min(total)
    };
    let percent = if total == 0 {
        if outcome == Some(true) { 100 } else { 0 }
    } else {
        ((done as f64 / total as f64) * 100.0).floor() as usize
    };
    let filled = percent * 16 / 100;
    let bar = format!("{}{}", "#".repeat(filled), "-".repeat(16 - filled));
    let rate = done as f64 / elapsed.as_secs_f64().max(0.001);
    let eta = if done == total {
        "0s".to_owned()
    } else if rate > 0.0 {
        let seconds = ((total - done) as f64 / rate).ceil() as u64;
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        "--".to_owned()
    };
    let speed = match row.unit {
        Unit::Bytes => format!("{}/s", quantity(rate, Unit::Bytes)),
        Unit::Items => format!("{rate:.1} items/s"),
    };
    format!(
        "{} [{bar}] {percent:3}% {}/{} {speed} ETA {eta} ({}s){suffix}",
        row.label,
        quantity(done as f64, row.unit),
        quantity(total as f64, row.unit),
        elapsed.as_secs()
    )
}

/// AWS CLI prints cumulative per-command bytes on carriage-return lines. Only
/// parse numbers/units; filenames, URLs and diagnostic text never reach the UI.
pub(super) fn uploaded_bytes(line: &str) -> Option<u64> {
    let value = line.strip_prefix("Completed ")?.split_once('/')?.0;
    let mut fields = value.split_whitespace();
    let amount: f64 = fields.next()?.parse().ok()?;
    if !amount.is_finite() || amount < 0.0 {
        return None;
    }
    let scale = match fields.next()? {
        "Bytes" | "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1048576.0,
        "GiB" => 1073741824.0,
        "TiB" => 1099511627776.0,
        _ => return None,
    };
    Some((amount * scale) as u64)
}

#[cfg(test)]
#[path = "../../tests/unit/infra/progress/tests.rs"]
mod tests;

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
