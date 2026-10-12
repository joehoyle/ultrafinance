use super::*;
#[test]
fn legacy_market_evidence_bundles_are_rebuilt_without_overwriting() -> Result<()> {
    let root = std::env::temp_dir().join(format!("ultra-dataset-schema-{}", uuid::Uuid::new_v4()));
    let input = r#"{"schemaVersion":"1.0","merchants":[{"id":"alpha","canonicalName":"Alpha","countryHints":["CA"]}]}"#;
    let examples = r#"{"schemaVersion":"1.0","descriptors":[]}"#;
    let mut legacy = prepare(Source::MerchantStudio, input, Some(examples), "global")?;
    legacy.manifest.adapter_version = 2;
    let legacy_path = legacy.save(&root)?;
    let mut records = serde_json::to_value(&legacy.records)?;
    records[0]["merchant"]["market_evidence"] = json!([]);
    let legacy_bytes = serde_json::to_vec(&records)?;
    std::fs::write(legacy_path.join("knowledge.json"), &legacy_bytes)?;
    assert!(read_selected_records(&legacy_path.join("knowledge.json"), None).is_err());
    assert!(
        cached_bundle(
            &root,
            Source::MerchantStudio,
            input,
            Some(examples),
            "global"
        )?
        .is_none()
    );

    let rebuilt = prepare(Source::MerchantStudio, input, Some(examples), "global")?;
    let path = rebuilt.save(&root)?;
    assert_ne!(path, legacy_path);
    let (records, available) = read_selected_records(&path.join("knowledge.json"), None)?;
    assert_eq!(available, 1);
    assert_eq!(records[0].merchant.name, "Alpha");
    assert_eq!(records[0].raw["countryHints"], json!(["CA"]));
    assert!(records[0].merchant.markets.is_empty());
    assert_eq!(
        std::fs::read(legacy_path.join("knowledge.json"))?,
        legacy_bytes
    );
    assert_eq!(
        cached_bundle(
            &root,
            Source::MerchantStudio,
            input,
            Some(examples),
            "global"
        )?
        .unwrap()
        .0,
        path
    );
    // All adapters share the changed Merchant schema, including the file
    // preparation path used by Foursquare.
    for source in [
        Source::MerchantStudio,
        Source::OpenEnrichment,
        Source::DoDataThings,
        Source::MoneyVis,
        Source::BusinessTransactions,
    ] {
        assert_eq!(adapter_version(source), 3);
    }
    assert_eq!(adapter_version(Source::LunchMoney), 5);
    assert_eq!(adapter_version(Source::Foursquare), 5);
    std::fs::remove_dir_all(root)?;
    Ok(())
}
#[test]
fn cached_preparation_preserves_reviewed_records_and_limited_reads() -> Result<()> {
    let root = std::env::temp_dir().join(format!("ultra-dataset-cache-{}", uuid::Uuid::new_v4()));
    let input = "name,transaction_string,category_label\nAlpha,ALPHA STORE,Dining\nBeta,BETA STORE,Dining\n";
    assert!(cached_bundle(&root, Source::BusinessTransactions, input, None, "global")?.is_none());
    let bundle = prepare(Source::BusinessTransactions, input, None, "global")?;
    let path = bundle.save(&root)?;
    let mut records = bundle.records;
    records[0].merchant.name = "Reviewed Alpha".into();
    std::fs::write(path.join("knowledge.json"), serde_json::to_vec(&records)?)?;
    let (cached_path, _) =
        cached_bundle(&root, Source::BusinessTransactions, input, None, "global")?.unwrap();
    assert_eq!(cached_path, path);
    let (selected, total) = read_selected_records(&path.join("knowledge.json"), Some(1))?;
    assert_eq!(total, 2);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].merchant.name, "Reviewed Alpha");
    assert_eq!(
        read_selected_records(&path.join("knowledge.json"), None)?
            .0
            .len(),
        2
    );
    assert!(
        cached_bundle(
            &root,
            Source::BusinessTransactions,
            &format!("{input}Gamma,GAMMA,Dining\n"),
            None,
            "global"
        )?
        .is_none()
    );
    assert!(cached_bundle(&root, Source::BusinessTransactions, input, None, "ca")?.is_none());
    std::fs::write(
        path.join("knowledge.json"),
        format!("{} trailing", serde_json::to_string(&records)?),
    )?;
    assert!(read_selected_records(&path.join("knowledge.json"), Some(1)).is_err());
    std::fs::remove_dir_all(root)?;
    Ok(())
}
#[test]
fn business_transactions_isolates_generated_evidence_and_has_stable_name_ids() {
    let input = "name,transaction_string,category_label\nCafe Azul,CAFE AZUL STORE 0006 PHOENIX AZ $15.00,Dining\ncafe azul,CAFE AZUL PURCHASE 123,Dining\n";
    let bundle = prepare(Source::BusinessTransactions, input, None, "global").unwrap();
    assert_eq!(bundle.records.len(), 1);
    let record = &bundle.records[0];
    assert_eq!(record.merchant.name, "Cafe Azul");
    assert!(record.merchant.aliases.is_empty());
    assert!(record.merchant.markets.is_empty());
    assert!(record.merchant.website.is_none());
    assert!(record.raw.get("transaction_string").is_none());
    assert!(record.raw.get("category_label").is_none());
    assert!(
        !serde_json::to_string(&record.raw)
            .unwrap()
            .contains("PHOENIX")
    );
    assert_eq!(
        bundle.manifest.evaluation_kind,
        "synthetic-source-consistency"
    );
    for case in bundle.development.iter().chain(&bundle.holdout) {
        assert!(case.expected.is_some());
        assert_eq!(case.label_origin, "source-synthetic-generated-merchant");
        assert!(case.request.country.is_none());
        assert!(case.request.location.is_none());
        assert!(case.request.amount.is_none());
        assert!(case.request.extra.is_empty());
    }
    let reordered = "name,transaction_string,category_label\ncafe azul,CAFE AZUL PURCHASE 123,Dining\nCafe Azul,CAFE AZUL STORE 0006 PHOENIX AZ $15.00,Dining\n";
    let other = prepare(Source::BusinessTransactions, reordered, None, "global").unwrap();
    assert_eq!(other.records[0].external_id, record.external_id);
    assert_eq!(other.records[0].merchant.name, record.merchant.name);
    assert_eq!(
        serde_json::to_value(other.development).unwrap(),
        serde_json::to_value(bundle.development).unwrap()
    );
    assert_eq!(
        serde_json::to_value(other.holdout).unwrap(),
        serde_json::to_value(bundle.holdout).unwrap()
    );
}
#[test]
fn business_transactions_rejects_conflicting_labels_and_blank_names() {
    let input =
        "name,transaction_string\nCafe Azul,SAME TRANSACTION\nCafe Verde,SAME TRANSACTION\n";
    assert!(
        prepare(Source::BusinessTransactions, input, None, "global")
            .err()
            .unwrap()
            .to_string()
            .contains("conflicting merchant labels")
    );
    assert!(
        prepare(
            Source::BusinessTransactions,
            "name,transaction_string\n,CAFE PURCHASE\n",
            None,
            "global"
        )
        .is_err()
    );
}
#[test]
fn category_labels_do_not_turn_into_merchant_labels_and_splits_are_stable() {
    let input = "description,category\nADIDAS,Shopping\nPAYMENT FROM ADIDAS,Shopping\nNETFLIX,Subscription\n";
    let a = prepare(Source::DoDataThings, input, None, "global").unwrap();
    let b = prepare(Source::DoDataThings, "description,category\nNETFLIX,Subscription\nPAYMENT FROM ADIDAS,Shopping\nADIDAS,Shopping\n", None, "global").unwrap();
    assert_eq!(
        serde_json::to_value(&a.holdout).unwrap(),
        serde_json::to_value(&b.holdout).unwrap()
    );
    assert_eq!(
        a.manifest.labeled_development + a.manifest.labeled_holdout,
        0
    );
    assert!(a.records.is_empty());
}
#[test]
fn lunchmoney_uses_only_bank_originals_without_merchant_labels() {
    let input = json!({"transactions": [
        {"id": 1, "plaid_account_id": 2, "original_name": "BANK RAW $12", "payee": "Edited Shop", "amount": "12", "notes": "private"},
        {"id": 2, "plaid_account_id": 2, "original_name": "BANK RAW $12"},
        {"id": 3, "plaid_account_id": null, "original_name": "Manual"},
        {"id": 4, "plaid_account_id": 2, "original_name": null, "payee": "Never fallback"},
        {"id": 5, "plaid_account_id": 2, "original_name": "Split child", "split_parent_id": 1},
        {"id": 6, "plaid_account_id": 2, "original_name": "Group", "is_group_parent": true},
        {"id": 7, "plaid_account_id": 2, "original_name": "Pending", "is_pending": true},
        {"id": 8, "plaid_account_id": 2, "original_name": "   "},
        {"id": 9, "plaid_account_id": 2, "original_name": null, "payee": "Edited", "plaid_metadata": {"name": "PLAID RAW"}},
        {"id": 10, "plaid_account_id": 2, "original_name": "BANK RAW $12", "plaid_metadata": {"name": "DO NOT OVERRIDE"}}
    ]});
    let bundle = prepare(Source::LunchMoney, &input.to_string(), None, "global").unwrap();
    assert!(bundle.records.is_empty());
    assert_eq!(bundle.manifest.skipped_transactions, 6);
    let cases: Vec<_> = bundle.development.iter().chain(&bundle.holdout).collect();
    assert_eq!(cases.len(), 2);
    let descriptions: HashSet<_> = cases
        .iter()
        .map(|c| c.request.description.as_str())
        .collect();
    assert_eq!(descriptions, HashSet::from(["BANK RAW $12", "PLAID RAW"]));
    assert!(
        cases
            .iter()
            .all(|c| c.expected.is_none() && c.request.extra.is_empty())
    );
    assert!(prepare(Source::LunchMoney, "{}", None, "global").is_err());
}
#[test]
fn foursquare_rejects_manual_brand_mapping_inputs() {
    assert!(
        prepare(Source::Foursquare, "", Some("{\"brands\":[]}"), "ca")
            .err()
            .unwrap()
            .to_string()
            .contains("do not accept --examples")
    );
}
#[test]
fn lunchmoney_preserves_bank_context_without_edited_labels_or_account_data() {
    let input = json!({"transactions":[{
        "plaid_account_id":1,"original_name":"NORTHSTAR BOOKS 2232","payee":"Edited Target",
        "amount":"12.34","currency":"cad","date":"2026-01-03","notes":"secret",
        "custom_metadata":{"private":"secret"},
        "plaid_metadata":{"account_id":"secret","merchant_name":"Northstar Books",
            "website":"northstar.test","payment_channel":"in store","date":"2026-01-02",
            "location":{"city":"Granby","region":"QC","store_number":"02232","country":"ca"}}
    }]});
    let bundle = prepare(Source::LunchMoney, &input.to_string(), None, "global").unwrap();
    let case = bundle
        .development
        .iter()
        .chain(&bundle.holdout)
        .next()
        .unwrap();
    assert_eq!(case.request.amount.as_deref(), Some("12.34"));
    assert_eq!(case.request.currency.as_deref(), Some("CAD"));
    assert_eq!(case.request.date.as_deref(), Some("2026-01-02"));
    assert_eq!(case.request.country.as_deref(), Some("CA"));
    assert_eq!(
        case.request
            .location
            .as_ref()
            .unwrap()
            .store_number
            .as_deref(),
        Some("02232")
    );
    assert_eq!(
        case.request.extra["bank"]["merchant_name"],
        "Northstar Books"
    );
    assert!(case.expected.is_none());
    let saved = serde_json::to_string(case).unwrap();
    assert!(!saved.contains("secret"));
    assert!(!saved.contains("Edited Target"));
    assert_eq!(bundle.manifest.adapter_version, 5);
}
#[test]
fn moneyvis_discards_account_details() {
    let input = "Transaction Description,Transaction Type,Debit Amount,Credit Amount,Account Number,Sort Code,Balance\nLOCAL CAFE,DEB,4.50,,12345678,12-34-56,1000\n";
    let bundle = prepare(Source::MoneyVis, input, None, "global").unwrap();
    let sample = bundle
        .development
        .iter()
        .chain(&bundle.holdout)
        .next()
        .unwrap();
    assert_eq!(sample.request.amount.as_deref(), Some("4.50"));
    let serialized = serde_json::to_string(sample).unwrap();
    for sensitive in ["12345678", "12-34-56", "1000"] {
        assert!(!serialized.contains(sensitive));
    }
    assert!(sample.expected.is_none());
}

