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
#[path = "../tests/unit/resolution/tests.rs"]
mod tests;

#[cfg(test)]
include!("../tests/unit/resolution/support.rs");
