//! Show measured archive growth without exposing pg_dump's raw diagnostics.
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, IsTerminal, Read, Write},
    path::Path,
    time::{Duration, Instant},
};

pub(super) struct DumpProgress<'a> {
    archive: &'a Path,
    diagnostics: File,
    pending: Vec<u8>,
    completed: HashSet<u32>,
    tables_started: usize,
    started: Instant,
    last_update: Instant,
    terminal: bool,
    visible: bool,
}

impl<'a> DumpProgress<'a> {
    pub(super) fn new(archive: &'a Path, diagnostics: File) -> Self {
        let started = Instant::now();
        Self {
            archive,
            diagnostics,
            pending: Vec::new(),
            completed: HashSet::new(),
            tables_started: 0,
            started,
            last_update: started,
            terminal: io::stderr().is_terminal(),
            visible: false,
        }
    }

    pub(super) fn update(&mut self) {
        let interval = if self.terminal { 1 } else { 15 };
        if self.last_update.elapsed() >= Duration::from_secs(interval) {
            self.render(None);
        }
    }

    pub(super) fn finish(&mut self, success: bool) {
        self.render(Some(success));
    }

    fn render(&mut self, finished: Option<bool>) {
        // File reads at EOF resume as the worker processes append diagnostics.
        // Keep partial lines so an update cannot miscount a split message.
        let _ = self.diagnostics.read_to_end(&mut self.pending);
        self.tables_started += consume_completions(&mut self.pending, &mut self.completed);
        let bytes = archive_bytes(self.archive);
        let elapsed = self.started.elapsed();
        let mib = bytes as f64 / (1024.0 * 1024.0);
        let rate = mib / elapsed.as_secs_f64().max(0.001);
        let label = match finished {
            Some(true) => "Dump complete",
            Some(false) => "Dump failed",
            None => "Dumping",
        };
        // Serial dumps do not emit parallel worker completion messages.
        // After successful exit, every table that started has completed.
        let tables = if finished == Some(true) {
            self.completed.len().max(self.tables_started)
        } else {
            self.completed.len()
        };
        let message = format!(
            "{label}: {}s | {mib:.1} MiB written | {rate:.1} MiB/s avg | {tables} tables complete",
            elapsed.as_secs(),
        );
        let mut output = io::stderr().lock();
        if self.terminal {
            let _ = write!(output, "\r\x1b[2K{message}");
            self.visible = true;
            if finished.is_some() {
                let _ = writeln!(output);
                self.visible = false;
            }
        } else {
            let _ = writeln!(output, "{message}");
        }
        let _ = output.flush();
        self.last_update = Instant::now();
    }
}

impl Drop for DumpProgress<'_> {
    fn drop(&mut self) {
        // Keep cancellation/error messages off the active progress line.
        if self.visible {
            let _ = writeln!(io::stderr());
        }
    }
}

fn archive_bytes(archive: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(archive) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            Some(entry.metadata().ok()?.len())
        })
        .sum()
}

fn consume_completions(pending: &mut Vec<u8>, completed: &mut HashSet<u32>) -> usize {
    let Some(end) = pending.iter().rposition(|byte| *byte == b'\n') else {
        return 0;
    };
    let mut started = 0;
    for line in pending[..=end].split(|byte| *byte == b'\n') {
        let text = String::from_utf8_lossy(line);
        if text.starts_with("pg_dump: dumping contents of table ") {
            started += 1;
        }
        let Some(rest) = text.strip_prefix("pg_dump: finished item ") else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let Some(id) = fields.next().and_then(|id| id.parse().ok()) else {
            continue;
        };
        if fields.next() == Some("TABLE") && fields.next() == Some("DATA") {
            completed.insert(id);
        }
    }
    pending.drain(..=end);
    started
}

#[cfg(test)]
#[path = "../../tests/unit/infra/dump_progress/tests.rs"]
mod tests;
