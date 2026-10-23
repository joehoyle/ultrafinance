use super::images::{self, Prepared};
use anyhow::{Context, Result, bail};
use reqwest::{Url, redirect::Policy};
use scraper::{Html, Selector};
use serde_json::Value;
use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use ultrafinance_core::logos::{LogoKind, normalize_website};

pub struct Discovered {
    pub source_url: String,
    pub kind: LogoKind,
    pub image: Prepared,
}
struct Candidate {
    url: Url,
    kind: LogoKind,
    priority: u8,
}
struct Download {
    bytes: Vec<u8>,
    url: Url,
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || a == 0
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0)
        || (a == 198 && (b == 18 || b == 19)))
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => {
            // Only global unicast; exclude special-use 2001::/23 and docs.
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && !in_documentation_v6(ip)
        }
    }
}
fn in_documentation_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    s[0] == 0x3fff && s[1] < 0x1000
}
async fn fetch(mut url: Url, max: usize) -> Result<Download> {
    for _ in 0..6 {
        normalize_website(url.as_str())?;
        let host = url.host_str().context("missing host")?.to_string();
        let port = url.port_or_known_default().context("invalid port")?;
        if port != 80 && port != 443 {
            bail!("only standard HTTP(S) ports are allowed");
        }
        // Resolve and vet every redirect. Pin the checked addresses for direct
        // connections; configured outbound proxies remain responsible for DNS.
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await?
            .collect();
        if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
            bail!("website resolves to a non-public address");
        }
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .resolve_to_addrs(&host, &addresses)
            .user_agent("UltrafinanceLogoDiscovery/1.0 (+https://ultrafinance.app)")
            .build()?;
        let mut response = client.get(url.clone()).send().await?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .context("redirect without location")?
                .to_str()?;
            url = url.join(location)?;
            continue;
        }
        response.error_for_status_ref()?;
        if response.content_length().is_some_and(|n| n > max as u64) {
            bail!("download exceeds size limit");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > max {
                bail!("download exceeds size limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(Download { bytes, url });
    }
    bail!("too many redirects")
}
fn candidate(base: &Url, value: &str, kind: LogoKind) -> Option<Candidate> {
    let url = base.join(value).ok()?;
    normalize_website(url.as_str()).ok()?;
    let priority = match kind {
        LogoKind::Logo => 0,
        LogoKind::TouchIcon => 1,
        LogoKind::ManifestIcon => 2,
        LogoKind::Favicon => 3,
    };
    Some(Candidate {
        url,
        kind,
        priority,
    })
}
fn organization_logos(value: &Value, base: &Url, out: &mut Vec<Candidate>, depth: usize) {
    if depth > 16 || out.len() >= 24 {
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values.iter().take(64) {
                organization_logos(value, base, out, depth + 1);
            }
        }
        Value::Object(map) => {
            let is_organization = match map.get("@type") {
                Some(Value::String(s)) => {
                    s == "Organization"
                        || s == "LocalBusiness"
                        || s == "Corporation"
                        || s == "Brand"
                }
                Some(Value::Array(v)) => v.iter().any(|v| {
                    v == "Organization"
                        || v == "LocalBusiness"
                        || v == "Corporation"
                        || v == "Brand"
                }),
                _ => false,
            };
            if is_organization && let Some(logo) = map.get("logo") {
                let url = logo
                    .as_str()
                    .or_else(|| logo.get("url").and_then(Value::as_str))
                    .or_else(|| logo.get("contentUrl").and_then(Value::as_str));
                if let Some(url) = url
                    && let Some(c) = candidate(base, url, LogoKind::Logo)
                {
                    out.push(c);
                }
            }
            // Only walk structured containers, avoiding arbitrary objects (e.g.
            // logos for unrelated brands in a product/review list).
            if let Some(graph) = map.get("@graph") {
                organization_logos(graph, base, out, depth + 1);
            }
        }
        _ => {}
    }
}
fn html_candidates(html: &str, base: &Url) -> (Vec<Candidate>, Vec<Url>) {
    let document = Html::parse_document(html);
    let mut out = Vec::new();
    for node in document
        .select(&Selector::parse("script[type='application/ld+json']").unwrap())
        .take(16)
    {
        if let Ok(value) = serde_json::from_str::<Value>(&node.inner_html()) {
            organization_logos(&value, base, &mut out, 0);
        }
    }
    let mut manifests = Vec::new();
    for node in document
        .select(&Selector::parse("link[rel][href]").unwrap())
        .take(128)
    {
        let rel = node.value().attr("rel").unwrap().to_ascii_lowercase();
        let href = node.value().attr("href").unwrap();
        let tokens: Vec<_> = rel.split_whitespace().collect();
        let kind = if tokens.iter().any(|r| r.starts_with("apple-touch-icon")) {
            Some(LogoKind::TouchIcon)
        } else if tokens.contains(&"icon") {
            Some(LogoKind::Favicon)
        } else {
            None
        };
        if let Some(kind) = kind
            && let Some(c) = candidate(base, href, kind)
        {
            out.push(c);
        }
        if tokens.contains(&"manifest")
            && manifests.len() < 2
            && let Ok(url) = base.join(href)
        {
            manifests.push(url);
        }
    }
    (out, manifests)
}
pub async fn discover(website: &str, explicit: Option<&str>) -> Result<Discovered> {
    tokio::time::timeout(Duration::from_secs(120), discover_inner(website, explicit))
        .await
        .context("logo discovery exceeded two minutes")?
}
async fn discover_inner(website: &str, explicit: Option<&str>) -> Result<Discovered> {
    discover_with_fetch(website, explicit, fetch).await
}

