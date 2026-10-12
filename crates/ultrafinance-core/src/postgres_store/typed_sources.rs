//! Typed, compact source inputs for canonical and coverage rebuilding.
use super::*;
pub(super) struct Input {
    pub merchant_id: String,
    pub merchant: Merchant,
    pub url: String,
    pub hints: Vec<String>,
    pub region: Option<String>,
}
fn decode(r: &postgres::Row) -> Input {
    Input {
        merchant_id: r.get(0),
        merchant: Merchant {
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
    }
}
pub(super) fn read(c: &mut impl GenericClient, ids: Option<&[String]>) -> Result<Vec<Input>> {
    Ok(c.query("SELECT merchant_id,input_id,input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,url,country_hints,dataset_region FROM source_records WHERE ($1::text[] IS NULL OR merchant_id=ANY($1)) ORDER BY source,external_id", &[&ids])?.iter().map(decode).collect())
}
/// Visit source inputs without collecting an entire chain's records in Rust.
pub(super) fn for_each(
    c: &mut impl GenericClient,
    ids: &[String],
    mut consume: impl FnMut(Input) -> Result<()>,
) -> Result<()> {
    use postgres::fallible_iterator::FallibleIterator;
    let mut rows=c.query_raw("SELECT merchant_id,input_id,input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,url,country_hints,dataset_region FROM source_records WHERE merchant_id=ANY($1) ORDER BY source,external_id", [&ids as &(dyn postgres::types::ToSql+Sync)])?;
    while let Some(row) = rows.next()? {
        consume(decode(&row))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/postgres_store/typed_sources/tests.rs"]
mod tests;
