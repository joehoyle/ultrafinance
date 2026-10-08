//! Context-scoped remembered resolutions. Model decisions never become verified aliases.
use crate::{EnrichRequest, Merchant, store::SourceRecord};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const LEGACY_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS descriptor_resolutions(id TEXT PRIMARY KEY,data TEXT NOT NULL);";

pub(crate) fn migrate_postgres(tx: &mut postgres::Transaction<'_>) -> anyhow::Result<()> {
    let rows = tx.query("SELECT id,data FROM descriptor_resolutions", &[])?;
    tx.batch_execute("ALTER TABLE descriptor_resolutions RENAME TO legacy_descriptor_resolutions; ALTER INDEX descriptor_resolutions_pkey RENAME TO legacy_descriptor_resolutions_pkey;")?;
    tx.batch_execute(include_str!("../migrations/005_resolutions_postgres.sql"))?;
    for row in rows {
        let id: String = row.get(0);
        let mut resolution: Resolution = serde_json::from_str(row.get(1))?;
        for _ in 0..32 {
            if let Some(next) = tx.query_opt(
                "SELECT merchant_id FROM merchant_redirects WHERE retired_id=$1",
                &[&resolution.merchant.id],
            )? {
                resolution.merchant.id = next.get(0);
            } else {
                break;
            }
        }
        if id != resolution.id || id != key(&resolution.context) {
            bail!("invalid legacy descriptor mapping identity");
        }
        crate::columns::postgres_resolutions(tx, &id, &serde_json::to_string(&resolution)?)?;
    }
    tx.batch_execute("DROP TABLE legacy_descriptor_resolutions;")?;
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resolution {
    pub id: String,
    pub context: Value,
    pub merchant: Merchant,
    pub provenance: Vec<SourceRecord>,
    pub verified: bool,
    pub evidence: Option<String>,
}
impl Resolution {
    pub fn supported(
        request: &EnrichRequest,
        merchant: Merchant,
        provenance: Vec<SourceRecord>,
    ) -> Self {
        let context = context(request);
        Self {
            id: key(&context),
            context,
            merchant,
            provenance,
            verified: false,
            evidence: None,
        }
    }
}
pub fn context(request: &EnrichRequest) -> Value {
    // Dates are omitted; all other supplied context is retained conservatively.
    json!({"description":crate::store::normalize(&request.description),"country":request.country,
        "amount":request.amount,"currency":request.currency,"location":request.location,"extra":request.extra})
}
pub(crate) fn key(context: &Value) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in serde_json::to_vec(context).expect("JSON context is serializable") {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("descriptor-{hash:016x}")
}
pub(crate) fn check_update(old: Option<&Resolution>, new: &Resolution) -> Result<()> {
    if let Some(old) = old {
        if old.context != new.context {
            bail!("descriptor key collision; no mapping changed");
        }
        if old.merchant.id != new.merchant.id {
            bail!("conflicting descriptor mapping; revoke the existing mapping first");
        }
        if old.verified && !new.verified {
            bail!("a model result cannot replace a verified mapping");
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn exercise(store: &crate::store::MerchantStore) -> Result<()> {
    let merchant: Merchant = serde_json::from_value(
        json!({"id":"resolution-cafe","name":"Example Cafe","markets":["CA"]}),
    )?;
    store.put(&merchant)?;
    let request: EnrichRequest = serde_json::from_value(
        json!({"description":"opaque zxmq 123","country":"CA","amount":"10.00","extra":{"bank":"one"}}),
    )?;
    let context = context(&request);
    let mut resolution = Resolution {
        id: key(&context),
        context,
        merchant,
        provenance: vec![],
        verified: false,
        evidence: None,
    };
    let before = store.fingerprint()?;
    assert!(store.save_resolution(&resolution)?);
    assert_ne!(before, store.fingerprint()?);
    let candidates = store.search_request(&request, 10)?;
    assert_eq!(candidates[0].merchant.id, "resolution-cafe");
    assert!(!candidates[0].exact && !candidates[0].trusted);
    resolution.verified = true;
    assert!(store.save_resolution(&resolution).is_err());
    resolution.evidence = Some("Reviewed receipt and official website".into());
    assert!(store.save_resolution(&resolution)?);
    let candidates = store.search_request(&request, 10)?;
    assert!(candidates[0].exact && candidates[0].trusted);
    for changed in [
        json!({"country":"US"}),
        json!({"extra":{"bank":"two"}}),
        json!({"amount":"20.00"}),
    ] {
        let mut value = serde_json::to_value(&request)?;
        for (key, value2) in changed.as_object().unwrap() {
            value[key] = value2.clone();
        }
        let changed: EnrichRequest = serde_json::from_value(value)?;
        assert!(
            store
                .search_request(&changed, 10)?
                .iter()
                .all(|c| c.resolution_id.is_none())
        );
    }
    let mut old = resolution.clone();
    old.verified = false;
    old.evidence = None;
    assert!(store.save_resolution(&old).is_err());
    let merchant2: Merchant =
        serde_json::from_value(json!({"id":"resolution-other","name":"Different Shop"}))?;
    store.put(&merchant2)?;
    let mut conflict = resolution.clone();
    conflict.merchant = merchant2;
    assert!(store.save_resolution(&conflict).is_err());
    let records = crate::import::catalog(
        r#"[{"id":"duplicate","name":"Example Cafe"}]"#,
        "resolution-fixture",
    )?;
    store.import(&records)?;
    let retired = store
        .resolve_source("resolution-fixture", "duplicate")?
        .unwrap();
    let merge_request: EnrichRequest =
        serde_json::from_value(json!({"description":"merge fixture opaque"}))?;
    let old_mapping = Resolution::supported(&merge_request, store.get(&retired)?.unwrap(), vec![]);
    store.save_resolution(&old_mapping)?;
    let snapshot = store.dedupe_snapshot()?;
    store.apply_dedupe(
        &snapshot,
        &[vec!["resolution-cafe".into(), retired]],
        &json!({"fixture":"mapping foreign key"}),
    )?;
    assert_eq!(
        store.resolutions(Some(&old_mapping.id), 1)?[0].merchant.id,
        "resolution-cafe"
    );
    // Saving an older caller's record follows the redirect into the current FK.
    store.save_resolution(&old_mapping)?;
    assert!(store.revoke_resolution(&resolution.id)?);
    assert!(store.resolutions(Some(&resolution.id), 1)?.is_empty());
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn mappings_are_scoped_verified_explicitly_and_revocable() -> anyhow::Result<()> {
        super::exercise(&crate::store::MerchantStore::temporary()?)
    }
}
