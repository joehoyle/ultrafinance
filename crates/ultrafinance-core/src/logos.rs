//! Website image associations, independent of merchant identity and imports.
use anyhow::{Result, bail};
use reqwest::Url;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogoKind {
    Logo,
    TouchIcon,
    ManifestIcon,
    Favicon,
}
impl LogoKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::TouchIcon => "touch_icon",
            Self::ManifestIcon => "manifest_icon",
            Self::Favicon => "favicon",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAsset {
    /// SHA-256 of the stored bytes; immutable and shared across websites.
    pub id: String,
    /// Managed assets use /logos/assets/<hash>.<extension>, relative to the API origin.
    /// Absolute HTTP(S) URLs remain supported for existing records.
    pub url: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub byte_length: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebsiteLogo {
    pub website_url: String,
    pub source_url: String,
    pub kind: LogoKind,
    pub display: ImageAsset,
    /// Database timestamp; refreshed only by an explicit discovery run.
    pub checked_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LogoPage {
    pub logos: Vec<WebsiteLogo>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

/// Remove a deployment origin from older managed asset URLs, preserving external logos.
pub fn asset_url(value: &str, id: &str) -> String {
    if let Ok(url) = Url::parse(value)
        && matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && is_asset_path(url.path(), id)
    {
        return url.path().to_owned();
    }
    value.to_owned()
}

/// Managed image path, independent of the deployment origin.
pub fn is_asset_path(value: &str, id: &str) -> bool {
    let Some(filename) = value.strip_prefix("/logos/assets/") else {
        return false;
    };
    let Some((hash, extension)) = filename.split_once('.') else {
        return false;
    };
    hash == id
        && hash.len() == 64
        && hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        && matches!(
            extension,
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "ico" | "svg"
        )
}

/// Preserve storefront paths and queries: different shops on one host need not
/// share a logo. Do not alias www, HTTP/HTTPS, redirects, or registrable domains.
pub fn normalize_website(value: &str) -> Result<String> {
    let value = value.trim();
    let mut url = if value.contains("://") {
        Url::parse(value)?
    } else {
        Url::parse(&format!("https://{value}"))?
    };
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("website must be an HTTP(S) URL without credentials");
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_managed_urls_use_current_origin_but_external_logos_stay_absolute() {
        let id = "a".repeat(64);
        let path = format!("/logos/assets/{id}.webp");
        assert_eq!(
            asset_url(&format!("https://127.0.0.1:3000{path}"), &id),
            path
        );
        assert_eq!(
            asset_url(&format!("https://ultrafinance.app{path}"), &id),
            path
        );
        for value in [
            "https://example.com/logo.png".to_owned(),
            format!("https://example.com{path}?token=x"),
            format!("https://example.com/logos/assets/{}.webp", "b".repeat(64)),
        ] {
            assert_eq!(asset_url(&value, &id), value);
        }
    }

    #[test]
    fn normalize_keeps_storefronts_separate() {
        assert_eq!(
            normalize_website("EXAMPLE.com").unwrap(),
            "https://example.com/"
        );
        assert_eq!(
            normalize_website("https://example.com/shop?a=1#top").unwrap(),
            "https://example.com/shop?a=1"
        );
        assert_ne!(
            normalize_website("https://host.test/a").unwrap(),
            normalize_website("https://host.test/b").unwrap()
        );
        for url in [
            "file:///tmp/a",
            "https://user:secret@example.com",
            "javascript:alert(1)",
        ] {
            assert!(normalize_website(url).is_err());
        }
    }
    #[test]
    fn associations_are_optional_and_manual_logos_win() {
        use crate::{Merchant, store::MerchantStore};
        let store = MerchantStore::temporary().unwrap();
        let mut merchant = Merchant {
            categories: None,
            category_ids: vec![],
            counterparty_type: Default::default(),
            id: "example".into(),
            name: "Example".into(),
            website: Some("example.com".into()),
            logo_url: None,
            logo_source: None,
            markets: vec![],
            aliases: vec![],
            sources: vec![],
        };
        store.put(&merchant).unwrap();
        assert!(store.get("example").unwrap().unwrap().logo_url.is_none());
        let asset = ImageAsset {
            id: "a".repeat(64),
            url: "https://cdn.example.com/logos/a.webp".into(),
            mime_type: "image/webp".into(),
            width: 256,
            height: 128,
            byte_length: 100,
        };
        store
            .save_website_logo(&WebsiteLogo {
                website_url: "example.com".into(),
                source_url: "https://example.com/logo.png".into(),
                kind: LogoKind::Logo,
                display: asset,
                checked_at: None,
            })
            .unwrap();
        // Re-running database setup must retain existing image associations.
        MerchantStore::initialize_postgres(store.temporary_url()).unwrap();
        assert!(
            store
                .website_logo("https://example.com")
                .unwrap()
                .unwrap()
                .checked_at
                .is_some()
        );
        assert!(
            store
                .get("example")
                .unwrap()
                .unwrap()
                .logo_url
                .unwrap()
                .contains("cdn.example.com")
        );
        assert!(
            store.list(None, 10, 0).unwrap().merchants[0]
                .logo_url
                .is_some()
        );
        assert!(
            store.search("Example", None, 10).unwrap()[0]
                .merchant
                .logo_url
                .is_some()
        );
        merchant.logo_url = Some("https://example.com/manual.svg".into());
        store.put(&merchant).unwrap();
        assert_eq!(
            store.get("example").unwrap().unwrap().logo_url,
            merchant.logo_url
        );
    }
}