#[test]
fn rerun_preserves_user_labels_and_snapshot_path() {
    let bundle = prepare(
        Source::DoDataThings,
        "description,category\nADIDAS,Shopping\n",
        None,
        "global",
    )
    .unwrap();
    let directory = std::env::temp_dir().join(format!("ultra-datasets-{}", uuid::Uuid::new_v4()));
    let path = bundle.save(&directory).unwrap();
    std::fs::write(path.join("development.jsonl"), "user-labeled data").unwrap();
    assert_eq!(bundle.save(&directory).unwrap(), path);
    assert_eq!(
        std::fs::read_to_string(path.join("development.jsonl")).unwrap(),
        "user-labeled data"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn open_enrichment_keeps_regex_as_evidence_and_excludes_holdout_aliases() {
    let input = "id,name,parent_id,website_url,transaction_text_examples,transaction_text_regexp\na,Adidas,,https://adidas.com,\"[`ADIDAS`, `PAYMENT FROM ADIDAS`]\",(?i)ADIDAS\n";
    let bundle = prepare(Source::OpenEnrichment, input, None, "global").unwrap();
    assert_eq!(bundle.records.len(), 1);
    assert_eq!(
        bundle.records[0].raw["transaction_text_regexp"],
        "(?i)ADIDAS"
    );
    assert!(
        bundle.records[0]
            .raw
            .get("transaction_text_examples")
            .is_none()
    );
    for sample in bundle.holdout {
        assert!(
            !bundle.records[0]
                .merchant
                .aliases
                .iter()
                .any(|a| normalize(a) == normalize(&sample.request.description))
        );
    }
    assert!(bundle.records[0].merchant.logo_url.is_none());
}
