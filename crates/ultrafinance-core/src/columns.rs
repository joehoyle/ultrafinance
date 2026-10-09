//! Column writes shared by import and refresh paths.
use anyhow::Result;
use postgres::GenericClient;

pub(crate) fn postgres_merchants(c: &mut impl GenericClient, id: &str, data: &str) -> Result<()> {
    c.execute(r#"INSERT INTO merchants(id,name,website,logo_url,logo_source,markets_json,aliases_json,sources_json) VALUES($1,($2::text::jsonb #>> '{name}'),($2::text::jsonb #>> '{website}'),($2::text::jsonb #>> '{logo_url}'),($2::text::jsonb #>> '{logo_source}'),COALESCE((($2::text::jsonb #> '{markets}'))::text,'[]'),COALESCE((($2::text::jsonb #> '{aliases}'))::text,'[]'),COALESCE((($2::text::jsonb #> '{sources}'))::text,'[]')) ON CONFLICT(id) DO UPDATE SET name=excluded.name,website=excluded.website,logo_url=excluded.logo_url,logo_source=excluded.logo_source,markets_json=excluded.markets_json,aliases_json=excluded.aliases_json,sources_json=excluded.sources_json"#, &[&id, &data])?;
    let mut merchant: crate::Merchant = serde_json::from_str(data)?;
    merchant.id = id.to_owned();
    postgres_identity_key(c, &merchant)?;
    Ok(())
}

pub(crate) fn postgres_identity_key(
    c: &mut impl GenericClient,
    merchant: &crate::Merchant,
) -> Result<()> {
    let key = crate::dedupe::deterministic_key(merchant);
    let rule_name = key.as_ref().map(|(name, _)| name.as_str());
    // Empty host represents a genuinely absent website, never an invalid URL.
    let rule_host = key.as_ref().map(|(_, host)| host.as_deref().unwrap_or(""));
    let normalized = crate::store::normalize(&merchant.name);
    let host = crate::dedupe::host(&merchant.website);
    c.execute("INSERT INTO merchant_identity_keys(merchant_id,normalized_name,website_host,rule_name,rule_host) VALUES($1,$2,$3,$4,$5) ON CONFLICT(merchant_id) DO UPDATE SET normalized_name=excluded.normalized_name,website_host=excluded.website_host,rule_name=excluded.rule_name,rule_host=excluded.rule_host", &[&merchant.id,&normalized,&host,&rule_name,&rule_host])?;
    Ok(())
}

pub(crate) fn postgres_manual_merchants(
    c: &mut impl GenericClient,
    id: &str,
    data: &str,
) -> Result<()> {
    c.execute(r#"INSERT INTO manual_merchants(id,name,website,logo_url,logo_source,markets_json,aliases_json,sources_json) VALUES($1,($2::text::jsonb #>> '{name}'),($2::text::jsonb #>> '{website}'),($2::text::jsonb #>> '{logo_url}'),($2::text::jsonb #>> '{logo_source}'),COALESCE((($2::text::jsonb #> '{markets}'))::text,'[]'),COALESCE((($2::text::jsonb #> '{aliases}'))::text,'[]'),COALESCE((($2::text::jsonb #> '{sources}'))::text,'[]')) ON CONFLICT(id) DO UPDATE SET name=excluded.name,website=excluded.website,logo_url=excluded.logo_url,logo_source=excluded.logo_source,markets_json=excluded.markets_json,aliases_json=excluded.aliases_json,sources_json=excluded.sources_json"#, &[&id, &data])?;
    Ok(())
}
pub(crate) fn postgres_source_records(
    c: &mut impl GenericClient,
    source: &str,
    external_id: &str,
    merchant_id: &str,
    data: &str,
) -> Result<()> {
    let record: crate::store::SourceRecord = serde_json::from_str(data)?;
    let m = &record.merchant;
    let strings = |key: &str| -> Vec<String> {
        record.raw[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    let hints = strings("countryHints");
    let negative_aliases = strings("negativeAliases");
    let region = crate::markets::dataset_region(&record);
    let pattern = record.raw["transaction_text_regexp"].as_str();
    let parent = record.raw["parent_id"].as_str();
    c.execute("INSERT INTO source_records(source,external_id,merchant_id,version,attribution,license,url,transaction_pattern,parent_id,input_id,input_name,input_website,input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,country_hints,dataset_region,negative_aliases) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20) ON CONFLICT(source,external_id) DO UPDATE SET merchant_id=excluded.merchant_id,version=excluded.version,attribution=excluded.attribution,license=excluded.license,url=excluded.url,transaction_pattern=excluded.transaction_pattern,parent_id=excluded.parent_id,input_id=excluded.input_id,input_name=excluded.input_name,input_website=excluded.input_website,input_logo_url=excluded.input_logo_url,input_logo_source=excluded.input_logo_source,input_markets=excluded.input_markets,input_aliases=excluded.input_aliases,input_sources=excluded.input_sources,country_hints=excluded.country_hints,dataset_region=excluded.dataset_region,negative_aliases=excluded.negative_aliases", &[&source,&external_id,&merchant_id,&record.version,&record.attribution,&record.license,&record.url,&pattern,&parent,&m.id,&m.name,&m.website,&m.logo_url,&m.logo_source,&m.markets,&m.aliases,&m.sources,&hints,&region,&negative_aliases])?;
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
