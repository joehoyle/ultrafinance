use super::*;
#[test]
fn aws_upload_numbers_are_parsed_without_names_or_urls() {
    assert_eq!(
        uploaded_bytes("Completed 256.0 KiB/1.0 GiB (5 MiB/s) with file(s) remaining"),
        Some(262144)
    );
    assert_eq!(uploaded_bytes("Completed 1.2 MiB/2.0 MiB"), Some(1258291));
    assert!(uploaded_bytes("upload: SECRET_FILE to SECRET_URL").is_none());
    assert!(uploaded_bytes("Completed NaN MiB/1.0 GiB").is_none());
}
#[test]
fn counters_do_not_regress_or_exceed_total_and_failure_does_not_fill_bar() {
    let counter = Counter {
        label: "Upload",
        total: AtomicU64::new(100),
        unit: Unit::Bytes,
        position: AtomicU64::new(0),
    };
    counter.set(40);
    counter.set(20);
    assert_eq!(counter.position(), 40);
    let line = format_row(&counter, Duration::from_secs(2), 0, Some(false));
    assert!(line.contains(" 40%"));
    assert!(line.contains("stopped"));
    counter.inc(1000);
    assert_eq!(counter.position(), 100);
}
