//! Throttled import progress on stderr; stdout remains machine-readable.
use std::{
    io::{IsTerminal, Write},
    time::{Duration, Instant},
};

pub(crate) struct Progress {
    prefix: &'static str,
    label: &'static str,
    total: usize,
    current: usize,
    started: Instant,
    last: Instant,
    terminal: bool,
    finished: bool,
}
impl Progress {
    pub(crate) fn new(label: &'static str, total: usize) -> Self {
        Self::with_prefix("Import", label, total)
    }
    pub(crate) fn dedupe(label: &'static str, total: usize) -> Self {
        Self::with_prefix("Dedupe", label, total)
    }
    fn with_prefix(prefix: &'static str, label: &'static str, total: usize) -> Self {
        let now = Instant::now();
        let mut progress = Self {
            prefix,
            label,
            total,
            current: 0,
            started: now,
            last: now,
            terminal: std::io::stderr().is_terminal(),
            finished: false,
        };
        progress.render(total == 0);
        progress
    }
    pub(crate) fn advance(&mut self, current: usize) {
        self.current = current.min(self.total);
        let interval = if self.terminal {
            Duration::from_millis(150)
        } else {
            Duration::from_secs(2)
        };
        if self.current == self.total || self.last.elapsed() >= interval {
            self.render(self.current == self.total);
        }
    }
    pub(crate) fn finish(mut self) {
        if !self.finished {
            self.current = self.total;
            self.render(true);
        }
    }
    fn render(&mut self, complete: bool) {
        let percent = self
            .current
            .saturating_mul(100)
            .checked_div(self.total)
            .unwrap_or(100);
        let mut stderr = std::io::stderr().lock();
        let prefix = if self.terminal { "\r\x1b[2K" } else { "" };
        let end = if !self.terminal || complete { "\n" } else { "" };
        let _ = write!(
            stderr,
            "{prefix}{}: {} {}/{} ({percent}%) · {}s{end}",
            self.prefix,
            self.label,
            self.current,
            self.total,
            self.started.elapsed().as_secs()
        );
        let _ = stderr.flush();
        self.last = Instant::now();
        self.finished = complete;
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        if !self.finished {
            let prefix = if self.terminal { "\r\x1b[2K" } else { "" };
            eprintln!(
                "{prefix}{}: {} stopped at {}/{}",
                self.prefix, self.label, self.current, self.total
            );
        }
    }
}
