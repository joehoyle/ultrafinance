//! Resolve public IDs once per bounded batch; never retain a catalog-wide map.
use super::*;

pub(super) fn keys(tx: &mut Transaction<'_>, ids: &[String]) -> Result<HashMap<String, i64>> {
    let keys: HashMap<_, _> = tx
        .query(
            "SELECT id,merchant_key FROM merchants WHERE id=ANY($1)",
            &[&ids],
        )?
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    if ids.iter().any(|id| !keys.contains_key(id)) {
        bail!("merchant public ID has no internal key");
    }
    Ok(keys)
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/numeric_ids_tests.rs"]
mod tests;