fn conventional_icons(base: &Url) -> Vec<Candidate> {
    [
        ("/apple-touch-icon.png", LogoKind::TouchIcon),
        ("/apple-touch-icon-precomposed.png", LogoKind::TouchIcon),
        ("/favicon.ico", LogoKind::Favicon),
        ("/favicon.png", LogoKind::Favicon),
    ]
    .into_iter()
    .filter_map(|(path, kind)| candidate(base, path, kind))
    .collect()
}

async fn discover_with_fetch<F, Fut>(
    website: &str,
    explicit: Option<&str>,
    mut download: F,
) -> Result<Discovered>
where
    F: FnMut(Url, usize) -> Fut,
    Fut: std::future::Future<Output = Result<Download>>,
{
    let website = Url::parse(&normalize_website(website)?)?;
    let mut seen = HashSet::new();
    if let Some(url) = explicit {
        let c = candidate(&website, url, LogoKind::Logo).context("invalid explicit image URL")?;
        return evaluate_candidates(vec![c], &mut download, &mut seen).await;
    }
    let mut fallback_base = website.clone();
    // Reserve part of the overall two-minute deadline for conventional paths,
    // even when page redirects, manifests, or declared images stall.
    let primary = async {
        let page =
            tokio::time::timeout(Duration::from_secs(20), download(website, 2 * 1024 * 1024))
                .await??;
        fallback_base = page.url.clone();
        let html = std::str::from_utf8(&page.bytes).context("website HTML is not UTF-8")?;
        let (mut candidates, manifests) = html_candidates(html, &page.url);
        for manifest in manifests {
            if let Ok(data) = download(manifest, 256 * 1024).await
                && let Ok(value) = serde_json::from_slice::<Value>(&data.bytes)
                && let Some(icons) = value["icons"].as_array()
            {
                for icon in icons.iter().take(8) {
                    // Maskable-only images have a different safe area contract.
                    if icon["purpose"]
                        .as_str()
                        .is_some_and(|p| !p.split_whitespace().any(|p| p == "any"))
                    {
                        continue;
                    }
                    if let Some(src) = icon["src"].as_str()
                        && let Some(c) = candidate(&data.url, src, LogoKind::ManifestIcon)
                    {
                        candidates.push(c);
                    }
                }
            }
        }
        // Leave four image slots for fallback paths within the 24-image budget.
        candidates.sort_by_key(|c| c.priority);
        candidates.truncate(20);
        evaluate_candidates(candidates, &mut download, &mut seen).await
    };
    if let Ok(Ok(found)) = tokio::time::timeout(Duration::from_secs(75), primary).await {
        return Ok(found);
    }
    evaluate_candidates(conventional_icons(&fallback_base), &mut download, &mut seen).await
}

