//! Typed, compact source inputs for canonical and coverage rebuilding.
use super::*;
pub(super) struct Input {
    pub merchant_id: String,
    pub merchant: Merchant,
    pub url: String,
    pub hints: Vec<String>,
    pub region: Option<String>,
}
fn decode(r: &postgres::Row) -> Result<Input> {
    Ok(Input {
        merchant_id: r.get(0),
        merchant: Merchant {
            categories: None,
            category_ids: r.get(13),
            counterparty_type: r.try_get::<_, &str>(12)?.parse()?,
            id: r.get(1),
            name: r.get(2),
            website: r.get(3),
            logo_url: r.get(4),
            logo_source: r.get(5),
            markets: r.get(6),
            aliases: r.get(7),
            sources: r.get(8),
        },
        url: r.get(9),
        hints: r.get(10),
        region: r.get(11),
    })
}
pub(super) fn read(c: &mut impl GenericClient, ids: Option<&[String]>) -> Result<Vec<Input>> {
    let mut inputs = current_foursquare(c, ids)?;
    inputs.extend(c.query("SELECT merchant_id,COALESCE(input_id,external_id),input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,url,country_hints,dataset_region,input_counterparty_type,source_category_ids(s.source,s.external_id) FROM source_records s JOIN source_provenance p ON p.id=s.source_provenance_id WHERE ($1::text[] IS NULL OR merchant_id=ANY($1)) ORDER BY s.source,external_id", &[&ids])?.iter().map(decode).collect::<Result<Vec<_>>>()?);
    Ok(inputs)
}
/// Visit source inputs without collecting an entire chain's records in Rust.
pub(super) fn for_each(
    c: &mut impl GenericClient,
    ids: &[String],
    mut consume: impl FnMut(Input) -> Result<()>,
) -> Result<()> {
    for input in current_foursquare(c, Some(ids))? {
        consume(input)?;
    }
    use postgres::fallible_iterator::FallibleIterator;
    let mut rows=c.query_raw("SELECT merchant_id,COALESCE(input_id,external_id),input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,url,country_hints,dataset_region,input_counterparty_type,source_category_ids(s.source,s.external_id) FROM source_records s JOIN source_provenance p ON p.id=s.source_provenance_id WHERE merchant_id=ANY($1) ORDER BY s.source,external_id", [&ids as &(dyn postgres::types::ToSql+Sync)])?;
    while let Some(row) = rows.next()? {
        consume(decode(&row)?)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/typed_sources/tests.rs"]
mod tests;

// Current Foursquare merchants are authoritative, not reconstructed from originals.
fn current_foursquare(c: &mut impl GenericClient, ids: Option<&[String]>) -> Result<Vec<Input>> {
    let ready: bool = c
        .query_one("SELECT to_regclass('source_identities') IS NOT NULL", &[])?
        .get(0);
    if !ready {
        return Ok(vec![]);
    }
    c.query("SELECT m.*,merchant_category_ids(m.id) AS category_ids FROM merchants m WHERE ($1::text[] IS NULL OR m.id=ANY($1)) AND (EXISTS(SELECT 1 FROM location_records l WHERE l.merchant_id=m.merchant_key AND l.source='foursquare') OR EXISTS(SELECT 1 FROM foursquare_unlocated u WHERE u.merchant_id=m.merchant_key))", &[&ids])?.iter().map(|r| Ok(Input { merchant_id:r.get("id"), merchant:rows::merchant(r)?,url:String::new(),hints:vec![],region:None })).collect()
}
