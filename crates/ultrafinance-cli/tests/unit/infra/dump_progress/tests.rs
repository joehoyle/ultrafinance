use super::*;

#[test]
fn completion_counts_require_finished_table_data_and_handle_partial_lines() {
    let mut pending = b"pg_dump: dumping contents of table secret\npg_dump: finished item 1 SEQUENCE SET secret\npg_dump: finished item 2 TABLE DA".to_vec();
    let mut completed = HashSet::new();
    assert_eq!(consume_completions(&mut pending, &mut completed), 1);
    assert!(completed.is_empty());
    assert_eq!(pending, b"pg_dump: finished item 2 TABLE DA");
    pending.extend_from_slice(b"TA secret\npg_dump: finished item 2 TABLE DATA secret\npg_dump: finished item 3 TABLE DATA another\n");
    consume_completions(&mut pending, &mut completed);
    assert_eq!(completed.len(), 2);
    assert!(pending.is_empty());
}

#[test]
fn archive_size_counts_only_existing_regular_files() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(archive_bytes(&directory.path().join("absent")), 0);
    fs::write(directory.path().join("toc.dat"), b"toc").unwrap();
    fs::write(directory.path().join("123.dat.gz"), [0; 1024]).unwrap();
    fs::create_dir(directory.path().join("nested")).unwrap();
    fs::write(directory.path().join("nested/ignored"), [0; 2000]).unwrap();
    assert_eq!(archive_bytes(directory.path()), 1027);
}
