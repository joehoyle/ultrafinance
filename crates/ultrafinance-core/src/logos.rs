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
    pub original: ImageAsset,
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
                original: asset.clone(),
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
