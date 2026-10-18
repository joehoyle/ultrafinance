use super::*;
use crate::logos::{LogoPage, WebsiteLogo, normalize_website};

const SELECT_LOGOS: &str = "SELECT jsonb_build_object('website_url', w.website_url, 'source_url', w.source_url, 'kind', w.kind, 'checked_at', w.checked_at::text, 'original', to_jsonb(o), 'display', to_jsonb(d)) FROM ultrafinance_logos.website_logos w JOIN ultrafinance_logos.assets o ON o.id=w.original_asset_id JOIN ultrafinance_logos.assets d ON d.id=w.display_asset_id";

fn installed(c: &mut impl GenericClient) -> Result<bool> {
    Ok(c.query_one(
        "SELECT to_regclass('ultrafinance_logos.website_logos') IS NOT NULL",
        &[],
    )?
    .get(0))
}
impl PostgresStore {
    pub fn list_website_logos(&self, limit: usize, offset: usize) -> Result<LogoPage> {
        self.run(move |c| {
            let mut tx = c
                .build_transaction()
                .read_only(true)
                .isolation_level(IsolationLevel::RepeatableRead)
                .start()?;
            if !installed(&mut tx)? {
                return Ok(LogoPage {
                    logos: vec![],
                    total: 0,
                    limit,
                    offset,
                });
            }
            let total: i64 = tx
                .query_one("SELECT count(*) FROM ultrafinance_logos.website_logos", &[])?
                .get(0);
            let logos = tx
                .query(
                    &format!("{SELECT_LOGOS} ORDER BY w.website_url LIMIT $1 OFFSET $2"),
                    &[&(limit as i64), &(offset as i64)],
                )?
                .into_iter()
                .map(|r| serde_json::from_value(r.get(0)).map_err(Into::into))
                .collect::<Result<Vec<_>>>()?;
            tx.commit()?;
            Ok(LogoPage {
                logos,
                total: total as usize,
                limit,
                offset,
            })
        })
    }

    pub fn website_logo(&self, website: &str) -> Result<Option<WebsiteLogo>> {
        let website = normalize_website(website)?;
        self.run(move |c| {
            if !installed(c)? {
                return Ok(None);
            }
            c.query_opt(
                &format!("{SELECT_LOGOS} WHERE w.website_url=$1"),
                &[&website],
            )?
            .map(|r| serde_json::from_value(r.get(0)).map_err(Into::into))
            .transpose()
        })
    }
    pub fn save_website_logo(&self, logo: &WebsiteLogo) -> Result<()> {
        let mut logo = logo.clone();
        logo.website_url = normalize_website(&logo.website_url)?;
        normalize_website(&logo.source_url)?;
        for asset in [&logo.original, &logo.display] {
            let url = reqwest::Url::parse(&asset.url)?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                bail!("published logo assets require credential-free HTTPS URLs");
            }
            if asset.width == 0
                || asset.height == 0
                || asset.width > 8192
                || asset.height > 8192
                || asset.byte_length == 0
                || asset.byte_length > 8 * 1024 * 1024
            {
                bail!("invalid image asset dimensions or size");
            }
        }
        self.run(move |c| {
            let mut tx = c.transaction()?;
            for asset in [&logo.original, &logo.display] {
                // Keep an existing immutable asset URL stable if a second website
                // uploads identical content to another storage destination.
                tx.execute("INSERT INTO ultrafinance_logos.assets (id,url,mime_type,width,height,byte_length) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (id) DO NOTHING", &[&asset.id, &asset.url, &asset.mime_type, &(asset.width as i32), &(asset.height as i32), &(asset.byte_length as i64)])?;
            }
            tx.execute("INSERT INTO ultrafinance_logos.website_logos (website_url,original_asset_id,display_asset_id,source_url,kind) VALUES ($1,$2,$3,$4,$5) ON CONFLICT (website_url) DO UPDATE SET original_asset_id=EXCLUDED.original_asset_id,display_asset_id=EXCLUDED.display_asset_id,source_url=EXCLUDED.source_url,kind=EXCLUDED.kind,checked_at=now()", &[&logo.website_url,&logo.original.id,&logo.display.id,&logo.source_url,&logo.kind.as_str()])?;
            tx.commit()?;
            Ok(())
        })
    }
    /// One indexed lookup for a whole result set; never discover over the network.
    pub fn logo_fallbacks(
        &self,
        websites: Vec<String>,
    ) -> Result<HashMap<String, (String, String)>> {
        if websites.is_empty() {
            return Ok(HashMap::new());
        }
        self.run(move |c| {
            if !installed(c)? { return Ok(HashMap::new()); }
            Ok(c.query("SELECT w.website_url,a.url,w.source_url FROM ultrafinance_logos.website_logos w JOIN ultrafinance_logos.assets a ON a.id=w.display_asset_id WHERE w.website_url=ANY($1)", &[&websites])?.into_iter().map(|r| (r.get(0), (r.get(1),r.get(2)))).collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MerchantStore;

    #[test]
    fn database_init_installs_logos_on_existing_catalog_without_runtime_ddl() {
        let store = MerchantStore::temporary().unwrap();
        assert!(store.0.run(installed).unwrap());
        store
            .0
            .run(|c| {
                c.batch_execute("DROP SCHEMA ultrafinance_logos CASCADE")?;
                Ok(())
            })
            .unwrap();
        let runtime = MerchantStore::postgres(store.temporary_url()).unwrap();
        assert!(
            runtime
                .website_logo("https://example.com")
                .unwrap()
                .is_none()
        );
        assert_eq!(runtime.list_website_logos(10, 0).unwrap().total, 0);
        assert!(!runtime.0.run(installed).unwrap());
        drop(runtime);
        let initialized = MerchantStore::initialize_postgres(store.temporary_url()).unwrap();
        assert!(initialized.0.run(installed).unwrap());
        // Additive logo tables must not change merchant schema compatibility.
        assert_eq!(
            initialized
                .0
                .run(|c| Ok(c
                    .query_one("SELECT version FROM ultrafinance_schema", &[])?
                    .get::<_, i32>(0)))
                .unwrap(),
            crate::store::POSTGRES_SCHEMA_VERSION
        );
    }
}
