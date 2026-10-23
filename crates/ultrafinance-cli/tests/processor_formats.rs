use serde_json::{Value, json};
use std::process::Command;
use ultrafinance_core::store::MerchantStore;

#[test]
fn researched_formats_import_preview_and_enrichment_need_no_provider() {
    let store = MerchantStore::temporary().unwrap();
    store.put(&serde_json::from_value(json!({"id":"test-processor","name":"Example Processor","type":"payment_processor"})).unwrap()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("formats.json");
    let format = json!({
        "id":"example-star", "processor":"Example Processor", "pattern":"(?i:NEX)\\* *(?P<merchant>\\S(?:.*\\S)?)",
        "sources":["https://example.test/docs"], "reviewed_on":"2026-10-19", "scope":"Test fixture"
    });
    std::fs::write(&file, serde_json::to_vec(&json!([format])).unwrap()).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ultrafinance"))
            .args(args)
            .env("ULTRAFINANCE_DATABASE_URL", store.temporary_url())
            .env_remove("TYPESAFE_API_KEY")
            .env_remove("ULTRAFINANCE_MERCHANTS")
            .env_remove("ULTRAFINANCE_DISCOVERY_URL")
            .env_remove("ULTRAFINANCE_DISCOVERY_API_KEY")
            .output()
            .unwrap()
    };
    let preview = run(&[
        "processor-formats",
        "import",
        file.to_str().unwrap(),
        "--dry-run",
    ]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert!(store.processor_formats().unwrap().is_empty());
    let imported = run(&["processor-formats", "import", file.to_str().unwrap()]);
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let listed: Value =
        serde_json::from_slice(&run(&["processor-formats", "list"]).stdout).unwrap();
    assert_eq!(listed[0]["processor_id"], "test-processor");
    assert!(listed[0].get("sources").is_none());
    assert_eq!(
        store.processor_formats().unwrap()[0].sources,
        vec!["https://example.test/docs"]
    );
    assert!(listed[0].get("positive_examples").is_none());
    assert!(listed[0].get("negative_examples").is_none());
    let matched: Value =
        serde_json::from_slice(&run(&["processor-formats", "match", "NEX*Alpha Cafe"]).stdout)
            .unwrap();
    assert_eq!(matched[0]["capture"]["merchant"], "Alpha Cafe");
    let unknown: Value =
        serde_json::from_slice(&run(&["processor-formats", "match", "NEX Alpha Cafe"]).stdout)
            .unwrap();
    assert!(unknown.as_array().unwrap().is_empty());
    let enriched = run(&["enrich", "NEX*Alpha Cafe", "--details", "--json"]);
    assert!(
        enriched.status.success(),
        "{}",
        String::from_utf8_lossy(&enriched.stderr)
    );
    let response: Value = serde_json::from_slice(&enriched.stdout).unwrap();
    assert_eq!(response["counterparties"][0]["id"], "test-processor");
    assert_eq!(response["merchant"]["status"], "unresolved");
    let details = String::from_utf8(enriched.stderr).unwrap();
    assert!(details.contains("Matched stored format: example-star"));
    assert!(details.contains("No Jev processor request."));
    assert!(details.contains("Merchant lookup: Alpha Cafe"));
    let mut invalid = format;
    invalid["pattern"] = json!("[");
    std::fs::write(&file, serde_json::to_vec(&json!([invalid])).unwrap()).unwrap();
    assert!(
        !run(&["processor-formats", "import", file.to_str().unwrap()])
            .status
            .success()
    );
    assert_eq!(store.processor_formats().unwrap().len(), 1);
}
