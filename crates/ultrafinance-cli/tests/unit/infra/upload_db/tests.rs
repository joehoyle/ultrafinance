use super::*;

fn options() -> Options {
    Options {
        bucket: Some("private-transfer".into()),
        image: None,
        latest: false,
        postgres_bin: None,
        resume: None,
        check_only: false,
        dump_jobs: None,
        restore_jobs: None,
        transfer_jobs: 16,
    }
}

#[test]
fn dump_is_parallel_compressed_and_uses_only_the_local_database() {
    let tools = PgTools {
        directory: PathBuf::from("/fixture/postgresql17"),
    };
    let command = dump_command(&tools, Path::new("archive"), 12, "zstd:1");
    assert_eq!(command.get_program(), "/fixture/postgresql17/pg_dump");
    let args: Vec<_> = command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    for arg in [
        ultrafinance_core::store::LOCAL_DATABASE_URL,
        "--format=directory",
        "--compress=zstd:1",
        "--no-owner",
        "--no-privileges",
        "12",
        "archive",
    ] {
        assert!(args.iter().any(|a| a == arg), "missing {arg}");
    }
    assert!(
        command
            .get_envs()
            .any(|(k, v)| k == "PGPASSWORD" && v.is_none())
    );
}

#[test]
fn archive_requires_toc_and_rejects_paths_that_could_escape_the_manifest() {
    let directory = tempfile::tempdir().unwrap();
    assert!(archive_files(directory.path()).is_err());
    fs::write(directory.path().join("toc.dat"), b"toc").unwrap();
    fs::write(directory.path().join("123.dat.zst"), b"data").unwrap();
    assert_eq!(archive_files(directory.path()).unwrap().len(), 2);
    fs::write(directory.path().join("bad\tname"), b"bad").unwrap();
    assert!(archive_files(directory.path()).is_err());
}

#[test]
fn parallel_transfer_checks_every_file_and_propagates_failure() {
    let items: Vec<_> = (0..40).map(|n| PathBuf::from(n.to_string())).collect();
    let mut values = parallel(&items, 4, |p| Ok(p.to_owned())).unwrap();
    values.sort();
    let mut expected = items.clone();
    expected.sort();
    assert_eq!(values, expected);
    assert!(
        parallel(&items, 4, |p| {
            if p == Path::new("5") {
                bail!("transfer failed");
            }
            Ok(())
        })
        .is_err()
    );
}

#[test]
fn checksum_matches_sha256_and_shell_quoting_prevents_expansion() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("data");
    fs::write(&file, b"abc").unwrap();
    assert_eq!(
        checksum(&file).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let value = "a'$(false)`false`\nnext";
    let output = Command::new("/bin/bash")
        .args(["-c", &format!("printf %s {}", quote(value))])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), value);
}

#[test]
fn generated_remote_script_is_valid_bash_and_does_not_put_secrets_in_arguments() {
    let script = remote_script(
        "0123456789abcdef",
        "toc.dat\thash\t100\thttps://example.invalid/file\n",
        &options(),
    );
    let mut child = Command::new("/bin/bash")
        .arg("-n")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
    assert!(script.contains("export PGPASSWORD="));
    assert!(!script.contains("--password="));
    assert!(script.contains("--no-owner --no-privileges --exit-on-error"));
    assert!(!script.contains("--single-transaction"));
    let validation = script
        .find("ULTRAFINANCE_DATABASE_NAME=\"$staging\"")
        .unwrap();
    let cutover = script.find("step cutover").unwrap();
    let drop_old = script.find("DROP DATABASE :\"old\" WITH (FORCE)").unwrap();
    assert!(validation < cutover && cutover < drop_old);
    assert!(script.contains("ALTER DATABASE :\"database\" ALLOW_CONNECTIONS false;"));
    assert!(script.contains("trap cleanup EXIT"));
    let preflight = script
        .split("step preflight")
        .nth(1)
        .unwrap()
        .split("step download")
        .next()
        .unwrap();
    assert!(preflight.contains("-c 'SELECT 1'"));
    assert!(!preflight.contains("database check"));
}

#[test]
fn preflight_failure_reports_the_specific_error_before_the_bootstrap_wrapper() {
    let message =
        connection_failure("UF_UPLOAD_ERROR:restore-capacity\nUF_UPLOAD_ERROR:remote-script\n");
    assert!(message.contains("--restore-jobs"));
}

#[test]
fn ecs_monitor_requires_remote_success_marker_and_successful_transport_exit() {
    for (script, expected) in [
        ("printf 'UF_UPLOAD_SUCCESS_test\\n'", true),
        ("printf 'UF_UPLOAD_FAILED\\n'", false),
        ("printf 'UF_UPLOAD_SUCCESS_other\\n'", false),
        ("printf 'UF_UPLOAD_SUCCESS_test\\n'; exit 1", false),
    ] {
        let outcome = monitor(
            Command::new("/bin/bash").args(["-c", script]),
            "UF_UPLOAD_SUCCESS_test",
            "test",
            || Ok(()),
        );
        assert_eq!(outcome.is_ok(), expected, "{script}");
    }
}

#[cfg(unix)]
#[test]
fn postgres_discovery_skips_incompatible_versions_and_pins_both_clients() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let mut directories = Vec::new();
    for (name, dump_major, psql_major) in [
        ("old", 14, 14),
        ("new", 18, 18),
        ("mixed", 17, 18),
        ("valid clients", 17, 17),
    ] {
        let directory = root.path().join(name);
        fs::create_dir(&directory).unwrap();
        for (program, version) in [("pg_dump", dump_major), ("psql", psql_major)] {
            let path = directory.join(program);
            fs::write(
                &path,
                format!("#!/bin/sh\nprintf '{program} (PostgreSQL) {version}.1\\n'\n"),
            )
            .unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        directories.push(directory);
    }
    assert!(PgTools::find(directories[..3].iter().cloned()).is_none());
    let tools = PgTools::find(directories.clone()).unwrap();
    assert_eq!(tools.directory, directories[3]);
    assert_eq!(
        tools.command("psql").get_program(),
        directories[3].join("psql")
    );
    assert!(PgTools::resolve(Some(&directories[0])).is_err());
    assert_eq!(
        PgTools::resolve(Some(&directories[3])).unwrap().directory,
        directories[3]
    );
}

