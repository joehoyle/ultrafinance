use serde::{Deserialize, Serialize};

/// Entity classification, not a transaction operation or account-provider hint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum CounterpartyType {
    #[default]
    Merchant,
    FinancialInstitution,
    PaymentProcessor,
}

impl CounterpartyType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merchant => "merchant",
            Self::FinancialInstitution => "financial_institution",
            Self::PaymentProcessor => "payment_processor",
        }
    }
}

impl std::str::FromStr for CounterpartyType {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "merchant" => Ok(Self::Merchant),
            "financial_institution" => Ok(Self::FinancialInstitution),
            "payment_processor" => Ok(Self::PaymentProcessor),
            _ => anyhow::bail!("unsupported counterparty type: {value}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Counterparty, EnrichRequest, Enricher, MerchantResult,
        store::{MerchantStore, SourceRecord},
    };
    use serde_json::json;

    fn institution() -> Counterparty {
        serde_json::from_value(json!({"id":"bank", "name":"Example Bank",
            "type":"financial_institution", "website":"https://bank.example",
            "markets":["CA"], "aliases":["EXAMPLE MONTHLY FEE"]}))
        .unwrap()
    }

    #[test]
    fn counterparty_types_default_for_legacy_records_and_reject_unknown_types() {
        let legacy: Counterparty =
            serde_json::from_value(json!({"id":"cafe", "name":"Cafe"})).unwrap();
        assert_eq!(legacy.counterparty_type, CounterpartyType::Merchant);
        assert!(
            serde_json::from_value::<Counterparty>(json!({"id":"x","name":"X","type":"bank"}))
                .is_err()
        );
        let mut bank = institution();
        let bank_key = crate::dedupe::deterministic_key(&bank);
        bank.counterparty_type = CounterpartyType::Merchant;
        assert_ne!(bank_key, crate::dedupe::deterministic_key(&bank));
    }

    #[tokio::test]
    async fn counterparties_keep_institutions_out_of_legacy_merchant_results() -> anyhow::Result<()>
    {
        let bank = institution();
        let shop = serde_json::from_value(json!({"id":"shop","name":"Example Shop"}))?;
        let enricher = Enricher::new(None, "unused".into(), 0.95, vec![bank, shop])?;
        let requests: Vec<EnrichRequest> = ["EXAMPLE MONTHLY FEE", "Example Shop", "zzzzqqqq"]
            .iter()
            .map(|s| serde_json::from_value(json!({"description":s})).unwrap())
            .collect();
        let responses = enricher.enrich_batch(&requests).await;
        let bank = responses[0].as_ref().unwrap();
        assert_eq!(
            bank.counterparties[0].counterparty_type,
            CounterpartyType::FinancialInstitution
        );
        assert!(matches!(bank.merchant, MerchantResult::Unresolved { .. }));
        assert_eq!(bank.kind, crate::TransactionKind::Unknown);
        let shop = responses[1].as_ref().unwrap();
        assert_eq!(shop.counterparties[0].id, "shop");
        assert!(matches!(&shop.merchant, MerchantResult::Matched { data } if data.id == "shop"));
        assert!(responses[2].as_ref().unwrap().counterparties.is_empty());
        let page = enricher.list_merchants(None, None, 1, 0).await?;
        assert_eq!(page.total, 1);
        assert_eq!(page.merchants[0].id, "shop");
        let page = enricher
            .list_counterparties(
                None,
                None,
                1,
                0,
                Some(CounterpartyType::FinancialInstitution),
            )
            .await?;
        assert_eq!(page.total, 1);
        assert_eq!(page.merchants[0].id, "bank");
        assert_eq!(
            enricher
                .list_counterparties(None, None, 10, 0, None)
                .await?
                .total,
            2
        );
        Ok(())
    }

    #[test]
    fn counterparty_type_survives_source_import_refresh_and_manual_override() -> anyhow::Result<()>
    {
        let store = MerchantStore::temporary()?;
        let mut record = SourceRecord {
            source: "fixture".into(),
            external_id: "bank".into(),
            merchant: institution(),
            version: None,
            attribution: "Fixture".into(),
            license: "test".into(),
            url: "https://bank.example".into(),
            raw: json!({}),
        };
        store.reconcile_import(vec![record.clone()], false, 1)?;
        let id = store.resolve_source("fixture", "bank")?.unwrap();
        assert_eq!(
            store.get(&id)?.unwrap().counterparty_type,
            CounterpartyType::FinancialInstitution
        );
        assert_eq!(
            store.merchant_provenance(&id)?[0]
                .merchant
                .counterparty_type,
            CounterpartyType::FinancialInstitution
        );
        record.merchant.aliases.push("EXAMPLE BANK TRANSFER".into());
        store.reconcile_import(vec![record], false, 1)?;
        let mut manual = store.get(&id)?.unwrap();
        assert_eq!(
            manual.counterparty_type,
            CounterpartyType::FinancialInstitution
        );
        manual.name = "Example Bank Canada".into();
        store.put(&manual)?;
        assert_eq!(
            store.get(&id)?.unwrap().counterparty_type,
            CounterpartyType::FinancialInstitution
        );
        assert_eq!(
            store.search("EXAMPLE BANK TRANSFER", None, 10)?[0]
                .merchant
                .counterparty_type,
            CounterpartyType::FinancialInstitution
        );
        Ok(())
    }
    #[tokio::test]
    async fn processors_are_typed_and_keep_merchant_unresolved() -> anyhow::Result<()> {
        let processor = serde_json::from_value(
            json!({"id":"processor", "name":"Example Payments", "type":"payment_processor"}),
        )?;
        let enricher = Enricher::new(None, "unused".into(), 0.95, vec![processor])?;
        let response = enricher
            .enrich(&serde_json::from_value(
                json!({"description":"Example Payments"}),
            )?)
            .await?;
        assert_eq!(
            response.counterparties[0].counterparty_type,
            CounterpartyType::PaymentProcessor
        );
        assert!(matches!(
            response.merchant,
            MerchantResult::Unresolved { .. }
        ));
        assert_eq!(
            "payment_processor".parse::<CounterpartyType>()?,
            CounterpartyType::PaymentProcessor
        );
        Ok(())
    }
}
