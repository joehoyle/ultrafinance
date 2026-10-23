//! Explicit, read-only benchmarks against a full catalog. No model calls.
use crate::{EnrichRequest, store::MerchantStore};
use std::time::Instant;

#[test]
#[ignore = "requires ULTRAFINANCE_PERF_DATABASE_URL pointing to a full catalog"]
fn profile_catalog_retrieval() -> anyhow::Result<()> {
    let url = std::env::var("ULTRAFINANCE_PERF_DATABASE_URL")?;
    let store = MerchantStore::postgres(&url)?;
    for round in 0..2 {
        for description in ["STARBUCKS", "TIM HORTONS", "SQ *EXAMPLE CAFE TORONTO"] {
            let request: EnrichRequest = serde_json::from_value(serde_json::json!({
                "description":description,"country":"CA"
            }))?;
            let started = Instant::now();
            let (candidates, _) = store.search_primary_request_traced(&request, 10)?;
            eprintln!(
                "{}",
                serde_json::json!({"round":round,"description":description,
                "milliseconds":started.elapsed().as_millis(),"candidates":candidates.len()})
            );
        }
        let started = Instant::now();
        let page = store.list_typed(None, 12, 0, Some(crate::CounterpartyType::Merchant))?;
        eprintln!(
            "{}",
            serde_json::json!({"round":round,"list_milliseconds":started.elapsed().as_millis(),"total":page.total})
        );
    }
    Ok(())
}
