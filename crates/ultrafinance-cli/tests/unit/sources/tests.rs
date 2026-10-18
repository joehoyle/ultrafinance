use super::*;
#[test]
fn rolling_year_handles_leap_days_and_calendar_boundaries() {
    for (epoch, start, end) in [
        (1709164800, "2023-02-28", "2024-02-29"),
        (1767225600, "2025-01-01", "2026-01-01"),
        (1791504000, "2025-10-09", "2026-10-09"),
    ] {
        assert_eq!(
            last_year(std::time::UNIX_EPOCH + Duration::from_secs(epoch)),
            (start.into(), end.into())
        );
    }
}
#[test]
fn regions_cannot_escape_cache_or_change_upstream_path() {
    for region in ["../us", "US", "us/../../", "", "global?x=1"] {
        assert!(urls(Source::OpenEnrichment, region).is_err());
    }
    assert!(
        urls(Source::OpenEnrichment, "uk").unwrap()[0]
            .1
            .contains("/uk/merchants.csv")
    );
    assert!(urls(Source::MerchantStudio, "us").is_err());
}
#[test]
fn raw_csv_preserves_fields_and_paginates() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("input.csv");
    std::fs::write(
        &file,
        "name,description\nAlpha,\"PAYMENT $12, STORE 2\"\nBeta,OTHER\n",
    )
    .unwrap();
    assert_eq!(
        raw_rows(&file, false, 1, 0).unwrap()["rows"][0]["description"],
        "PAYMENT $12, STORE 2"
    );
    assert_eq!(
        raw_rows(&file, false, 1, 1).unwrap()["rows"][0]["name"],
        "Beta"
    );
}
#[test]
fn snapshot_corruption_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("input.csv"), "changed").unwrap();
    std::fs::write(dir.path().join("download.json"), serde_json::to_vec(&json!({"files":[{"file":"input.csv", "fingerprint": ultrafinance_core::eval::fingerprint(b"original")}]})).unwrap()).unwrap();
    assert!(snapshot_files(dir.path()).is_err());
}
#[test]
fn snapshots_are_reused_and_invalid_updates_leave_latest_intact() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(DatasetSource::Dodatathings);
    let files = || {
        vec![(
            "input.csv",
            "https://example.test/source.csv".into(),
            b"description,category\nPAYMENT,Other\n".to_vec(),
        )]
    };
    let first = save_snapshot(root.path(), &r, "global", files()).unwrap();
    assert_eq!(
        save_snapshot(root.path(), &r, "global", files()).unwrap(),
        first
    );
    assert!(
        save_snapshot(
            root.path(),
            &r,
            "global",
            vec![(
                "input.csv",
                "https://example.test/source.csv".into(),
                b"<html>Error</html>".to_vec()
            )]
        )
        .is_err()
    );
    assert_eq!(latest(root.path(), r.source, "global").unwrap(), first);
    assert!(snapshot_files(&first).is_ok());
}
#[tokio::test]
async fn downloader_rejects_http_errors_empty_and_oversized_responses() {
    use std::io::Read;
    for response in [
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 67108865\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok",
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/data", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buf = [0; 4096];
            let read = stream.read(&mut buf).unwrap();
            assert!(read > 0);
            stream.write_all(response.as_bytes()).unwrap();
        });
        let result = fetch(
            &reqwest::Client::builder().no_proxy().build().unwrap(),
            &url,
        )
        .await;
        if response.ends_with("ok") {
            assert_eq!(result.unwrap(), b"ok");
        } else {
            assert!(result.is_err());
        }
        server.join().unwrap();
    }
}

#[test]
fn foursquare_snapshots_skip_content_scans_and_keep_size_checks() -> Result<()> {
    let root = tempfile::tempdir()?;
    let input = root.path().join("download.csv");
    // A malformed tail is left for import parsing, not a separate snapshot scan.
    std::fs::write(&input, "fsq_place_id,name\na,Cafe\nb,Too,many,fields\n")?;
    let r = registry(DatasetSource::Foursquare);
    let snapshot =
        save_foursquare_snapshot(root.path(), &r, "ca", &input, "https://example.test".into())?;
    let (cached, _) = snapshot_files(&snapshot)?;
    let mut metadata: Value =
        serde_json::from_slice(&std::fs::read(snapshot.join("download.json"))?)?;
    assert!(metadata["files"][0]["fingerprint"].is_null());
    // Legacy fingerprints also do not cause a complete scan on cached imports.
    metadata["files"][0]["fingerprint"] = json!("legacy-content-fingerprint");
    std::fs::write(
        snapshot.join("download.json"),
        serde_json::to_vec(&metadata)?,
    )?;
    assert!(snapshot_files(&snapshot).is_ok());
    std::fs::write(cached, "shortened")?;
    assert!(snapshot_files(&snapshot).is_err());
    Ok(())
}
