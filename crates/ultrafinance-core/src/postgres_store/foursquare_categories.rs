//! Retain provider assignments independently of canonical merchant aggregates.
use super::*;
use postgres::{binary_copy::BinaryCopyInWriter, types::Type};

#[derive(PartialEq, Eq)]
pub(super) struct Assignment {
    pub external_id: String,
    pub identity_external_id: Option<String>,
    pub category_ids: Vec<String>,
}

pub(super) fn assignments(records: &[SourceRecord]) -> Vec<Assignment> {
    let mut assignments = Vec::new();
    for record in records {
        let places = record.raw["places"].as_array();
        let grouped = places.is_some_and(|places| {
            places.iter().any(|p| {
                p["fsq_place_id"]
                    .as_str()
                    .is_some_and(|id| format!("place:{id}") != record.external_id)
            })
        });
        if grouped {
            for place in places.into_iter().flatten() {
                if let Some(id) = place["fsq_place_id"].as_str() {
                    let mut categories: Vec<String> = place["fsq_category_ids"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect();
                    categories.sort();
                    categories.dedup();
                    assignments.push(Assignment {
                        external_id: format!("place:{id}"),
                        identity_external_id: Some(record.external_id.clone()),
                        category_ids: categories,
                    });
                }
            }
        } else {
            assignments.push(Assignment {
                external_id: record.external_id.clone(),
                identity_external_id: None,
                category_ids: record.merchant.category_ids.clone(),
            });
        }
    }
    assignments
}

pub(super) fn changes(
    tx: &mut Transaction<'_>,
    records: &[SourceRecord],
) -> Result<Vec<Assignment>> {
    let assignments = assignments(records);
    let ids: Vec<_> = assignments.iter().map(|a| a.external_id.clone()).collect();
    let previous: HashMap<String, Assignment> = tx.query("SELECT external_id,identity_external_id,foursquare_category_ids(external_id) FROM foursquare_categories WHERE external_id=ANY($1)", &[&ids])?
        .into_iter().map(|r| { let id: String = r.get(0); (id.clone(), Assignment { external_id:id, identity_external_id:r.get(1), category_ids:r.get(2) }) }).collect();
    Ok(assignments
        .into_iter()
        .filter(|a| previous.get(&a.external_id) != Some(a))
        .collect())
}

pub(super) fn persist(tx: &mut Transaction<'_>, assignments: &[Assignment]) -> Result<()> {
    if assignments.is_empty() {
        return Ok(());
    }
    tx.batch_execute("CREATE TEMP TABLE IF NOT EXISTS fsq_category_stage(external_id TEXT,identity_external_id TEXT,category_ids TEXT[]) ON COMMIT DROP; TRUNCATE fsq_category_stage")?;
    let sink = tx.copy_in("COPY fsq_category_stage FROM STDIN BINARY")?;
    let mut writer = BinaryCopyInWriter::new(sink, &[Type::TEXT, Type::TEXT, Type::TEXT_ARRAY]);
    for assignment in assignments {
        writer.write(&[
            &assignment.external_id,
            &assignment.identity_external_id,
            &assignment.category_ids,
        ])?;
    }
    writer.finish()?;
    tx.batch_execute("INSERT INTO foursquare_categories SELECT external_id,identity_external_id FROM fsq_category_stage ON CONFLICT(external_id) DO UPDATE SET identity_external_id=excluded.identity_external_id WHERE foursquare_categories.identity_external_id IS DISTINCT FROM excluded.identity_external_id; TRUNCATE fsq_category_stage")?;
    category_assignments::write(
        tx,
        category_assignments::Owner::Foursquare,
        &assignments
            .iter()
            .map(|a| (a.external_id.as_str(), "", a.category_ids.as_slice()))
            .collect::<Vec<_>>(),
    )?;
    Ok(())
}
