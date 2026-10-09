//! Merchant evidence is distinct from upstream identity mappings. Foursquare
//! place mappings remain refreshable, and detailed place evidence lives in
//! location_records; matching needs one source contribution per linked brand.
use super::*;
use serde_json::Value;

pub(super) fn read(
    c: &mut impl GenericClient,
    ids: &[String],
) -> Result<HashMap<String, Vec<SourceRecord>>> {
    let mut sources: HashMap<String, Vec<SourceRecord>> = HashMap::new();
    for row in c.query(include_str!("merchant_provenance.sql"), &[&ids])? {
        let mut record: SourceRecord = serde_json::from_str(row.get(1))?;
        if record.source == "foursquare" {
            for values in [
                &mut record.merchant.aliases,
                &mut record.merchant.markets,
                &mut record.merchant.sources,
            ] {
                values.sort();
                values.dedup();
            }
            for key in ["countryHints", "negativeAliases"] {
                if let Some(values) = record.raw[key].as_array_mut() {
                    values.sort_by_key(Value::to_string);
                    values.dedup();
                }
            }
        }
        sources.entry(row.get(0)).or_default().push(record);
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MerchantStore;
    use serde_json::json;

    fn place(index: usize) -> SourceRecord {
        let csv = format!(
            "fsq_place_id,name,country,address,locality,fsq_category_ids,date_closed\n{index},Cafe,CA,{index} Main St,Toronto,\"[\"\"restaurant\"\"]\",\n"
        );
        crate::foursquare::prepare(&csv, None, "ca")
            .unwrap()
            .0
            .remove(0)
    }

    #[test]
    fn linked_branches_have_one_merchant_contribution_and_refreshable_outlets() -> Result<()> {
        let store = MerchantStore::temporary()?;
        let mut records: Vec<_> = (0..200).map(place).collect();
        let mut other = records[0].clone();
        other.source = "merchant-studio".into();
        other.external_id = "cafe".into();
        other.raw = json!({});
        records.push(other);
        store.reconcile_import(records.clone(), false, 5000)?;
        let candidate = store.search("Cafe", None, 10)?.remove(0);
        let id = &candidate.merchant.id;
        assert_eq!(store.stats()?.total, 1);
        assert_eq!(candidate.provenance.len(), 2);
        let fsq = candidate
            .provenance
            .iter()
            .find(|r| r.source == "foursquare")
            .unwrap();
        assert_eq!(fsq.raw["source_record_count"], 200);
        // Unreviewed places cannot become a reviewed-brand identity by grouping.
        assert!(fsq.external_id.starts_with("place:"));
        assert_eq!(fsq.merchant.markets, ["CA"]);
        assert!(fsq.raw.get("places").is_none());
        assert!(serde_json::to_vec(&candidate)?.len() < 3000);
        assert_eq!(
            store.source_records("foursquare", None, 1000, 0)?.len(),
            200
        );
        assert_eq!(store.locations(id)?.len(), 200);
        assert_eq!(
            store.resolve_source("foursquare", "place:199")?.as_ref(),
            Some(id)
        );

        // Old remembered decisions must use the consolidated catalog evidence.
        let request = serde_json::from_value(json!({"description":"opaque cafe descriptor"}))?;
        let old = store
            .source_records("foursquare", None, 1000, 0)?
            .into_iter()
            .map(|r| serde_json::from_value(r["record"].clone()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        store.save_resolution(&crate::resolution::Resolution::supported(
            &request,
            candidate.merchant.clone(),
            old,
        ))?;
        assert_eq!(store.search_request(&request, 10)?[0].provenance.len(), 2);

        let original = store
            .locations(id)?
            .into_iter()
            .find(|r| r.external_id == "place:199")
            .unwrap();
        records[199].raw["places"][0]["address"] = json!("Updated Street");
        store.reconcile_import(records, false, 5000)?;
        let updated = store
            .locations(id)?
            .into_iter()
            .find(|r| r.external_id == "place:199")
            .unwrap();
        assert_eq!(updated.location.id, original.location.id);
        assert_eq!(updated.location.address.as_deref(), Some("Updated Street"));
        assert_eq!(store.merchant_provenance(id)?.len(), 2);

        let mut target = candidate.merchant.clone();
        target.id = "other".into();
        target.name = "Other Cafe".into();
        store.put(&target)?;
        store.link("foursquare", "place:199", &target.id)?;
        assert_eq!(store.locations(id)?.len(), 199);
        assert_eq!(store.locations(&target.id)?.len(), 1);
        let remaining = store.merchant_provenance(id)?;
        assert_eq!(
            remaining
                .iter()
                .find(|r| r.source == "foursquare")
                .unwrap()
                .raw["source_record_count"],
            199
        );
        assert_eq!(
            store.merchant_provenance(&target.id)?[0].raw["source_record_count"],
            1
        );
        Ok(())
    }

    #[test]
    fn consolidation_preserves_reviewed_identity_distinct_credits_and_matching_rules() -> Result<()>
    {
        let store = MerchantStore::temporary()?;
        let mut records: Vec<_> = (0..4).map(place).collect();
        records[1].external_id = "brand:cafe".into();
        records[2].url = "https://example.test/distinct-credit".into();
        records[3].raw["transaction_text_regexp"] = json!("^BANK CAFE");
        records[0].raw["negativeAliases"] = json!(["OTHER CAFE"]);
        records[1].raw["countryHints"] = json!(["CA"]);
        store.reconcile_import(records, false, 5000)?;
        let provenance = store.search("Cafe", None, 10)?.remove(0).provenance;
        assert_eq!(provenance.len(), 3);
        let brand = provenance
            .iter()
            .find(|r| r.external_id == "brand:cafe")
            .unwrap();
        assert_eq!(brand.raw["source_record_count"], 2);
        assert_eq!(brand.raw["negativeAliases"], json!(["OTHER CAFE"]));
        assert_eq!(brand.raw["countryHints"], json!(["CA"]));
        assert!(
            provenance
                .iter()
                .any(|r| r.url == "https://example.test/distinct-credit")
        );
        assert!(
            provenance
                .iter()
                .any(|r| r.raw["transaction_text_regexp"] == "^BANK CAFE")
        );
        assert!(store.search("OTHER CAFE", None, 10)?.is_empty());
        Ok(())
    }
}
