//! Throttled import progress on stderr; stdout remains machine-readable.
use std::{
    cell::RefCell,
    io::{IsTerminal, Write},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

static VERBOSE: AtomicBool = AtomicBool::new(false);
/// Enable detailed stage diagnostics instead of the compact import display.
pub fn set_import_verbose(enabled: bool) {
    VERBOSE.store(enabled, Ordering::Relaxed);
}
pub(crate) fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}
thread_local! { static OVERALL: RefCell<Option<Rc<RefCell<Summary>>>> = const { RefCell::new(None) }; }

struct Summary {
    total: usize,
    current: usize,
    locations: usize,
    skipped: usize,
    stage: &'static str,
    started: Instant,
    last: Instant,
    terminal: bool,
}
fn number(n: usize) -> String {
    let digits = n.to_string();
    let mut result = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(c);
    }
    result
}
fn duration(seconds: f64) -> String {
    let seconds = seconds.ceil() as u64;
    if seconds >= 3600 {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}
impl Summary {
    fn render(&mut self, force: bool, complete: bool) {
        let interval = if self.terminal {
            Duration::from_millis(150)
        } else {
            Duration::from_secs(5)
        };
        if !force && !complete && self.last.elapsed() < interval {
            return;
        }
        let elapsed = self.started.elapsed().as_secs_f64();
        let percent = if self.total == 0 {
            100.0
        } else {
            (self.current as f64 * 1000.0 / self.total as f64).floor() / 10.0
        };
        let rate = self.current as f64 / elapsed.max(0.001);
        let timing = if self.current > 0 && self.current < self.total {
            format!(
                " · {}/s · ETA {}",
                number(rate.round() as usize),
                duration((self.total - self.current) as f64 / rate)
            )
        } else {
            String::new()
        };
        let places = if self.locations + self.skipped > 0 {
            format!(
                " · {} {}",
                number(self.locations),
                if self.locations == 1 {
                    "location"
                } else {
                    "locations"
                }
            )
        } else {
            String::new()
        };
        let prefix = if self.terminal { "\r\x1b[2K" } else { "" };
        let end = if !self.terminal || complete { "\n" } else { "" };
        let mut stderr = std::io::stderr().lock();
        let _ = write!(
            stderr,
            "{prefix}Import: {percent:.1}% · {}/{} · {}{timing}{places} · {}{end}",
            number(self.current),
            number(self.total),
            duration(elapsed.floor()),
            self.stage
        );
        let _ = stderr.flush();
        self.last = Instant::now();
    }
}
/// One terminal row owns the complete reconciliation; child stages update its
/// status rather than clearing it or printing their own batch percentages.
pub(crate) struct Overall(Rc<RefCell<Summary>>);
impl Overall {
    pub(crate) fn new(total: usize) -> Self {
        let now = Instant::now();
        let state = Rc::new(RefCell::new(Summary {
            total,
            current: 0,
            locations: 0,
            skipped: 0,
            stage: "Starting",
            started: now,
            last: now,
            terminal: std::io::stderr().is_terminal(),
        }));
        OVERALL.with(|slot| *slot.borrow_mut() = Some(state.clone()));
        state.borrow_mut().render(true, false);
        Self(state)
    }
    pub(crate) fn report(&mut self, current: usize) {
        let mut state = self.0.borrow_mut();
        state.current = current.min(state.total);
        state.stage = "Processing";
        let terminal = state.terminal;
        // Redirected logs get periodic summaries, terminals update each chunk.
        state.render(terminal, false);
    }
}
impl Drop for Overall {
    fn drop(&mut self) {
        OVERALL.with(|slot| *slot.borrow_mut() = None);
        let mut state = self.0.borrow_mut();
        state.stage = if state.current == state.total {
            "Processed; awaiting commit"
        } else {
            "Stopped; uncommitted"
        };
        state.render(true, true);
    }
}
pub(crate) fn locations(processed: usize, skipped: usize) {
    OVERALL.with(|slot| {
        if let Some(state) = slot.borrow().as_ref() {
            let mut state = state.borrow_mut();
            state.locations += processed;
            state.skipped += skipped;
        }
    });
}
fn stage(label: &'static str) -> &'static str {
    match label {
        "looking up existing source identities" => "Finding merchants",
        "matching new merchant identities" => "Matching merchants",
        "inserting new merchants with final fields" => "Saving merchants",
        "checking/writing source records" => "Saving source records",
        "rebuilding merchant search and markets" => "Rebuilding merchants",
        "reading merchant source inputs" => "Reading source inputs",
        "reading outlet countries" => "Reading outlet countries",
        "writing canonical merchant identities" => "Updating merchant identities",
        "writing merchant aliases" => "Updating aliases",
        "writing merchant search index" => "Updating search",
        "processing places for linked locations" => "Preparing locations",
        "writing linked locations" => "Saving locations",
        "reconciling merchant groups" => "Merging merchants",
        "reading prepared records (bytes)" => "Reading bundle",
        _ => label,
    }
}

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
        if self.finished && current == self.current {
            return;
        }
        self.current = current.min(self.total);
        let interval = if self.terminal {
            Duration::from_millis(150)
        } else {
            Duration::from_secs(2)
        };
        let complete = self.total == self.current;
        if complete || self.last.elapsed() >= interval {
            self.render(complete);
        }
    }
    pub(crate) fn finish(mut self) {
        if !self.finished {
            self.current = self.total;
            self.render(true);
        }
    }
    fn render(&mut self, complete: bool) {
        self.finished = complete;
        if self.prefix == "Import" && !verbose() {
            let forwarded = OVERALL.with(|slot| {
                if let Some(state) = slot.borrow().as_ref() {
                    let mut state = state.borrow_mut();
                    let next = stage(self.label);
                    let force = state.terminal && state.stage != next;
                    state.stage = next;
                    state.render(force, false);
                    true
                } else {
                    false
                }
            });
            if forwarded {
                self.last = Instant::now();
                return;
            }
        }
        let elapsed = self.started.elapsed().as_secs_f64();
        let percent = self
            .current
            .saturating_mul(100)
            .checked_div(self.total)
            .unwrap_or(100);
        let count = format!("{}/{} ({percent}%)", self.current, self.total);
        let rate = self.current as f64 / elapsed.max(0.001);
        let eta = if self.current > 0 && self.current < self.total {
            format!(
                " · ETA {:.0}s",
                ((self.total - self.current) as f64 / rate).ceil()
            )
        } else {
            String::new()
        };
        let mut stderr = std::io::stderr().lock();
        let prefix = if self.terminal { "\r\x1b[2K" } else { "" };
        let end = if !self.terminal || complete { "\n" } else { "" };
        let _ = write!(
            stderr,
            "{prefix}{}: {} {count} · {elapsed:.1}s · {rate:.0}/s{eta}{end}",
            self.prefix, self.label,
        );
        let _ = stderr.flush();
        self.last = Instant::now();
        self.finished = complete;
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        if !self.finished {
            if !verbose() && OVERALL.with(|slot| slot.borrow().is_some()) {
                return;
            }
            let prefix = if self.terminal { "\r\x1b[2K" } else { "" };
            eprintln!(
                "{prefix}{}: {} stopped at {}/{}",
                self.prefix, self.label, self.current, self.total
            );
        }
    }
}

/// Track bytes read without an additional scan or growing allocation.
pub(crate) struct Reader<R> {
    inner: R,
    progress: Progress,
    bytes: usize,
}
impl<R: std::io::Read> Reader<R> {
    pub(crate) fn new(inner: R, label: &'static str, bytes: usize) -> Self {
        Self {
            inner,
            progress: Progress::new(label, bytes),
            bytes: 0,
        }
    }
}
impl<R: std::io::Read> std::io::Read for Reader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buffer)?;
        self.bytes += n;
        self.progress.advance(self.bytes);
        Ok(n)
    }
}
