//! Column writes shared by import and refresh paths.
use anyhow::Result;
use postgres::GenericClient;

pub(crate) fn postgres_merchants(c: &mut impl GenericClient, id: &str, data: &str) -> Result<()> {
    c.execute(r#"INSERT INTO merchants(id,name,website,logo_url,logo_source,markets_json,market_evidence_json,aliases_json,sources_json) VALUES($1,($2::text::jsonb #>> '{name}'),($2::text::jsonb #>> '{website}'),($2::text::jsonb #>> '{logo_url}'),($2::text::jsonb #>> '{logo_source}'),COALESCE((($2::text::jsonb #> '{markets}'))::text,'[]'),COALESCE((($2::text::jsonb #> '{market_evidence}'))::text,'[]'),COALESCE((($2::text::jsonb #> '{aliases}'))::text,'[]'),COALESCE((($2::text::jsonb #> '{sources}'))::text,'[]')) ON CONFLICT(id) DO UPDATE SET name=excluded.name,website=excluded.website,logo_url=excluded.logo_url,logo_source=excluded.logo_source,markets_json=excluded.markets_json,market_evidence_json=excluded.market_evidence_json,aliases_json=excluded.aliases_json,sources_json=excluded.sources_json"#, &[&id, &data])?;
    Ok(())
}
pub(crate) fn postgres_manual_merchants(
    c: &mut impl GenericClient,
    id: &str,
    data: &str,
) -> Result<()> {
    c.execute(r#"INSERT INTO manual_merchants(id,name,website,logo_url,logo_source,markets_json,market_evidence_json,aliases_json,sources_json) VALUES($1,($2::text::jsonb #>> '{name}'),($2::text::jsonb #>> '{website}'),($2::text::jsonb #>> '{logo_url}'),($2::text::jsonb #>> '{logo_source}'),COALESCE((($2::text::jsonb #> '{markets}'))::text,'[]'),COALESCE((($2::text::jsonb #> '{market_evidence}'))::text,'[]'),COALESCE((($2::text::jsonb #> '{aliases}'))::text,'[]'),COALESCE((($2::text::jsonb #> '{sources}'))::text,'[]')) ON CONFLICT(id) DO UPDATE SET name=excluded.name,website=excluded.website,logo_url=excluded.logo_url,logo_source=excluded.logo_source,markets_json=excluded.markets_json,market_evidence_json=excluded.market_evidence_json,aliases_json=excluded.aliases_json,sources_json=excluded.sources_json"#, &[&id, &data])?;
    Ok(())
}
pub(crate) fn postgres_source_records(
    c: &mut impl GenericClient,
    source: &str,
    external_id: &str,
    merchant_id: &str,
    data: &str,
) -> Result<()> {
    c.execute(r#"INSERT INTO source_records(source,external_id,merchant_id,merchant_json,raw_json,version,attribution,license,url,transaction_pattern,parent_id) VALUES($1,$2,$3,COALESCE((($4::text::jsonb #> '{merchant}'))::text,'{}'),COALESCE((($4::text::jsonb #> '{raw}'))::text,'{}'),($4::text::jsonb #>> '{version}'),($4::text::jsonb #>> '{attribution}'),($4::text::jsonb #>> '{license}'),($4::text::jsonb #>> '{url}'),CASE WHEN jsonb_typeof($4::text::jsonb #> '{raw,transaction_text_regexp}')='string' THEN ($4::text::jsonb #>> '{raw,transaction_text_regexp}') ELSE NULL END,($4::text::jsonb #>> '{raw,parent_id}')) ON CONFLICT(source,external_id) DO UPDATE SET merchant_id=excluded.merchant_id,merchant_json=excluded.merchant_json,raw_json=excluded.raw_json,version=excluded.version,attribution=excluded.attribution,license=excluded.license,url=excluded.url,transaction_pattern=excluded.transaction_pattern,parent_id=excluded.parent_id"#, &[&source, &external_id, &merchant_id, &data])?;
    Ok(())
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn postgres_location_records(
    c: &mut impl GenericClient,
    id: &str,
    source: &str,
    external_id: &str,
    merchant_id: &Option<&str>,
    merchant_source: &Option<&str>,
    merchant_external_id: &Option<&str>,
    data: &str,
) -> Result<()> {
    c.execute(r#"INSERT INTO location_records(id,source,external_id,merchant_id,merchant_source,merchant_external_id,name,precision,address,city,region,postal_code,country,store_number,latitude,longitude,aliases_json,place_ids_json,transaction_pattern,manual_override,attribution,license,url) VALUES($1,$2,$3,$4,$5,$6,($7::text::jsonb #>> '{location,name}'),($7::text::jsonb #>> '{location,precision}'),($7::text::jsonb #>> '{location,address}'),($7::text::jsonb #>> '{location,city}'),($7::text::jsonb #>> '{location,region}'),($7::text::jsonb #>> '{location,postal_code}'),($7::text::jsonb #>> '{location,country}'),($7::text::jsonb #>> '{location,store_number}'),(($7::text::jsonb #>> '{location,latitude}'))::double precision,(($7::text::jsonb #>> '{location,longitude}'))::double precision,COALESCE((($7::text::jsonb #> '{aliases}'))::text,'[]'),COALESCE((($7::text::jsonb #> '{place_ids}'))::text,'{}'),($7::text::jsonb #>> '{transaction_pattern}'),COALESCE((($7::text::jsonb #>> '{manual_override}'))::boolean,false),($7::text::jsonb #>> '{attribution}'),($7::text::jsonb #>> '{license}'),($7::text::jsonb #>> '{url}')) ON CONFLICT(source,external_id) DO UPDATE SET merchant_id=excluded.merchant_id,merchant_source=excluded.merchant_source,merchant_external_id=excluded.merchant_external_id,name=excluded.name,precision=excluded.precision,address=excluded.address,city=excluded.city,region=excluded.region,postal_code=excluded.postal_code,country=excluded.country,store_number=excluded.store_number,latitude=excluded.latitude,longitude=excluded.longitude,aliases_json=excluded.aliases_json,place_ids_json=excluded.place_ids_json,transaction_pattern=excluded.transaction_pattern,manual_override=excluded.manual_override,attribution=excluded.attribution,license=excluded.license,url=excluded.url"#, &[&id, &source, &external_id, &merchant_id, &merchant_source, &merchant_external_id, &data])?;
    Ok(())
}

pub(crate) fn postgres_resolutions(c: &mut impl GenericClient, id: &str, data: &str) -> Result<()> {
    c.execute(r#"INSERT INTO descriptor_resolutions(id,merchant_id,description,country,amount,currency,location_present,location_address,location_city,location_region,location_postal_code,location_country,location_store_number,extra_json,provenance_json,verified,review_evidence) VALUES($1,($2::text::jsonb #>> '{merchant,id}'),($2::text::jsonb #>> '{context,description}'),($2::text::jsonb #>> '{context,country}'),($2::text::jsonb #>> '{context,amount}'),($2::text::jsonb #>> '{context,currency}'),COALESCE(jsonb_typeof($2::text::jsonb #> '{context,location}')='object',false),($2::text::jsonb #>> '{context,location,address}'),($2::text::jsonb #>> '{context,location,city}'),($2::text::jsonb #>> '{context,location,region}'),($2::text::jsonb #>> '{context,location,postal_code}'),($2::text::jsonb #>> '{context,location,country}'),($2::text::jsonb #>> '{context,location,store_number}'),COALESCE(NULLIF(($2::text::jsonb #> '{context,extra}'), 'null'::jsonb)::text,'{}'),COALESCE(NULLIF(($2::text::jsonb #> '{provenance}'), 'null'::jsonb)::text,'[]'),COALESCE((($2::text::jsonb #>> '{verified}'))::boolean,false),($2::text::jsonb #>> '{evidence}')) ON CONFLICT(id) DO UPDATE SET merchant_id=excluded.merchant_id,description=excluded.description,country=excluded.country,amount=excluded.amount,currency=excluded.currency,location_present=excluded.location_present,location_address=excluded.location_address,location_city=excluded.location_city,location_region=excluded.location_region,location_postal_code=excluded.location_postal_code,location_country=excluded.location_country,location_store_number=excluded.location_store_number,extra_json=excluded.extra_json,provenance_json=excluded.provenance_json,verified=excluded.verified,review_evidence=excluded.review_evidence,updated_at=(clock_timestamp())"#, &[&id,&data])?;
    Ok(())
}

/// Canonical application serialization makes fingerprints independent of SQL JSON formatting.
pub(crate) fn canonical_document(data: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(data)?;
    Ok(if value.get("context").is_some() {
        serde_json::to_string(&serde_json::from_value::<crate::resolution::Resolution>(
            value,
        )?)?
    } else if value.get("location").is_some() {
        serde_json::to_string(&serde_json::from_value::<crate::location::LocationRecord>(
            value,
        )?)?
    } else if value.get("external_id").is_some() {
        serde_json::to_string(&serde_json::from_value::<crate::store::SourceRecord>(
            value,
        )?)?
    } else {
        serde_json::to_string(&serde_json::from_value::<crate::Merchant>(value)?)?
    })
}
