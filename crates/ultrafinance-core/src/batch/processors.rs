//! Deterministic, researched processor formats stored in the catalog.
use super::*;

pub(super) fn merchant_request(request: &EnrichRequest, detail: &Value) -> (EnrichRequest, Value) {
    let mut merchant = request.clone();
    let mut descriptor = json!({"original": request.description, "text": request.description, "status": "unchanged"});
    if detail["status"] != "matched" || detail["method"] != "format" {
        return (merchant, descriptor);
    }
    let Some(start) = detail["merchant_capture"]["start"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
    else {
        return (merchant, descriptor);
    };
    let Some(text) = detail["merchant_capture"]["merchant"].as_str() else {
        return (merchant, descriptor);
    };
    if start == 0
        || start >= request.description.len()
        || !request.description.is_char_boundary(start)
        || request.description[start..].trim() != text
        || text.is_empty()
    {
        return (merchant, descriptor);
    }
    merchant.description = text.to_owned();
    descriptor["status"] = json!("processor_removed");
    descriptor["text"] = json!(text);
    descriptor["removed_span"] =
        json!({"start":0,"end":start,"text":&request.description[..start]});
    (merchant, descriptor)
}

impl Enricher {
    pub(super) async fn resolve_processors(
        &self,
        requests: &[EnrichRequest],
        results: &mut [Result<EnrichResponse>],
        audit: &mut [(String, String, Value)],
    ) {
        let store = self.store.clone();
        let descriptions: Vec<_> = requests
            .iter()
            .map(|request| request.description.clone())
            .collect();
        let matches =
            tokio::task::spawn_blocking(move || store.processor_format_matches(&descriptions))
                .await;
        let matches = match matches {
            Ok(Ok(matches)) => matches,
            _ => {
                for (_, _, data) in audit.iter_mut() {
                    data["processor_resolution"] =
                        json!({"status":"retrieval_error", "method":"format"});
                }
                return;
            }
        };
        for (index, found) in matches.into_iter().enumerate() {
            if requests[index].validate().is_err() || results[index].is_err() {
                continue;
            }
            let detail = &mut audit[index].2["processor_resolution"];
            // Equivalent overlapping rules for one identity/capture are harmless;
            // different identities or extractions are ambiguous and never strip.
            let mut unique = Vec::new();
            for item in &found {
                let key = (
                    item.processor.id.as_str(),
                    item.capture.start,
                    item.capture.merchant.as_str(),
                );
                if !unique.contains(&key) {
                    unique.push(key);
                }
            }
            if unique.len() > 1 {
                *detail = json!({"status":"ambiguous_format", "method":"format", "matches":found});
                continue;
            }
            if let Some(matched) = found.first() {
                *detail = json!({"status":"matched", "method":"format", "format":matched.format,
                    "counterparty":matched.processor, "merchant_capture":matched.capture,
                    "matching_format_ids":found.iter().map(|matched| &matched.format.id).collect::<Vec<_>>()});
                let response = results[index].as_mut().unwrap();
                response.counterparties.push(matched.processor.clone());
                for source in &matched.format.sources {
                    let credit = format!("{} descriptor format ({source})", matched.processor.name);
                    if !response.attributions.contains(&credit) {
                        response.attributions.push(credit);
                    }
                }
                continue;
            }
            *detail = json!({"status":"no_format", "method":"format"});
            // Preserve reviewed, whole-descriptor identity matches. This does
            // not identify a wrapper and never removes any descriptor text.
            let store = self.store.clone();
            let request = requests[index].clone();
            let candidates = tokio::task::spawn_blocking(move || {
                store.search_typed(
                    &request.description,
                    request.country.as_deref(),
                    32,
                    Some(CounterpartyType::PaymentProcessor),
                )
            })
            .await;
            if let Ok(Ok(candidates)) = candidates
                && exact_match(&requests[index], &candidates)
            {
                let mut processor = unresolved();
                processor.merchant = MerchantResult::Matched {
                    data: Box::new(candidates[0].merchant.clone()),
                };
                let processor = attributed(processor, &candidates);
                *detail = json!({"status":"matched", "method":"exact", "counterparty":candidates[0].merchant});
                let response = results[index].as_mut().unwrap();
                response.counterparties.push(candidates[0].merchant.clone());
                response.attributions.extend(processor.attributions);
            }
        }
    }
}
