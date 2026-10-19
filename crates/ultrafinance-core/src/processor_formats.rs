//! Researched, validated processor descriptor formats stored in the catalog.
use anyhow::{Context, Result, bail};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessorFormat {
    pub id: String,
    pub processor_id: String,
    /// Full-descriptor pattern with one named `merchant` capture for the suffix.
    pub pattern: String,
    pub sources: Vec<String>,
    pub reviewed_on: String,
    pub scope: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

impl ProcessorFormat {
    pub(crate) fn compile(&self) -> Result<Regex> {
        if self.pattern.is_empty() || self.pattern.len() > 4096 {
            bail!("processor format pattern must be 1–4096 bytes");
        }
        // Wrapping prevents partial matches and alternative branches escaping
        // anchors supplied by an imported pattern.
        let pattern = RegexBuilder::new(&format!(r"\A(?:{})\z", self.pattern))
            .size_limit(1024 * 1024)
            .build()
            .context("invalid or unsupported processor format regex")?;
        if !pattern.capture_names().any(|name| name == Some("merchant")) || pattern.is_match("") {
            bail!("processor format requires a nonempty named merchant capture");
        }
        Ok(pattern)
    }

    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty()
            || self.id.len() > 128
            || self.processor_id.trim().is_empty()
            || self.scope.trim().is_empty()
        {
            bail!("processor format requires an ID, processor ID and scope");
        }
        if self.sources.is_empty() || self.sources.len() > 16 {
            bail!("processor format requires 1–16 source URLs");
        }
        for source in &self.sources {
            let url = reqwest::Url::parse(source).context("invalid processor format source URL")?;
            if !matches!(url.scheme(), "https" | "http")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                bail!("processor format sources must be public HTTP(S) URLs without credentials");
            }
        }
        if self.reviewed_on.len() != 10
            || self.reviewed_on.bytes().enumerate().any(|(i, c)| {
                if matches!(i, 4 | 7) {
                    c != b'-'
                } else {
                    !c.is_ascii_digit()
                }
            })
        {
            bail!("processor format reviewed_on must use YYYY-MM-DD");
        }
        self.compile()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CapturedMerchant {
    pub merchant: String,
    /// UTF-8 byte offset where the merchant suffix starts in the original input.
    pub start: usize,
}

pub(crate) fn capture(regex: &Regex, description: &str) -> Option<CapturedMerchant> {
    let captures = regex.captures(description)?;
    let merchant = captures.name("merchant")?;
    // Only remove a prefix. Never silently drop trailing geography/references.
    if merchant.start() == 0
        || merchant.end() != description.len()
        || merchant.as_str().trim().is_empty()
    {
        return None;
    }
    Some(CapturedMerchant {
        merchant: merchant.as_str().trim().to_owned(),
        start: merchant.start(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatMatch {
    pub format: ProcessorFormat,
    pub processor: crate::Counterparty,
    pub capture: CapturedMerchant,
}

/// Validate rule syntax, provenance and unique IDs. Runtime matching rejects
/// ambiguous results; regression examples live in test fixtures.
pub fn validate_set(formats: &[ProcessorFormat]) -> Result<()> {
    let mut ids = std::collections::HashSet::new();
    for format in formats {
        format.validate()?;
        if !ids.insert(&format.id) {
            bail!("duplicate processor format ID");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/processor_formats/tests.rs"]
mod tests;