#[test]
fn compression_falls_back_only_for_missing_build_support() {
    let mut attempts = Vec::new();
    let chosen = choose_compression(|spec| {
        attempts.push(spec.to_owned());
        Ok((spec == "zstd:1").then(|| "pg_dump: error: invalid compression specification: this build does not support compression with ZSTD".to_owned()))
    }).unwrap();
    assert_eq!(chosen, "gzip:1");
    assert_eq!(attempts, ["zstd:1", "gzip:1"]);
    assert_eq!(choose_compression(|_| Ok(None)).unwrap(), "zstd:1");
    assert_eq!(
        choose_compression(|spec| Ok(
            (spec != "none").then(|| "this build does not support compression with GZIP".into())
        ))
        .unwrap(),
        "none"
    );
    let mut attempts = 0;
    let error = choose_compression(|_| {
        attempts += 1;
        Ok(Some(
            "no space left on device PRIVATE_DATABASE_VALUE".into(),
        ))
    })
    .unwrap_err()
    .to_string();
    assert_eq!(attempts, 1);
    assert!(error.contains("temporary disk space"));
    assert!(!error.contains("PRIVATE_DATABASE_VALUE"));
}

#[test]
fn failed_dump_explains_the_cause_without_echoing_raw_diagnostics() {
    let error = run_dump(
        Command::new("/bin/bash").args([
            "-c",
            "printf '%s\\n' 'pg_dump: no space left on device PRIVATE_DATABASE_VALUE' >&2; exit 1",
        ]),
        Path::new("/nonexistent-upload-test-archive"),
        || Ok(()),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("temporary disk space"));
    assert!(!error.contains("PRIVATE_DATABASE_VALUE"));
    run_dump(
        Command::new("/bin/bash").args(["-c", "exit 0"]),
        Path::new("/nonexistent-upload-test-archive"),
        || Ok(()),
    )
    .unwrap();
}

#[test]
fn upload_progress_requires_success_before_counting_the_last_byte() {
    let mut bars = Progress::new(&[("Upload", Some(1024), Unit::Bytes)]);
    let counter = bars.counter(0);
    let result = run_upload(
        Command::new("/bin/bash").args([
            "-c",
            "printf 'Completed 1.0 KiB/1.0 KiB\\r'; sleep 0.2; exit 1",
        ]),
        1024,
        &counter,
        || Ok(()),
    );
    assert!(result.is_err());
    assert_eq!(counter.position(), 1023);
    run_upload(
        Command::new("/bin/bash").args(["-c", "exit 0"]),
        1,
        &counter,
        || Ok(()),
    )
    .unwrap();
    assert_eq!(counter.position(), 1024);
    bars.finish();
}

#[test]
fn remote_numeric_progress_rejects_invalid_and_sensitive_fields() {
    assert!(remote_progress("UF_UPLOAD_PROGRESS:download:20:100:bytes").is_some());
    assert!(remote_progress("UF_UPLOAD_PROGRESS:restore:1:10:items").is_some());
    for line in [
        "UF_UPLOAD_PROGRESS:download:101:100:bytes",
        "UF_UPLOAD_PROGRESS:download:NaN:100:bytes",
        "UF_UPLOAD_PROGRESS:download:1:100:items",
        "UF_UPLOAD_PROGRESS:restore:1:10:items:PRIVATE_DATA",
        "UF_UPLOAD_PROGRESS:SECRET:1:10:items",
    ] {
        assert!(remote_progress(line).is_none(), "{line}");
    }
}

#[test]
fn resume_reads_legacy_manifest_as_data_without_executing_script() {
    let hash = "a".repeat(64);
    let sizes = std::collections::HashMap::from([("toc.dat".to_owned(), 100)]);
    let script = format!(
        "exit 99\ncat > downloads <<'UF_DOWNLOADS'\ntoc.dat\t{hash}\thttps://private.invalid\nUF_DOWNLOADS\nmalicious command\n"
    );
    let entries = manifest_entries(legacy_manifest(&script).unwrap(), Some(&sizes)).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].size, 100);
    let current = format!("toc.dat\t{hash}\t100\thttps://expired.invalid\n");
    assert_eq!(manifest_entries(&current, None).unwrap()[0].size, 100);
    assert!(manifest_entries(legacy_manifest(&script).unwrap(), None).is_err());
    assert!(
        manifest_entries(
            &format!("toc.dat\t{hash}\t100\turl\ntoc.dat\t{hash}\t100\turl\n"),
            None
        )
        .is_err()
    );
    assert!(manifest_entries(&format!("../escape\t{hash}\t100\turl\n"), None).is_err());
    assert!(manifest_entries("toc.dat\tinvalid_hash\t100\turl", None).is_err());
}

#[test]
fn upload_image_selection_supports_live_latest_and_explicit_digest() {
    let mut args = options();
    assert!(matches!(args.image_selection(), ImageSelection::Live));
    args.latest = true;
    assert!(matches!(args.image_selection(), ImageSelection::Latest));
    args.latest = false;
    args.image = Some("example@sha256:digest".to_owned());
    assert!(matches!(
        args.image_selection(),
        ImageSelection::Digest("example@sha256:digest")
    ));
}
