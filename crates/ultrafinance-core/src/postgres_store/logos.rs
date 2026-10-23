use super::*;
use crate::logos::{LogoPage, WebsiteLogo, normalize_website};

const SELECT_LOGOS: &str = "SELECT jsonb_build_object('website_url', w.website_url, 'source_url', w.source_url, 'kind', w.kind, 'checked_at', w.checked_at::text, 'display', to_jsonb(d)) FROM ultrafinance_logos.website_logos w JOIN ultrafinance_logos.assets d ON d.id=w.display_asset_id";

fn decode_logo(value: serde_json::Value) -> Result<WebsiteLogo> {
    let mut logo: WebsiteLogo = serde_json::from_value(value)?;
    logo.display.url = crate::logos::asset_url(&logo.display.url, &logo.display.id);
    Ok(logo)
}

fn installed(c: &mut impl GenericClient) -> Result<bool> {
    Ok(c.query_one(
        "SELECT to_regclass('ultrafinance_logos.website_logos') IS NOT NULL",
        &[],
    )?
    .get(0))
}
impl PostgresStore {
    pub fn logo_discovery_websites(
        &self,
        market: &str,
        refresh: bool,
        limit: Option<usize>,
    ) -> Result<Vec<String>> {
        if market.len() != 2 || !market.bytes().all(|c| c.is_ascii_uppercase()) {
            bail!("market must be a two-letter uppercase country code");
        }
        if limit == Some(0) {
            bail!("website limit must be positive");
        }
        let market = market.to_owned();
        self.run(move |client| {
            // This is an offline administrative scan, not an API request. Keep the
            // relaxed timeout local to a read-only transaction on this connection.
            let mut tx = client.build_transaction().read_only(true).start()?;
            tx.batch_execute("SET LOCAL statement_timeout='0'; SET LOCAL work_mem='64MB'")?;
            let c = &mut tx;
            let known: HashSet<String> = if !refresh && installed(&mut *c)? {
                c.query("SELECT website_url FROM ultrafinance_logos.website_logos", &[])?.into_iter().map(|r| r.get(0)).collect()
            } else { HashSet::new() };
            // Filter the authoritative market array directly; avoid paginating the
            // entire merchant catalog or doing one query per branch/website.
            let websites: std::collections::BTreeSet<String> = c.query(
                "SELECT DISTINCT website FROM merchants WHERE markets @> ARRAY[$1::text] AND counterparty_type='merchant' AND website IS NOT NULL AND btrim(website)<>'' AND (logo_url IS NULL OR btrim(logo_url)='')",
                &[&market],
            )?.into_iter().filter_map(|r| normalize_website(r.get::<_, String>(0).as_str()).ok())
                .filter(|website| !known.contains(website)).collect();
            tx.commit()?;
            Ok(websites.into_iter().take(limit.unwrap_or(usize::MAX)).collect())
        })
    }

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
                .map(|r| decode_logo(r.get(0)))
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
            .map(|r| decode_logo(r.get(0)))
            .transpose()
        })
    }
    pub fn save_website_logo(&self, logo: &WebsiteLogo) -> Result<()> {
        let mut logo = logo.clone();
        logo.website_url = normalize_website(&logo.website_url)?;
        normalize_website(&logo.source_url)?;
        {
            let asset = &logo.display;
            if !crate::logos::is_asset_path(&asset.url, &asset.id) {
                let url = reqwest::Url::parse(&asset.url)?;
                if !matches!(url.scheme(), "http" | "https")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    bail!(
                        "published logo assets require a managed asset path or credential-free HTTP(S) URL"
                    );
                }
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
            {
                let asset = &logo.display;
                // Rediscovery upgrades legacy absolute URLs to portable managed paths.
                // Preserve existing URLs for legacy callers supplying absolute URLs.
                tx.execute("INSERT INTO ultrafinance_logos.assets (id,url,mime_type,width,height,byte_length) VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT (id) DO UPDATE SET url=EXCLUDED.url WHERE EXCLUDED.url LIKE '/logos/assets/%'", &[&asset.id, &asset.url, &asset.mime_type, &(asset.width as i32), &(asset.height as i32), &(asset.byte_length as i64)])?;
            }
            tx.execute("INSERT INTO ultrafinance_logos.website_logos (website_url,display_asset_id,source_url,kind) VALUES ($1,$2,$3,$4) ON CONFLICT (website_url) DO UPDATE SET display_asset_id=EXCLUDED.display_asset_id,source_url=EXCLUDED.source_url,kind=EXCLUDED.kind,checked_at=now()", &[&logo.website_url,&logo.display.id,&logo.source_url,&logo.kind.as_str()])?;
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
            Ok(c.query("SELECT w.website_url,a.url,w.source_url,a.id FROM ultrafinance_logos.website_logos w JOIN ultrafinance_logos.assets a ON a.id=w.display_asset_id WHERE w.website_url=ANY($1)", &[&websites])?.into_iter().map(|r| (r.get(0), (crate::logos::asset_url(r.get::<_, String>(1).as_str(), r.get::<_, String>(3).as_str()),r.get(2)))).collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MerchantStore;

    #[test]
    fn saves_origin_independent_logos_and_accepts_http_urls() {
        use crate::logos::{ImageAsset, LogoKind};
        let store = MerchantStore::temporary().unwrap();
        let asset = ImageAsset {
            id: "a".repeat(64),
            url: format!("/logos/assets/{}.webp", "a".repeat(64)),
            mime_type: "image/webp".into(),
            width: 16,
            height: 16,
            byte_length: 4,
        };
        let mut logo = WebsiteLogo {
            website_url: "https://example.com/".into(),
            source_url: "https://example.com/logo.png".into(),
            kind: LogoKind::Logo,
            display: asset,
            checked_at: None,
        };
        // Simulate an older discovery record pinned to a particular origin.
        let managed_url = logo.display.url.clone();
        logo.display.url = "http://old-host.test/logo.webp".into();
        store.save_website_logo(&logo).unwrap();
        // Simulate the exact HTTPS localhost URLs created by the old CLI.
        let legacy_url = format!("https://127.0.0.1:3000{managed_url}");
        store
            .0
            .run(move |c| {
                c.execute(
                    "UPDATE ultrafinance_logos.assets SET url=$1",
                    &[&legacy_url],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store
                .website_logo(&logo.website_url)
                .unwrap()
                .unwrap()
                .display
                .url,
            managed_url
        );
        assert_eq!(
            store.list_website_logos(10, 0).unwrap().logos[0]
                .display
                .url,
            managed_url
        );
        let fallbacks = store
            .0
            .logo_fallbacks(vec![logo.website_url.clone()])
            .unwrap();
        assert_eq!(fallbacks[&logo.website_url].0, managed_url);
        logo.display.url = managed_url.clone();
        store.save_website_logo(&logo).unwrap();
        assert_eq!(
            store
                .website_logo(&logo.website_url)
                .unwrap()
                .unwrap()
                .display
                .url,
            logo.display.url
        );
        logo.display.url = "http://example.com/logo.webp".into();
        assert!(store.save_website_logo(&logo).is_ok());
        logo.display.url = "http://user:secret@localhost/logo.webp".into();
        assert!(store.save_website_logo(&logo).is_err());
        for invalid in [
            "//evil.example/logo.webp",
            "/logos/assets/../secret",
            "/other/logo.webp",
            "/logos/assets/wrong.webp",
        ] {
            logo.display.url = invalid.into();
            assert!(store.save_website_logo(&logo).is_err());
        }
    }

    #[test]
    fn database_init_removes_legacy_original_reference_and_preserves_display() {
        let store = MerchantStore::temporary().unwrap();
        store.0.run(|c| {
            c.batch_execute("INSERT INTO ultrafinance_logos.assets (id,url,mime_type,width,height,byte_length) VALUES (repeat('a',64),'/original.svg','image/svg+xml',1024,512,2000),(repeat('b',64),'/display.webp','image/webp',256,128,100); ALTER TABLE ultrafinance_logos.website_logos ADD COLUMN original_asset_id text REFERENCES ultrafinance_logos.assets(id); INSERT INTO ultrafinance_logos.website_logos (website_url,original_asset_id,display_asset_id,source_url,kind) VALUES ('https://example.com/',repeat('a',64),repeat('b',64),'https://example.com/logo.svg','logo'); ALTER TABLE ultrafinance_logos.website_logos ALTER COLUMN original_asset_id SET NOT NULL")?;
            Ok(())
        }).unwrap();
        let upgraded = MerchantStore::initialize_postgres(store.temporary_url()).unwrap();
        let logo = upgraded
            .website_logo("https://example.com")
            .unwrap()
            .unwrap();
        assert_eq!(logo.display.id, "b".repeat(64));
        assert_eq!(logo.source_url, "https://example.com/logo.svg");
        assert!(
            serde_json::to_value(&logo)
                .unwrap()
                .get("original")
                .is_none()
        );
        assert!(upgraded.0.run(|c| Ok(c.query_one("SELECT NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema='ultrafinance_logos' AND table_name='website_logos' AND column_name='original_asset_id')", &[])?.get::<_, bool>(0))).unwrap());
    }

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
