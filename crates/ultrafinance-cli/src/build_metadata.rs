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
mod tests {
    use super::*;
    #[test]
    fn utc_dates_cover_epoch_leap_day_and_year_boundary() {
        assert_eq!(utc_time(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_time(951_827_696), "2000-02-29T12:34:56Z");
        assert_eq!(utc_time(1_767_225_599), "2025-12-31T23:59:59Z");
    }
    #[test]
    fn capture_distinguishes_exact_tag_dirty_checkout_and_later_commit() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .current_dir(root)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&["init", "--quiet"]);
        std::fs::write(root.join("source.txt"), "first").unwrap();
        run(&["add", "source.txt"]);
        run(&[
            "-c",
            "user.name=Build test",
            "-c",
            "user.email=build-test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "initial",
        ]);
        run(&["tag", "v1.2.3"]);
        let release = BuildMetadata::capture(root);
        assert_eq!(release.tag.as_deref(), Some("v1.2.3"));
        assert!(!release.dirty);
        assert_eq!(release.revision.as_ref().unwrap().len(), 12);
        std::fs::write(root.join("source.txt"), "second").unwrap();
        let dirty = BuildMetadata::capture(root);
        assert!(dirty.dirty);
        assert!(dirty.version("1.2.3").contains("built "));
        run(&["add", "source.txt"]);
        run(&[
            "-c",
            "user.name=Build test",
            "-c",
            "user.email=build-test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "next",
        ]);
        let development = BuildMetadata::capture(root);
        assert!(development.tag.is_none());
        assert!(!development.dirty);
        assert_ne!(development.revision, release.revision);
    }
    #[test]
    fn only_clean_tagged_builds_use_a_release_version() {
        let mut metadata = BuildMetadata {
            tag: Some("v1.2.3".into()),
            revision: Some("abc123".into()),
            dirty: false,
            time: "2026-10-08T21:00:00Z".into(),
        };
        assert_eq!(metadata.version("1.2.3"), "v1.2.3");
        metadata.dirty = true;
        assert_eq!(
            metadata.version("1.2.3"),
            "1.2.3-dev (built 2026-10-08T21:00:00Z; git abc123-dirty)"
        );
        metadata.dirty = false;
        metadata.tag = None;
        assert_eq!(
            metadata.version("1.2.3"),
            "1.2.3-dev (built 2026-10-08T21:00:00Z; git abc123)"
        );
        metadata.revision = None;
        assert_eq!(
            metadata.version("1.2.3"),
            "1.2.3-dev (built 2026-10-08T21:00:00Z)"
        );
    }
}
