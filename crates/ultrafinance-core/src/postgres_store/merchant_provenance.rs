//! Other providers expose retained inputs; Foursquare exposes a marked summary
//! of current merchant/outlet identity and shared attribution, never originals.
use super::*;

pub(super) fn read(
    c: &mut impl GenericClient,
    ids: &[String],
) -> Result<HashMap<String, Vec<SourceRecord>>> {
    read_with_merchants(c, ids, &HashMap::new())
}

pub(super) fn read_with_merchants(
    c: &mut impl GenericClient,
    ids: &[String],
    known: &HashMap<String, Merchant>,
) -> Result<HashMap<String, Vec<SourceRecord>>> {
    let mut sources: HashMap<String, Vec<SourceRecord>> = HashMap::new();
    if ids.is_empty() {
        return Ok(sources);
    }
    for row in c.query(include_str!("merchant_provenance.sql"), &[&ids])? {
        let record = rows::source(&row)?;
        sources
            .entry(row.try_get("merchant_id")?)
            .or_default()
            .push(record);
    }
    let summaries = c
        .query(
            include_str!("foursquare_provenance.sql"),
            &[
                &ids,
                &crate::foursquare::CREDIT,
                &"Apache-2.0",
                &crate::foursquare::URL,
            ],
        )
        .context("Foursquare provenance summary query failed")?;
    if summaries.is_empty() {
        return Ok(sources);
    }
    let mut summary_ids: Vec<String> = summaries
        .iter()
        .map(|r| r.get("merchant_id"))
        .filter(|id| !known.contains_key(id))
        .collect();
    summary_ids.sort_unstable();
    summary_ids.dedup();
    let merchants: HashMap<String, Merchant> = if summary_ids.is_empty() {
        HashMap::new()
    } else {
        c.query("SELECT *,merchant_category_ids(id) AS category_ids,category_details(merchant_category_ids(id))::text AS resolved_categories FROM merchants WHERE id=ANY($1)", &[&summary_ids])?
            .iter().map(|r| Ok((r.get("id"), rows::merchant(r)?))).collect::<Result<_>>()?
    };
    for row in summaries {
        let id: String = row.get("merchant_id");
        let merchant = known
            .get(&id)
            .or_else(|| merchants.get(&id))
            .context("provenance merchant missing")?
            .clone();
        sources.entry(id).or_default().push(SourceRecord {
            source:"foursquare".into(), external_id:row.get("external_id"), merchant,
            version:None, attribution:row.get("attribution"), license:row.get("license"),url:row.get("url"),
            raw:serde_json::json!({"identity_only":true,"source_record_count":row.get::<_,i64>("n")}),
        });
    }
    Ok(sources)
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/merchant_provenance/tests.rs"]
mod tests;
