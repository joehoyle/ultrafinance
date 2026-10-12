//! Shared metadata capture for Cargo builds and the Docker release wrapper.
use std::{
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct BuildMetadata {
    pub tag: Option<String>,
    pub revision: Option<String>,
    pub dirty: bool,
    pub time: String,
}

pub fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
impl BuildMetadata {
    pub fn capture(root: &Path) -> Self {
        Self {
            tag: git(root, &["describe", "--tags", "--exact-match", "HEAD"])
                .filter(|s| !s.is_empty()),
            revision: git(root, &["rev-parse", "--short=12", "HEAD"]).filter(|s| !s.is_empty()),
            dirty: git(root, &["status", "--porcelain", "--untracked-files=normal"])
                .is_some_and(|s| !s.is_empty()),
            time: utc_time(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock is before Unix epoch")
                    .as_secs(),
            ),
        }
    }
    pub fn version(&self, package: &str) -> String {
        if !self.dirty
            && let Some(tag) = &self.tag
        {
            return tag.clone();
        }
        let revision = self
            .revision
            .as_deref()
            .map(|r| format!("; git {r}"))
            .unwrap_or_default();
        let dirty = if self.dirty { "-dirty" } else { "" };
        format!("{package}-dev (built {}{revision}{dirty})", self.time)
    }
}

/// Gregorian UTC date from Unix seconds, without a platform-specific `date` command.
fn utc_time(seconds: u64) -> String {
    let days = (seconds / 86400) as i64 + 719468;
    let era = days / 146097;
    let day_of_era = days - era * 146097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let time = seconds % 86400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

#[cfg(test)]
#[path = "../tests/unit/build_metadata/tests.rs"]
mod tests;
