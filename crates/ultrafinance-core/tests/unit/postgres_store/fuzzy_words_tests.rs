use super::*;

#[test]
fn adaptive_words_preserve_legacy_threshold_exact_exclusion_and_boundary_ties() -> Result<()> {
    let store = crate::store::MerchantStore::temporary()?;
    let mut client = postgres::Client::connect(
        store.temporary_url(),
        postgres_native_tls::MakeTlsConnector::new(native_tls::TlsConnector::builder().build()?),
    )?;
    let words: Vec<String> = ('a'..='z')
        .map(|suffix| format!("cafe{suffix}"))
        .chain(
            [
                "cafe", "café", "caffè", "caféine", "coffee", "coffees", "coffer", "foobar", "aaaa",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .collect();
    client.execute(
        "INSERT INTO merchant_search_words SELECT unnest($1::text[]) ON CONFLICT DO NOTHING",
        &[&words],
    )?;
    let mut tx = client.transaction()?;
    tx.batch_execute("SET LOCAL pg_trgm.similarity_threshold='0.3'; SET LOCAL enable_seqscan=off; SET LOCAL jit=off")?;
    // The cafe suffixes have enough equally ranked matches to cross the top-4
    // boundary. Both passes must retain the legacy lexical tie ordering.
    let ties: i64 = tx.query_one("SELECT count(*) FROM merchant_search_words WHERE word LIKE 'cafe_' AND similarity(word,'cafe')=similarity('cafea','cafe')", &[])?.get(0);
    assert!(ties > 4);
    let mut cache = ReadCache::default();
    for token in [
        "cafe", "cafea", "café", "coffee", "coffe", "aaaa", "foobar", "absent", "", "a",
    ] {
        let mut expected: Vec<String> = tx.query(
            "SELECT word FROM merchant_search_words WHERE word % $1 AND word <> $1 ORDER BY similarity(word,$1) DESC,word LIMIT 4", &[&token]
        )?.into_iter().map(|row| row.get(0)).collect();
        expected.sort();
        assert_eq!(
            cache.fuzzy_words(&mut tx, &[token])?,
            expected,
            "token {token}"
        );
        assert_eq!(
            cache.fuzzy_words(&mut tx, &[token])?,
            expected,
            "cached token {token}"
        );
    }
    let threshold: String = tx.query_one("SHOW pg_trgm.similarity_threshold", &[])?.get(0);
    assert_eq!(threshold, "0.3");
    Ok(())
}
