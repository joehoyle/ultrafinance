use anyhow::Result;
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};
use std::io::{self, IsTerminal, Write};
use ultrafinance_core::store::{Candidate, MerchantPage, MerchantStats};

pub fn enrichment(
    description: &str,
    response: &ultrafinance_core::EnrichResponse,
    json: bool,
) -> Result<()> {
    use ultrafinance_core::{MerchantResult, location::LocationResult};
    if json {
        return write_output(&serde_json::to_string_pretty(response)?);
    }
    let mut rendered = format!("Transaction: {}\n", text(description));
    match &response.merchant {
        MerchantResult::Matched { data } => {
            rendered.push_str(&format!("Merchant:    {}\n", text(&data.name)));
            if let Some(website) = &data.website {
                rendered.push_str(&format!("Website:     {}\n", text(website)));
            }
            if !data.markets.is_empty() {
                rendered.push_str(&format!(
                    "Markets:     {}\n",
                    text(&data.markets.join(", "))
                ));
            }
        }
        MerchantResult::Unresolved { .. } => rendered.push_str("Merchant:    Unresolved\n"),
    }
    match &response.location {
        LocationResult::Unresolved { .. } => rendered.push_str("Location:    Unresolved\n"),
        LocationResult::Matched { data } | LocationResult::Extracted { data } => {
            let label = if matches!(response.location, LocationResult::Matched { .. }) {
                "Matched outlet"
            } else {
                "Extracted geography"
            };
            let address = [
                &data.name,
                &data.address,
                &data.city,
                &data.region,
                &data.postal_code,
                &data.country,
            ]
            .into_iter()
            .flatten()
            .map(|v| text(v))
            .collect::<Vec<_>>()
            .join(", ");
            rendered.push_str(&format!("Location:    {label}\n"));
            if !address.is_empty() {
                rendered.push_str(&format!("             {address}\n"));
            }
            if let Some(number) = &data.store_number {
                rendered.push_str(&format!("Store:       {}\n", text(number)));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for credit in &response.attributions {
        if seen.insert(credit) {
            if seen.len() == 1 {
                rendered.push_str("\nAttributions:\n");
            }
            rendered.push_str(&format!("  • {}\n", text(credit)));
        }
    }
    write_output(rendered.trim_end())
}

pub fn interpretation_formats(
    inventory: &ultrafinance_core::interpretation::Formats,
    json: bool,
) -> Result<()> {
    if json {
        return write_output(&serde_json::to_string_pretty(inventory)?);
    }
    let mut rendered = format!("Descriptor rules ({})\n", inventory.formats.len());
    for (index, rule) in inventory.formats.iter().enumerate() {
        rendered.push_str(&format!("\n{:>2}. {}\n", index + 1, rule.id));
        // Wrap prose, but keep regexes intact for copying into other tools.
        let mut line = String::from("    ");
        for word in rule.description.split_whitespace() {
            if line.chars().count() + word.chars().count() + 1 > 92 {
                rendered.push_str(line.trim_end());
                rendered.push('\n');
                line = String::from("    ");
            }
            line.push_str(word);
            line.push(' ');
        }
        rendered.push_str(line.trim_end());
        rendered.push('\n');
        match rule.pattern {
            Some(pattern) => rendered.push_str(&format!("    Pattern:\n      {pattern}\n")),
            None => rendered.push_str("    Pattern: none (preservation rule)\n"),
        }
    }
    let mut processors = Table::new();
    processors.load_style(UTF8_FULL_CONDENSED).set_header([
        "Prefix (literal)",
        "Processor hint",
        "Prefix pattern",
    ]);
    for processor in inventory.processors {
        let code = regex::escape(processor.code);
        let separator = if processor.separator == ' ' {
            " ".to_owned()
        } else {
            format!(r"\s*{}", regex::escape(&processor.separator.to_string()))
        };
        processors.add_row([
            format!("\"{}{}\"", processor.code, processor.separator),
            processor.processor.to_owned(),
            format!(r"^\s*(?i-u:{code}){separator}\s*\S"),
        ]);
    }
    rendered.push_str(&format!(
        "\nProcessor prefixes ({})\nCase-insensitive; separators are required and nested prefixes are supported.\nQuoted prefixes make trailing spaces visible. Prefix patterns show the equivalent\nmatching condition; processor prefixes are parsed directly rather than with regex.\n\n{processors}\n\nUse --json for machine-readable definitions.",
        inventory.processors.len()
    ));
    rendered.push_str(&format!("\n\n{}\nUse locations gazetteer CITY [--country CODE] [--region REGION] to inspect matches.", gazetteer_summary(inventory.gazetteer)));
    write_output(&rendered)
}

fn gazetteer_summary(metadata: &serde_json::Value) -> String {
    format!(
        "Offline gazetteer: {}\nSnapshot: {} · {} places · {} countries · {} aliases\nSource: {} · License: {}\nRegion mappings: {} ISO mappings; {} regions retain full names.\nAliases: {}\nGeoNames IDs preserve ambiguity; these are locality clues, not outlet evidence.",
        metadata["source"].as_str().unwrap_or(""),
        metadata["snapshot_date"].as_str().unwrap_or(""),
        metadata["records"],
        metadata["countries"],
        metadata["aliases"],
        metadata["source_url"].as_str().unwrap_or(""),
        metadata["license"].as_str().unwrap_or(""),
        metadata["mapped_regions"],
        metadata["unmapped_regions"],
        metadata["alias_policy"].as_str().unwrap_or("")
    )
}

pub fn gazetteer(
    city: Option<&str>,
    country: Option<&str>,
    region: Option<&str>,
    json: bool,
) -> Result<()> {
    let metadata = ultrafinance_core::gazetteer::metadata();
    let matches = city
        .map(|city| ultrafinance_core::gazetteer::lookup(city, country, region))
        .unwrap_or_default();
    if json {
        return write_output(&serde_json::to_string_pretty(
            &serde_json::json!({"gazetteer":metadata,"matches":matches}),
        )?);
    }
    let mut rendered = gazetteer_summary(metadata);
    if let Some(city) = city {
        let mut table = Table::new();
        table.load_style(UTF8_FULL_CONDENSED).set_header([
            "GeoNames ID",
            "City",
            "Region",
            "Country",
        ]);
        for location in &matches {
            table.add_row([
                location.geoname_id.to_string(),
                text(location.city),
                text(location.region),
                location.country.into(),
            ]);
        }
        rendered.push_str(&format!(
            "\n\nLookup: {} · {} matches\n{table}",
            text(city),
            matches.len()
        ));
    }
    write_output(&rendered)
}

pub fn gazetteer_list(
    country: Option<&str>,
    region: Option<&str>,
    limit: usize,
    offset: usize,
    json: bool,
) -> Result<()> {
    let (total, matches) = ultrafinance_core::gazetteer::list(country, region, limit, offset);
    let metadata = ultrafinance_core::gazetteer::metadata();
    if json {
        return write_output(&serde_json::to_string_pretty(&serde_json::json!({
            "gazetteer": metadata, "total": total, "limit": limit, "offset": offset,
            "matches": matches
        }))?);
    }
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL_CONDENSED)
        .set_header(["GeoNames ID", "City", "Region", "Country"]);
    for location in &matches {
        table.add_row([
            location.geoname_id.to_string(),
            text(location.city),
            text(location.region),
            location.country.into(),
        ]);
    }
    let mut rendered = format!(
        "{}\n\nPlaces: {} shown · {total} matching · offset {offset}\n{table}",
        gazetteer_summary(metadata),
        matches.len()
    );
    let next = offset.saturating_add(matches.len());
    if !matches.is_empty() && next < total {
        rendered.push_str(&format!(
            "\nNext page: repeat this command with --offset {next}"
        ));
    }
    write_output(&rendered)
}

fn provider_answer(
    label: &str,
    answer: &serde_json::Value,
    candidates: Option<&Vec<serde_json::Value>>,
) -> String {
    let choice = answer["choice"].as_str().unwrap_or("unavailable");
    let choice_name = |choice: &str| {
        if choice == "none" {
            return "No merchant match".to_owned();
        }
        choice
            .strip_prefix("candidate_")
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| candidates.and_then(|candidates| candidates.get(index)))
            .and_then(|candidate| candidate["merchant"]["name"].as_str())
            .map(text)
            .unwrap_or_else(|| text(choice))
    };
    let percentage = |value: &serde_json::Value| {
        value
            .as_f64()
            .map(|value| format!("{:.1}%", value * 100.0))
            .unwrap_or_else(|| "unavailable".into())
    };
    let mut rendered = format!(
        "\n   {label}\n   Selected: {} ({})\n   Confidence: {} · selected probability: {}\n",
        choice_name(choice),
        text(choice),
        percentage(&answer["confidence"]),
        percentage(&answer["probabilities"][choice]),
    );
    if let Some(probabilities) = answer["probabilities"].as_object() {
        let mut probabilities: Vec<_> = probabilities.iter().collect();
        probabilities.sort_by(|(left_name, left), (right_name, right)| {
            right
                .as_f64()
                .unwrap_or(0.0)
                .total_cmp(&left.as_f64().unwrap_or(0.0))
                .then_with(|| left_name.cmp(right_name))
        });
        let mut table = Table::new();
        table
            .load_style(UTF8_FULL_CONDENSED)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_width(120)
            .set_header(["Selected", "Choice", "Merchant", "Probability"]);
        for (candidate, probability) in probabilities {
            table.add_row([
                if candidate == choice {
                    "✓".into()
                } else {
                    String::new()
                },
                text(candidate),
                choice_name(candidate),
                percentage(probability),
            ]);
        }
        rendered.push_str(&format!("{table}\n"));
    }
    rendered
}

pub fn enrichment_details(details: &serde_json::Value) -> Result<()> {
    let mut rendered = String::from("1. Interpret descriptor\n");
    let interpretation = &details["interpretation"];
    if let Some(processor) = interpretation["processor_hint"].as_str() {
        rendered.push_str(&format!("   Processor: {}\n", text(processor)));
    }
    if let Some(tokens) = interpretation["unverified_tokens"].as_array()
        && !tokens.is_empty()
    {
        rendered.push_str(&format!(
            "   Unverified metadata: {}\n",
            serde_json::to_string(tokens)?
        ));
    }
    if let Some(hypotheses) = interpretation["hypotheses"].as_array() {
        for hypothesis in hypotheses {
            rendered.push_str(&format!(
                "   Name: {}",
                text(hypothesis["merchant_text"].as_str().unwrap_or(""))
            ));
            if let Some(location) = hypothesis["possible_location"].as_str() {
                let validated = if hypothesis.get("location_hint").is_some() {
                    "validated place clue"
                } else {
                    "unvalidated text guess"
                };
                rendered.push_str(&format!(" · location: {} ({validated})", text(location)));
            }
            rendered.push('\n');
            if let Some(hint) = hypothesis.get("location_hint") {
                rendered.push_str(&format!("      Hint: {hint}"));
                if let Some(ids) = hypothesis.get("geoname_ids") {
                    rendered.push_str(&format!(" · GeoNames IDs: {ids}"));
                }
                rendered.push('\n');
            }
        }
    }
    rendered.push_str("\n2. Retrieve merchant candidates\n");
    let candidates = details["catalog_candidates"]
        .as_array()
        .or_else(|| details["candidates"].as_array());
    if let Some(candidates) = candidates {
        let mut table = Table::new();
        table.load_style(UTF8_FULL_CONDENSED).set_header([
            if details.get("provider_choices").is_some() {
                "Record"
            } else {
                "Choice"
            },
            "Merchant",
            "Similarity",
            "Exact",
            "Trusted",
            "Outlet evidence",
        ]);
        for (index, candidate) in candidates.iter().enumerate() {
            let outlets = candidate["interpretation_evidence"]
                .as_array()
                .map_or(0, |support| {
                    support
                        .iter()
                        .filter(|support| support["outlet"].is_object())
                        .count()
                });
            table.add_row([
                if details.get("provider_choices").is_some() {
                    format!("record_{index}")
                } else {
                    format!("candidate_{index}")
                },
                text(candidate["merchant"]["name"].as_str().unwrap_or("")),
                format!("{:.3}", candidate["score"].as_f64().unwrap_or(0.0)),
                candidate["exact"].to_string(),
                candidate["trusted"].to_string(),
                outlets.to_string(),
            ]);
        }
        rendered.push_str(&format!(
            "   {} shortlisted; similarity is not match confidence.\n{table}\n",
            candidates.len()
        ));
    }
    rendered.push_str("\n3. Evaluate merchant match\n");
    let method = details["method"].as_str().unwrap_or("unavailable");
    rendered.push_str(&format!("   Method: {}\n", text(method)));
    if matches!(method, "provider" | "discovery") {
        rendered.push_str(&format!(
            "   Model: {} · minimum confidence and probability: {}\n",
            text(details["model"].as_str().unwrap_or("")),
            details["threshold"]
        ));
    }
    if let Some(choices) = details["provider_choices"].as_array() {
        let mut table = Table::new();
        table.load_style(UTF8_FULL_CONDENSED).set_header([
            "Brand choice",
            "Merchant",
            "Catalog records",
            "Record resolution",
        ]);
        for (index, choice) in choices.iter().enumerate() {
            table.add_row([
                format!("candidate_{index}"),
                text(choice["merchant"]["name"].as_str().unwrap_or("")),
                choice["record_ids"]
                    .as_array()
                    .map_or(0, Vec::len)
                    .to_string(),
                text(choice["catalog_resolution"].as_str().unwrap_or("")),
            ]);
        }
        rendered.push_str(&format!(
            "   {} distinct merchant-name choices:\n{table}\n",
            choices.len()
        ));
    }
    if let Some(requests) = details["provider_requests"].as_array() {
        for request in requests {
            if request.get("body").is_none()
                && request["request_id"].is_string()
                && request["owner_log_id"].is_null()
            {
                rendered.push_str(&format!(
                    "\n   Shared Jev request {} · full body recorded once in the batch report\n",
                    text(request["request_id"].as_str().unwrap())
                ));
                continue;
            }
            if let Some(owner) = request["owner_log_id"].as_str() {
                rendered.push_str(&format!(
                    "\n   Shared Jev request {} ({}) · full body in log {}\n",
                    text(request["request_id"].as_str().unwrap_or("")),
                    text(request["status"].as_str().unwrap_or("unavailable")),
                    text(owner)
                ));
                continue;
            }
            rendered.push_str(&format!(
                "\n   Jev request body ({}, {} bytes):\n{}\n\n",
                text(request["status"].as_str().unwrap_or("unavailable")),
                serde_json::to_vec(&request["body"])?.len(),
                serde_json::to_string_pretty(&request["body"])?
            ));
        }
    } else {
        rendered.push_str("   No Jev request was sent.\n");
    }
    for (key, label) in [
        ("catalog_provider_answer", "Catalog answer"),
        ("provider_answer", "Provider answer"),
    ] {
        if let Some(answer) = details.get(key).filter(|answer| !answer.is_null()) {
            let answer_candidates = if key == "catalog_provider_answer" {
                details["catalog_provider_choices"]
                    .as_array()
                    .or(candidates)
            } else {
                details["provider_choices"]
                    .as_array()
                    .or_else(|| details["candidates"].as_array())
            };
            rendered.push_str(&provider_answer(label, answer, answer_candidates));
            if answer["catalog_resolution"] == "ambiguous" {
                rendered.push_str("   Merchant name selected, but catalog identity is ambiguous; no record matched.\n");
            }
        }
    }
    if details["discovery_attempted"] == true {
        rendered.push_str("   Discovery fallback attempted.\n");
        if let Some(candidates) = details["candidates"].as_array() {
            for (index, candidate) in candidates.iter().enumerate() {
                rendered.push_str(&format!(
                    "   Discovery candidate_{index}: {}\n",
                    text(candidate["merchant"]["name"].as_str().unwrap_or(""))
                ));
            }
        }
    }
    if let Some(reason) = details["discovery_skipped"].as_str() {
        rendered.push_str(&format!("   Discovery skipped: {}\n", text(reason)));
    }
    rendered.push_str(&format!(
        "   Outcome: {}\n",
        text(details["status"].as_str().unwrap_or("unavailable"))
    ));
    if let Some(error) = details["error"].as_str() {
        rendered.push_str(&format!("   Error: {}\n", text(error)));
    }
    if let Some(status) = details["response"]["location"]["status"].as_str() {
        rendered.push_str(&format!(
            "\n4. Resolve location independently\n   Outcome: {}\n",
            text(status)
        ));
        if status == "unresolved" && details["response"]["merchant"]["status"] == "unresolved" {
            rendered.push_str("   Merchant unresolved; no merchant outlet selected.\n");
        }
    }
    let mut stderr = io::stderr().lock();
    stderr.write_all(rendered.as_bytes())?;
    Ok(())
}

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

pub fn merchant_dedupe(report: &ultrafinance_core::dedupe::Report, json: bool) -> Result<()> {
    if json {
        return write_output(&serde_json::to_string_pretty(report)?);
    }
    write_output(&render_dedupe(report))
}

fn render_dedupe(report: &ultrafinance_core::dedupe::Report) -> String {
    let retired = report
        .groups
        .iter()
        .map(|group| group.len() - 1)
        .sum::<usize>();
    let heading = if report.dry_run {
        "Dedupe preview"
    } else {
        "Dedupe results"
    };
    let mut rendered = format!(
        "{heading}: {} merchants scanned · {} candidate pairs\n",
        report.merchants_scanned, report.candidates
    );
    if report.merchants.is_empty() {
        rendered.push_str("No duplicate candidates found.");
        return rendered;
    }
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header([
            "Pair",
            "Merchant",
            "Markets",
            "Result",
            "Surviving ID",
            "Pair decision",
            "Original ID",
        ]);
    if !io::stdout().is_terminal() {
        table.set_width(180);
    }
    // Keep IDs intact so they can be copied even when other columns wrap.
    for index in [4, 6] {
        table.column_mut(index).unwrap().set_constraint(
            comfy_table::ColumnConstraint::LowerBoundary(comfy_table::Width::Fixed(38)),
        );
    }
    for (pair_index, decision) in report.decisions.iter().enumerate() {
        let assessment = if decision.error.is_some() {
            "Invalid Jev answer".into()
        } else if decision.method == "rule" {
            "Name + website".into()
        } else {
            let choice = decision.answer["choice"].as_str().unwrap_or("unknown");
            let below = if choice == "same" && !decision.accepted {
                " (below threshold)"
            } else {
                ""
            };
            let confidence = decision.answer["confidence"]
                .as_f64()
                .map(|v| format!("\nConfidence {:.1}%", v * 100.0))
                .unwrap_or_default();
            format!("Jev: {choice}{below}{confidence}")
        };
        for (side, id) in [&decision.left, &decision.right].into_iter().enumerate() {
            let Some(merchant) = report.merchants.iter().find(|m| &m.id == id) else {
                continue;
            };
            let group = report.groups.iter().find(|g| g.contains(&merchant.id));
            let target = group.map(|g| g[0].as_str());
            let survivor = target == Some(merchant.id.as_str());
            let status = if report.errors > 0 && target.is_some() {
                "Not applied (errors)"
            } else {
                match (target, survivor, report.dry_run) {
                    (None, _, _) => "Kept separate",
                    (Some(_), true, true) => "Would keep (survivor)",
                    (Some(_), false, true) => "Would merge",
                    (Some(_), true, false) => "Kept (survivor)",
                    (Some(_), false, false) => "Merged",
                }
            };
            let markets = if survivor {
                let group = group.unwrap();
                report
                    .merchants
                    .iter()
                    .filter(|m| group.contains(&m.id))
                    .flat_map(|m| m.markets.iter())
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            } else {
                merchant.markets.clone()
            };
            table.add_row([
                if side == 0 {
                    (pair_index + 1).to_string()
                } else {
                    String::new()
                },
                text(&merchant.name),
                text(&markets.join(", ")),
                status.into(),
                text(target.unwrap_or("—")),
                if side == 0 {
                    text(&assessment)
                } else {
                    String::new()
                },
                text(&merchant.id),
            ]);
        }
    }
    rendered.push_str(&table.to_string());
    let verb = if report.errors > 0 {
        "not applied"
    } else if report.dry_run {
        "would merge"
    } else {
        "merged"
    };
    rendered.push_str(&format!("\n\n{} groups · {retired} records {verb}. Survivor rows show combined markets.\nEach pair is shown together; merchants may appear in multiple pairs. Use --json for full pair decisions.", report.groups.len()));
    if report.errors > 0 {
        rendered.push_str("\n\nInvalid Jev answers (no merges applied):\n");
        for decision in report.decisions.iter().filter(|d| d.error.is_some()) {
            let name = |id: &str| {
                report
                    .merchants
                    .iter()
                    .find(|m| m.id == id)
                    .map(|m| text(&m.name))
                    .unwrap_or_else(|| text(id))
            };
            rendered.push_str(&format!(
                "  {} [{}] ↔ {} [{}]: {}\n    Jev answer: {}\n",
                name(&decision.left),
                text(&decision.left),
                name(&decision.right),
                text(&decision.right),
                text(decision.error.as_deref().unwrap()),
                decision.answer
            ));
        }
    }
    rendered
}

pub fn merchant_search(description: &str, candidates: &[Candidate], json: bool) -> Result<()> {
    if json {
        return write_output(&serde_json::to_string_pretty(candidates)?);
    }
    if candidates.is_empty() {
        return write_output(&format!("No candidates found for {}.", text(description)));
    }
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header([
            "#",
            "Merchant",
            "Search similarity",
            "Match type",
            "Markets",
            "Website",
            "Merchant ID",
        ]);
    if !io::stdout().is_terminal() {
        table.set_width(140);
    }
    for (index, candidate) in candidates.iter().enumerate() {
        let kind = if candidate.resolution_id.is_some() {
            "Descriptor"
        } else if candidate.exact {
            "Exact"
        } else if candidate.regex_match_length.is_some() {
            "Pattern"
        } else {
            "Fuzzy"
        };
        table.add_row([
            (index + 1).to_string(),
            text(&candidate.merchant.name),
            format!("{:.3}", candidate.score),
            kind.into(),
            text(&candidate.merchant.markets.join(", ")),
            text(candidate.merchant.website.as_deref().unwrap_or("—")),
            text(&candidate.merchant.id),
        ]);
    }
    write_output(&format!(
        "Search: {}\n{table}\n\n{} candidates. Scores measure search similarity; Jev was not called. Use --json for full evidence.",
        text(description),
        candidates.len()
    ))
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
            .set_header(["ID", "Name", "Markets", "Website", "Aliases"]);
        if !io::stdout().is_terminal() {
            table.set_width(120);
        }
        for merchant in &page.merchants {
            table.add_row([
                text(&merchant.id),
                text(&merchant.name),
                text(&merchant.markets.join(", ")),
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

pub fn merchant_stats(stats: &MerchantStats, json: bool) -> Result<()> {
    if json {
        return write_output(&serde_json::to_string_pretty(stats)?);
    }
    let mut sources = Table::new();
    sources
        .load_style(UTF8_FULL_CONDENSED)
        .set_header(["Source", "Merchants", "Source records"]);
    for source in &stats.by_source {
        sources.add_row([
            text(&source.source),
            source.merchants.to_string(),
            source.records.to_string(),
        ]);
    }
    let mut markets = Table::new();
    markets
        .load_style(UTF8_FULL_CONDENSED)
        .set_header(["Market", "Merchants"]);
    for market in &stats.by_market {
        markets.add_row([text(&market.market), market.merchants.to_string()]);
    }
    let mut regions = Table::new();
    regions.load_style(UTF8_FULL_CONDENSED).set_header([
        "Source",
        "Dataset region",
        "Merchants",
        "Source records",
    ]);
    for region in &stats.by_source_region {
        regions.add_row([
            text(&region.source),
            text(&region.region),
            region.merchants.to_string(),
            region.records.to_string(),
        ]);
    }
    write_output(&format!(
        "Total merchants: {}\nManual entries / corrections: {}\nWithout imported source: {}\nNo known markets: {}\n\nBy source\n{sources}\n\nCounts are distinct per source; a merchant linked to multiple sources appears in each.\n\nBy known market\n{markets}\n\nMarkets are known coverage, not exhaustive. A merchant can count in multiple markets.\n\nBy source dataset region\n{regions}",
        stats.total, stats.manual, stats.without_source, stats.without_markets
    ))
}

fn write_output(rendered: &str) -> Result<()> {
    // Unix pipelines such as `logs | head` may close stdout early.
    match writeln!(io::stdout().lock(), "{rendered}") {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => return Err(error.into()),
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod dedupe_tests {
    use super::*;
    use ultrafinance_core::dedupe::{Decision, Report};

    #[test]
    fn candidate_pairs_are_adjacent_even_when_catalog_ids_interleave() {
        let merchants = serde_json::from_value(serde_json::json!([
            {"id":"a","name":"Pair A left"},
            {"id":"b","name":"Pair B left"},
            {"id":"c","name":"Pair A right"},
            {"id":"d","name":"Pair B right"}
        ]))
        .unwrap();
        let decisions = [("a", "c", "related"), ("b", "d", "different")]
            .into_iter()
            .map(|(left, right, choice)| Decision {
                left: left.into(),
                right: right.into(),
                accepted: false,
                method: "jev".into(),
                error: None,
                answer: serde_json::json!({"choice":choice,"confidence":0.99}),
            })
            .collect();
        let report = Report {
            dry_run: false,
            model: "test".into(),
            threshold: 0.98,
            candidates: 2,
            errors: 0,
            merchants_scanned: 4,
            merchants,
            groups: vec![],
            decisions,
            run_id: None,
            details_truncated: false,
        };
        let table = render_dedupe(&report);
        let positions: Vec<_> = ["Pair A left", "Pair A right", "Pair B left", "Pair B right"]
            .iter()
            .map(|name| table.find(name).unwrap())
            .collect();
        assert!(positions.windows(2).all(|p| p[0] < p[1]));
        assert!(table.contains("Jev: related"));
        assert!(table.contains("Jev: different"));
        assert!(table.contains("Kept separate"));
    }
}
