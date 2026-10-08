use anyhow::Result;
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};
use std::io::{self, IsTerminal, Write};
use ultrafinance_core::store::MerchantPage;

pub fn enrichment_logs(logs: &[serde_json::Value], json: bool, offset: usize) -> Result<()> {
    let rendered = if json {
        serde_json::to_string_pretty(logs)?
    } else if logs.is_empty() {
        format!("No enrichment logs on this page (offset {offset}).")
    } else {
        let mut table = Table::new();
        table
            .load_style(UTF8_FULL_CONDENSED)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header([
                "Created (UTC)",
                "Status",
                "Description",
                "Merchant",
                "Method",
                "Error",
            ]);
        if !io::stdout().is_terminal() {
            table.set_width(140);
        }
        for log in logs {
            let field =
                |pointer: &str| text(log.pointer(pointer).and_then(|v| v.as_str()).unwrap_or("—"));
            let merchant = log
                .pointer("/data/response/merchant/data/name")
                .and_then(|v| v.as_str())
                .or_else(|| log["merchant_id"].as_str())
                .unwrap_or("—");
            table.add_row([
                field("/created_at"),
                field("/status"),
                field("/data/request/description"),
                text(merchant),
                field("/data/method"),
                field("/data/error"),
            ]);
        }
        format!(
            "{table}\n\nShowing {}–{} enrichment logs (newest first). Use --json for full records.",
            offset + 1,
            offset + logs.len()
        )
    };
    write_output(&rendered)
}

pub(crate) fn text(value: &str) -> String {
    // Imported data is display text, never terminal escape sequences.
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub fn merchant_list(page: &MerchantPage, json: bool) -> Result<()> {
    let rendered = if json {
        serde_json::to_string_pretty(page)?
    } else if page.merchants.is_empty() {
        format!(
            "No merchants on this page. {} total; offset {}.",
            page.total, page.offset
        )
    } else {
        let mut table = Table::new();
        table
            .load_style(UTF8_FULL_CONDENSED)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(["ID", "Name", "Country", "Website", "Aliases"]);
        if !io::stdout().is_terminal() {
            table.set_width(120);
        }
        for merchant in &page.merchants {
            table.add_row([
                text(&merchant.id),
                text(&merchant.name),
                text(merchant.country.as_deref().unwrap_or("—")),
                text(merchant.website.as_deref().unwrap_or("—")),
                merchant.aliases.len().to_string(),
            ]);
        }
        format!(
            "{table}\n\nShowing {}–{} of {} merchants",
            page.offset + 1,
            page.offset + page.merchants.len(),
            page.total
        )
    };
    write_output(&rendered)
}

fn write_output(rendered: &str) -> Result<()> {
    // Unix pipelines such as `logs | head` may close stdout early.
    match writeln!(io::stdout().lock(), "{rendered}") {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => return Err(error.into()),
        _ => {}
    }
    Ok(())
}
