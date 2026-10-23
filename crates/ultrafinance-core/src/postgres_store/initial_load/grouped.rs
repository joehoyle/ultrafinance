//! Consume the sorted bootstrap stream without looking up existing identities.
//! Only one unfinished merchant and a bounded batch of source inputs stay live.
use super::*;
use crate::dedupe::{Decision, ImportReport, Report};

const DETAILS: usize = 5000;
struct Group {
    key: Option<(String, String)>,
    merchant: Merchant,
    numeric_id: i64,
    countries: MarketCountries,
    reported: bool,
}
impl Group {
    fn new(key: Option<(String, String)>, record: &SourceRecord, numeric_id: i64) -> Self {
        let mut merchant = record.merchant.clone();
        merchant.id = format!("mer_{}", uuid::Uuid::new_v4().simple());
        Self {
            key,
            merchant,
            numeric_id,
            countries: MarketCountries::default(),
            reported: false,
        }
    }
    fn add(&mut self, record: &SourceRecord) {
        let categories = std::mem::take(&mut self.merchant.category_ids);
        import_bulk::combine_source(&mut self.merchant, &record.merchant, &record.url);
        self.merchant.category_ids = categories;
        let hints: Vec<_> = record.raw["countryHints"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c.as_str().map(str::to_owned))
            .collect();
        self.countries
            .source_fields(&self.merchant.id, &record.merchant, &hints, None);
    }
    fn finish(mut self) -> (Merchant, i64) {
        let id = self.merchant.id.clone();
        import_bulk::finish_merchant(&mut self.merchant, &id);
        (self.countries.hydrate(self.merchant), self.numeric_id)
    }
    fn duplicate(&mut self, record: &SourceRecord, report: &mut Report) {
        report.candidates += 1;
        let needed = if self.reported { 1 } else { 2 };
        if report.merchants.len() + needed > DETAILS || report.decisions.len() == DETAILS {
            report.details_truncated = true;
            return;
        }
        let mut other = record.merchant.clone();
        other.id = format!("mer_{}", uuid::Uuid::new_v4().simple());
        report
            .groups
            .push(vec![self.merchant.id.clone(), other.id.clone()]);
        report.decisions.push(Decision {
            left: self.merchant.id.clone(),
            right: other.id.clone(),
            answer: crate::dedupe::rule_answer(&self.merchant),
            accepted: true,
            method: "rule".into(),
            error: None,
        });
        if !self.reported {
            report.merchants.push(self.merchant.clone());
            self.reported = true;
        }
        report.merchants.push(other);
    }
}

pub(super) fn load(
    tx: &mut Transaction<'_>,
    total: usize,
    chunk_size: usize,
    dry_run: bool,
) -> Result<ImportReport> {
    let revision: i64 = tx
        .query_one("SELECT revision FROM catalog_revision", &[])?
        .get(0);
    let mut report = Report {
        catalog_revision: Some(revision),
        dry_run,
        model: "deterministic".into(),
        threshold: 1.0,
        candidates: 0,
        errors: 0,
        merchants_scanned: total,
        merchants: vec![],
        groups: vec![],
        decisions: vec![],
        run_id: None,
        details_truncated: false,
    };
    let mut current: Option<Group> = None;
    let mut selected = 0;
    let mut progress = crate::import_progress::Overall::new(total);
    loop {
        let rows = tx.query(&format!("FETCH {chunk_size} FROM initial_records"), &[])?;
        if rows.is_empty() {
            break;
        }
        let mut records = Vec::with_capacity(rows.len());
        let mut owners = Vec::with_capacity(rows.len());
        let mut merchants = Vec::with_capacity(rows.len());
        // Reserve sequence values in one round trip. Gaps for grouped places
        // are intentional; no public identifier depends on these numbers.
        let keys = tx.query(
            "SELECT nextval('merchants_merchant_key_seq') FROM generate_series(1,$1::int)",
            &[&(rows.len() as i32)],
        )?;
        for (row, key_row) in rows.iter().zip(keys) {
            let key = row
                .get::<_, Option<String>>(0)
                .map(|name| (name, row.get::<_, String>(1)));
            let record: SourceRecord = serde_json::from_str(row.get::<_, &str>(2))?;
            // NULL means this record is not safely mergeable, never a shared bucket.
            let same = key.is_some() && current.as_ref().is_some_and(|g| g.key == key);
            if !same {
                if let Some(group) = current.take() {
                    merchants.push(group.finish());
                }
                current = Some(Group::new(key, &record, key_row.get(0)));
            }
            let group = current.as_mut().expect("group initialized above");
            group.add(&record);
            if same {
                group.duplicate(&record, &mut report);
            }
            owners.push(group.numeric_id);
            records.push(record);
        }
        import_bulk::write_initial_merchants(tx, &merchants)?;
        foursquare_identity::persist_initial(tx, &records, &owners)?;
        selected += records.len();
        progress.report(selected);
    }
    if let Some(group) = current.take() {
        import_bulk::write_initial_merchants(tx, &[group.finish()])?;
    }
    tx.batch_execute("CLOSE initial_records")?;
    if selected != total {
        bail!("initial grouped input count changed");
    }
    if report.candidates > 0 && !dry_run {
        report.run_id = Some(uuid::Uuid::new_v4().to_string());
    }
    Ok(ImportReport {
        delta: super::super::super::ImportDelta {
            added: if dry_run { 0 } else { total },
            updated: 0,
            unchanged: 0,
        },
        dedupe: report,
    })
}
