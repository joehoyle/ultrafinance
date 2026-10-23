//! Atomic promotion of researched identity, descriptor alias, and its evidence.
use super::*;
use crate::{EnrichRequest, resolution::Resolution};

impl PostgresStore {
    pub fn persist_research(
        &self,
        request: &EnrichRequest,
        merchant: &Merchant,
        new: bool,
        source: &str,
        model: &str,
        evidence: serde_json::Value,
    ) -> Result<Merchant> {
        let request = request.clone();
        let mut merchant = merchant.clone();
        let source = source.to_owned();
        let model = model.to_owned();
        self.run(move |c| {
            let mut tx = c.transaction()?;
            write_lock(&mut tx)?;
            if new {
                let website = merchant.website.clone().context("new research merchant requires a website")?;
                merchant.id = website.clone();
                validate(&merchant)?;
                let record = SourceRecord {
                    source: "web-research".into(), external_id: website.clone(), merchant,
                    attribution: "Unverified web research".into(), license: "unspecified".into(), url: website.clone(), version: Some(model.clone()), raw: evidence.clone(),
                };
                let mut importer = indexed_import::Import::new(&mut tx, false, 1, 1)?;
                importer.chunk(vec![record])?;
                importer.finish()?;
                let id: String = tx.query_one("SELECT merchant_id FROM source_records WHERE source='web-research' AND external_id=$1", &[&website])?.get(0);
                merchant = rows::merchant(&tx.query_one("SELECT *,merchant_category_ids(id) AS category_ids,category_details(merchant_category_ids(id))::text AS resolved_categories FROM merchants WHERE id=$1", &[&id])?)?;
            } else {
                merchant = rows::merchant(&tx.query_opt("SELECT *,merchant_category_ids(id) AS category_ids,category_details(merchant_category_ids(id))::text AS resolved_categories FROM merchants WHERE id=$1", &[&merchant.id])?.context("researched merchant no longer exists")?)?;
            }
            let mut resolution = Resolution::supported(&request, merchant.clone(), vec![]);
            let previous = tx.query_opt(&format!("{} WHERE r.id=$1", rows::RESOLUTIONS), &[&resolution.id])?.map(|r| rows::resolution(&r)).transpose()?;
            if let Some(old) = &previous
                && (old.context != resolution.context || old.merchant.id != merchant.id)
            {
                bail!("research conflicts with an existing descriptor mapping; no changes saved");
            }
            // Repeated charges with different amounts retain their own scoped
            // resolution, but share one merchant/descriptor/country alias record.
            let alias_id = crate::resolution::key(&serde_json::json!({
                "merchant_id":merchant.id,"description":normalize(&request.description),"country":request.country
            }));
            if let Some(old) = tx.query_opt("SELECT merchant_id,input_aliases FROM source_records WHERE source='web-research-alias' AND external_id=$1", &[&alias_id])? {
                let id: String = old.get(0);
                let aliases: Vec<String> = old.get(1);
                if id != merchant.id || !aliases.iter().any(|a|normalize(a)==normalize(&request.description)) {
                    bail!("research alias identity conflicts with stored evidence");
                }
            }
            let mut alias_merchant = merchant.clone();
            alias_merchant.aliases = vec![request.description.clone()];
            // A discovered image is a read-time overlay, never research evidence.
            alias_merchant.logo_url = None;
            alias_merchant.logo_source = None;
            let alias = SourceRecord {
                source: "web-research-alias".into(), external_id: alias_id, merchant: alias_merchant,
                attribution: "Unverified web research".into(), license: "unspecified".into(), url: source,
                version: Some(model), raw: evidence.clone(),
            };
            validate(&alias.merchant)?;
            import_records(&mut tx, std::slice::from_ref(&alias), Some(&[merchant.id.clone()]), None)?;
            merchant = rows::merchant(&tx.query_one("SELECT *,merchant_category_ids(id) AS category_ids,category_details(merchant_category_ids(id))::text AS resolved_categories FROM merchants WHERE id=$1", &[&merchant.id])?)?;
            resolution.merchant = merchant.clone();
            resolution.provenance = vec![alias];
            resolution.evidence = Some(serde_json::to_string(&evidence)?);
            if !previous.as_ref().is_some_and(|r|r.verified) {
                crate::resolution::check_update(previous.as_ref(), &resolution)?;
                crate::columns::postgres_resolutions(&mut tx, &resolution.id, &serde_json::to_string(&resolution)?)?;
            }
            tx.commit()?;
            Ok(merchant)
        })
    }
}
