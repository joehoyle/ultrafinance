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
