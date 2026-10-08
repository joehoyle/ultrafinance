use anyhow::Result;
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};
use std::io::{self, IsTerminal, Write};
use ultrafinance_core::store::MerchantPage;

fn text(value: &str) -> String {
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
    // Unix pipelines such as `merchants list | head` may close stdout early.
    match writeln!(io::stdout().lock(), "{rendered}") {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => return Err(error.into()),
        _ => {}
    }
    Ok(())
}