async fn evaluate_candidates<F, Fut>(
    mut candidates: Vec<Candidate>,
    download: &mut F,
    seen: &mut HashSet<String>,
) -> Result<Discovered>
where
    F: FnMut(Url, usize) -> Fut,
    Fut: std::future::Future<Output = Result<Download>>,
{
    candidates.sort_by_key(|c| c.priority);
    // Evaluate actual bytes, not the HTML's claimed type or size. Prefer a
    // higher resolution image within each source category (up to our target).
    let mut best: Option<(u8, u32, Discovered)> = None;
    for c in candidates.into_iter().take(24) {
        if best.as_ref().is_some_and(|(p, _, _)| c.priority > *p) {
            break;
        }
        if !seen.insert(c.url.to_string()) {
            continue;
        }
        let Ok(data) = download(c.url, 8 * 1024 * 1024).await else {
            continue;
        };
        let Ok(image) = tokio::task::spawn_blocking(move || images::prepare(data.bytes)).await?
        else {
            continue;
        };
        let quality = image.width.min(image.height).min(256);
        if best.as_ref().is_none_or(|(_, q, _)| quality > *q) {
            best = Some((
                c.priority,
                quality,
                Discovered {
                    source_url: data.url.to_string(),
                    kind: c.kind,
                    image,
                },
            ));
        }
    }
    best.map(|(_, _, d)| d)
        .context("no usable official logo or icon found")
}
#[cfg(test)]
mod tests {
    use super::*;
    fn png() -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(16, 16)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[tokio::test]
    async fn conventional_icons_work_when_page_fails_or_is_not_utf8() {
        for invalid_utf8 in [false, true] {
            let mut calls = Vec::new();
            let logo = discover_with_fetch("https://example.com/shop", None, |url, _| {
                calls.push(url.path().to_owned());
                let bytes = match url.path() {
                    "/shop" if invalid_utf8 => Some(vec![0xff]),
                    "/favicon.ico" => Some(png()),
                    _ => None,
                };
                std::future::ready(
                    bytes
                        .map(|bytes| Download { bytes, url })
                        .context("mock HTTP failure"),
                )
            })
            .await
            .unwrap();
            assert_eq!(logo.source_url, "https://example.com/favicon.ico");
            assert_eq!(logo.kind, LogoKind::Favicon);
            assert_eq!(
                calls,
                [
                    "/shop",
                    "/apple-touch-icon.png",
                    "/apple-touch-icon-precomposed.png",
                    "/favicon.ico",
                    "/favicon.png"
                ]
            );
        }
    }

    #[tokio::test]
    async fn declared_logos_win_and_broken_declared_images_fall_back_on_redirect_origin() {
        for broken in [false, true] {
            let mut calls = Vec::new();
            let logo = discover_with_fetch("https://example.com/shop", None, |url, _| {
                calls.push(url.to_string());
                let result = if url.path() == "/shop" {
                    Ok(Download {
                        bytes: br#"<link rel="icon" href="/declared.png">"#.to_vec(),
                        url: Url::parse("https://final.example/store").unwrap(),
                    })
                } else if url.path() == "/declared.png" {
                    Ok(Download {
                        bytes: if broken {
                            b"not an image".to_vec()
                        } else {
                            png()
                        },
                        url,
                    })
                } else if url.path() == "/apple-touch-icon.png" {
                    Ok(Download { bytes: png(), url })
                } else {
                    Err(anyhow::anyhow!("mock missing file"))
                };
                std::future::ready(result)
            })
            .await
            .unwrap();
            if broken {
                assert_eq!(
                    logo.source_url,
                    "https://final.example/apple-touch-icon.png"
                );
                assert_eq!(logo.kind, LogoKind::TouchIcon);
            } else {
                assert_eq!(logo.source_url, "https://final.example/declared.png");
                assert_eq!(calls.len(), 2);
            }
        }
    }

    #[tokio::test]
    async fn explicit_image_does_not_fetch_page_or_guessed_icons() {
        let mut calls = Vec::new();
        let result =
            discover_with_fetch("https://example.com/shop", Some("/brand.png"), |url, _| {
                calls.push(url.path().to_owned());
                std::future::ready(Err(anyhow::anyhow!("mock image failure")))
            })
            .await;
        assert!(result.is_err());
        assert_eq!(calls, ["/brand.png"]);
    }

    #[test]
    fn orders_official_sources_and_resolves_relative_links() {
        let html = r#"<script type="application/ld+json">{"@graph":[{"@type":"Organization","logo":{"url":"/brand.svg"}}]}</script><link rel="apple-touch-icon" href="touch.png"><link rel="shortcut icon" href="/favicon.ico"><link rel="manifest" href="site.webmanifest">"#;
        let (c, m) = html_candidates(html, &Url::parse("https://example.com/shop/").unwrap());
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].url.as_str(), "https://example.com/brand.svg");
        assert_eq!(c[1].url.as_str(), "https://example.com/shop/touch.png");
        assert_eq!(m[0].as_str(), "https://example.com/shop/site.webmanifest");
    }
    #[test]
    fn rejects_private_and_special_networks() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "100.100.100.200",
            "192.168.1.1",
            "0.0.0.0",
            "224.0.0.1",
            "198.18.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fe80::1",
            "fc00::1",
            "2001:db8::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
    }
}
