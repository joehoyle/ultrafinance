//! Bulk replacement of category evidence, keeping unchanged references intact.
use anyhow::Result;
use postgres::GenericClient;
use serde_json::json;

pub(crate) enum Owner {
    Merchant,
    Manual,
    Source,
    Foursquare,
}

pub(crate) fn write(
    c: &mut impl GenericClient,
    owner: Owner,
    assignments: &[(&str, &str, &[String])],
) -> Result<()> {
    if assignments.is_empty() {
        return Ok(());
    }
    // Most catalog inputs have no categories. Remove any old references directly
    // instead of staging empty arrays or touching the taxonomy for every record.
    if assignments.iter().all(|(_, _, ids)| ids.is_empty()) {
        let ids: Vec<_> = assignments.iter().map(|(id, _, _)| *id).collect();
        match owner {
            Owner::Merchant => {
                c.execute(
                    "DELETE FROM merchant_categories WHERE merchant_id=ANY(merchant_keys($1))",
                    &[&ids],
                )?;
            }
            Owner::Manual => {
                c.execute(
                    "DELETE FROM manual_merchant_categories WHERE merchant_id=ANY($1)",
                    &[&ids],
                )?;
            }
            Owner::Foursquare => {
                c.execute(
                    "DELETE FROM foursquare_place_categories WHERE external_id=ANY($1)",
                    &[&ids],
                )?;
            }
            Owner::Source => {
                let sources: Vec<_> = assignments.iter().map(|(_, source, _)| *source).collect();
                c.execute("DELETE FROM source_record_categories a USING unnest($1::text[],$2::text[]) AS u(owner,source) WHERE a.external_id=u.owner AND a.source=u.source", &[&ids,&sources])?;
            }
        }
        return Ok(());
    }
    let input: Vec<_> = assignments
        .iter()
        .map(|(id, source, ids)| json!({"owner":id,"source":source,"category_ids":ids}))
        .collect();
    let data = serde_json::to_string(&input)?;
    c.batch_execute("CREATE TEMP TABLE IF NOT EXISTS category_update(owner TEXT,source TEXT,category_ids TEXT[]) ON COMMIT DROP; TRUNCATE category_update")?;
    c.execute("INSERT INTO category_update SELECT * FROM jsonb_to_recordset($1::text::jsonb) AS i(owner TEXT,source TEXT,category_ids TEXT[])", &[&data])?;
    // New provider IDs remain usable before a later taxonomy update supplies names.
    c.batch_execute("INSERT INTO categories(external_id) SELECT DISTINCT ids.external_id FROM category_update u CROSS JOIN LATERAL unnest(u.category_ids) ids(external_id) WHERE NOT EXISTS(SELECT 1 FROM categories c WHERE c.external_id=ids.external_id) ON CONFLICT(external_id) DO NOTHING")?;
    let (table, key, value, extra) = match owner {
        Owner::Merchant => (
            "merchant_categories",
            "merchant_id",
            "merchant_key(u.owner)",
            false,
        ),
        Owner::Manual => (
            "manual_merchant_categories",
            "merchant_id",
            "u.owner",
            false,
        ),
        Owner::Source => ("source_record_categories", "external_id", "u.owner", true),
        Owner::Foursquare => (
            "foursquare_place_categories",
            "external_id",
            "u.owner",
            false,
        ),
    };
    let source_match = if extra { " AND a.source=u.source" } else { "" };
    c.batch_execute(&format!("DELETE FROM {table} a USING category_update u WHERE a.{key}={value}{source_match} AND NOT EXISTS(SELECT 1 FROM categories c WHERE c.id=a.category_id AND c.external_id=ANY(u.category_ids))"))?;
    let (source_column, source_value) = if extra {
        ("source,", "u.source,")
    } else {
        ("", "")
    };
    c.batch_execute(&format!("INSERT INTO {table}({source_column}{key},category_id) SELECT DISTINCT {source_value}{value},c.id FROM category_update u CROSS JOIN LATERAL unnest(u.category_ids) ids(external_id) JOIN categories c USING(external_id) ON CONFLICT DO NOTHING; TRUNCATE category_update"))?;
    Ok(())
}
