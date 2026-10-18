use anyhow::{Context, Result, bail};
use comfy_table::{Table, presets::UTF8_FULL_CONDENSED};
use serde_json::{Value, json};
use std::{io::Read, path::Path, time::Duration};
use ultrafinance_core::{EnrichRequest, Enricher, MerchantResult, batch::MAX_BATCH_ITEMS};

pub(crate) fn read_requests(path: &Path, args: &crate::EnrichArgs) -> Result<Vec<EnrichRequest>> {
    let reader: Box<dyn Read> = if path.as_os_str() == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(
            std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?,
        )
    };
    let mut contents = String::new();
    reader.take(1024 * 1024 + 1).read_to_string(&mut contents)?;
    if contents.len() > 1024 * 1024 {
        bail!("TXT input exceeds 1 MiB");
    }
    let mut template = args.clone();
    let mut requests = Vec::new();
    for (index, line) in contents.trim_start_matches('\u{feff}').lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        template.description = Some(line.to_owned());
        requests.push(
            crate::read_request(&template)
                .with_context(|| format!("invalid TXT line {}", index + 1))?,
        );
        if args
            .limit
            .is_some_and(|limit| requests.len() >= limit as usize)
        {
            break;
        }
    }
    if requests.is_empty() {
        bail!("TXT input contains no descriptions");
    }
    Ok(requests)
}

fn render_table(rows: &[Value]) -> String {
    let mut table = Table::new();
    table.load_style(UTF8_FULL_CONDENSED).set_header([
        "Original description",
        "Kind",
        "Matched counterparty",
    ]);
    for row in rows {
        let label = row["counterparties"][0]["name"]
            .as_str()
            .or_else(|| row["merchant"].as_str())
            .unwrap_or_else(|| {
                if row["status"] == "error" {
                    "Error (see stderr)"
                } else {
                    "Unresolved"
                }
            });
        table.add_row([
            crate::output::text(row["description"].as_str().unwrap_or_default()),
            crate::output::text(row["kind"].as_str().unwrap_or("—")),
            crate::output::text(label),
        ]);
    }
    table.to_string()
}

pub(crate) async fn run(
    enricher: &Enricher,
    requests: &[EnrichRequest],
    json_output: bool,
) -> Result<()> {
    let mut rows = Vec::with_capacity(requests.len());
    let mut errors = 0;
    for chunk in requests.chunks(MAX_BATCH_ITEMS) {
        // Catalog retrieval runs once per input, before shared provider batches.
        // Allow the same per-input budget as single enrich on large catalogs.
        let outcomes = tokio::time::timeout(
            Duration::from_secs(55 * chunk.len() as u64),
            enricher.enrich_batch(chunk),
        )
        .await;
        let outcomes = match outcomes {
            Ok(outcomes) => outcomes,
            Err(_) => chunk
                .iter()
                .map(|_| Err(anyhow::anyhow!("merchant batch evaluation timed out")))
                .collect(),
        };
        for (request, outcome) in chunk.iter().zip(outcomes) {
            let row = match outcome {
                Ok(response) => match response.merchant {
                    MerchantResult::Matched { data } => {
                        json!({"description": request.description, "kind": response.kind, "status": "matched", "merchant": data.name, "merchant_id": data.id, "counterparties": response.counterparties})
                    }
                    MerchantResult::Unresolved { .. } => {
                        json!({"description": request.description, "kind": response.kind, "status": if response.counterparties.is_empty() { "unresolved" } else { "matched" }, "merchant": null, "counterparties": response.counterparties})
                    }
                },
                Err(error) => {
                    errors += 1;
                    eprintln!(
                        "Row {}: {}",
                        rows.len() + 1,
                        crate::output::text(&error.to_string())
                    );
                    json!({"description": request.description, "status": "error", "merchant": null, "error": error.to_string()})
                }
            };
            rows.push(row);
        }
        eprintln!("Enriched {}/{} descriptions", rows.len(), requests.len());
    }
    let rendered = if json_output {
        serde_json::to_string_pretty(&json!({"results": rows}))?
    } else {
        render_table(&rows)
    };
    crate::output::write_output(&rendered)?;
    if errors > 0 {
        bail!("{errors} descriptions failed; partial results were printed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_distinguishes_unresolved_and_errors_and_sanitizes_terminal_controls() {
        let table = render_table(&[
            json!({"description":"RAW\u{1b}[31m", "merchant":"Coffee\nShop", "status":"matched"}),
            json!({"description":"UNKNOWN", "merchant":null, "status":"unresolved"}),
            json!({"description":"FAILED", "merchant":null, "status":"error"}),
        ]);
        assert!(table.contains("Original description"));
        assert!(table.contains("Coffee Shop"));
        assert!(table.contains("Unresolved"));
        assert!(table.contains("Error (see stderr)"));
        assert!(!table.contains('\u{1b}'));
    }
}
