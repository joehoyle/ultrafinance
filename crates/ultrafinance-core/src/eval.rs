//! Labeled evaluation of candidate retrieval and completed merchant matching.
use crate::{EnrichRequest, Enricher, MerchantResult, store::MerchantStore};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub version: u32,
    pub name: String,
    pub cases: Vec<Case>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub request: EnrichRequest,
    pub expected: Expected,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expected {
    Matched { merchant: MerchantRef },
    Unresolved,
    Unlabeled,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MerchantRef {
    Local(LocalRef),
    External(ExternalRef),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalRef {
    pub merchant_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalRef {
    pub source: String,
    pub external_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Search,
    Enrich,
}
#[derive(Debug, Serialize)]
pub struct CaseResult {
    pub id: String,
    pub description: String,
    pub request: EnrichRequest,
    pub candidates: Vec<crate::store::Candidate>,
    pub search_trace: crate::store::SearchTrace,
    pub expected: Expected,
    pub expected_local_id: Option<String>,
    pub candidate_ids: Vec<String>,
    pub expected_rank: Option<usize>,
    pub predicted_id: Option<String>,
    pub matched: Option<bool>,
    pub correct: Option<bool>,
    pub error: Option<String>,
    /// In-memory enrichment evidence, including Jev answers and discovery shortlists.
    pub enrichment: Option<serde_json::Value>,
    pub latency_ms: f64,
}
#[derive(Debug, Serialize)]
pub struct Metrics {
    pub cases: usize,
    pub labeled_merchants: usize,
    pub labeled_unresolved: usize,
    pub unlabeled: usize,
    pub candidate_coverage: Option<f64>,
    pub unresolved: Option<usize>,
    pub missing_source_references: usize,
    pub retrieval_hits: usize,
    pub candidate_recall: Option<f64>,
    pub top1_recall: Option<f64>,
    pub cases_without_candidates: usize,
    pub errors: usize,
    pub matches: Option<usize>,
    pub correct_matches: Option<usize>,
    pub false_matches: Option<usize>,
    pub correct_unresolved: Option<usize>,
    pub accuracy: Option<f64>,
    pub match_precision: Option<f64>,
    pub merchant_recall: Option<f64>,
    pub match_rate: Option<f64>,
    pub median_latency_ms: f64,
    pub p95_latency_ms: f64,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub report_version: u32,
    pub started_at_unix: u64,
    pub suite: String,
    pub suite_fingerprint: String,
    pub code_version: String,
    pub code_fingerprint: String,
    pub mode: Mode,
    pub candidate_limit: usize,
    pub model: Option<String>,
    pub threshold: Option<f64>,
    pub metrics: Metrics,
    pub results: Vec<CaseResult>,
}

// Stable snapshot identifier, not a cryptographic integrity check.
pub fn fingerprint(bytes: &[u8]) -> String {
    let mut value = 0xcbf29ce484222325u64;
    for byte in bytes {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{value:016x}")
}

/// Fingerprint a file/stream without retaining its contents.
pub fn fingerprint_reader(mut reader: impl std::io::Read) -> Result<String> {
    let mut hash = 0xcbf29ce484222325u64;
    let mut buffer = [0; 65536];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        for byte in &buffer[..n] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("fnv1a64:{hash:016x}"))
}

/// Validate a suite before starting retrieval or provider calls.
pub fn parse_suite(contents: &str) -> Result<Suite> {
    let suite: Suite = serde_json::from_str(contents)?;
    if suite.version != 1 || suite.name.trim().is_empty() || suite.cases.is_empty() {
        bail!("eval suite must have version 1, a name, and at least one case");
    }
    let mut ids = HashSet::new();
    for case in &suite.cases {
        if case.id.trim().is_empty() || !ids.insert(&case.id) {
            bail!("eval case IDs must be nonblank and unique");
        }
        case.request.validate()?;
        if serde_json::to_vec(&case.request)?.len() > 65536 {
            bail!("eval request exceeds 64 KiB");
        }
        if let Expected::Matched { merchant } = &case.expected {
            match merchant {
                MerchantRef::Local(r) if r.merchant_id.trim().is_empty() => {
                    bail!("expected merchant ID must be nonblank")
                }
                MerchantRef::External(r)
                    if r.source.trim().is_empty() || r.external_id.trim().is_empty() =>
                {
                    bail!("expected source reference must be nonblank")
                }
                _ => {}
            }
        }
    }
    Ok(suite)
}

pub async fn run(
    contents: &str,
    store: MerchantStore,
    mode: Mode,
    api_key: Option<String>,
    model: String,
    threshold: f64,
) -> Result<Report> {
    run_with_progress(contents, store, mode, api_key, model, threshold, None).await
}

/// Evaluate cases while exposing completed-case counts to a progress display.
pub async fn run_with_progress(
    contents: &str,
    store: MerchantStore,
    mode: Mode,
    api_key: Option<String>,
    model: String,
    threshold: f64,
    progress: Option<&std::sync::atomic::AtomicUsize>,
) -> Result<Report> {
    let suite = parse_suite(contents)?;
    let started_at_unix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let enricher = if mode == Mode::Enrich {
        Some(Enricher::for_evaluation(
            api_key,
            model.clone(),
            threshold,
            store.clone(),
        )?)
    } else {
        None
    };
    let mut results = Vec::new();
    let mut cases = suite.cases.into_iter();
    loop {
        let mut batch_results = Vec::new();
        let mut inputs = Vec::new();
        let mut starts = Vec::new();
        for case in cases.by_ref().take(crate::batch::MAX_BATCH_ITEMS) {
            let started = Instant::now();
            let expected_local_id = match &case.expected {
                Expected::Unresolved | Expected::Unlabeled => None,
                Expected::Matched {
                    merchant: MerchantRef::Local(r),
                } => Some(r.merchant_id.clone()),
                Expected::Matched {
                    merchant: MerchantRef::External(r),
                } => store.resolve_source(&r.source, &r.external_id)?,
            };
            let db = store.clone();
            let request = case.request.clone();
            let (candidates, search_trace) =
                tokio::task::spawn_blocking(move || db.search_request_traced(&request, 10))
                    .await??;
            let candidate_ids: Vec<_> = candidates.iter().map(|c| c.merchant.id.clone()).collect();
            let expected_rank = expected_local_id
                .as_ref()
                .and_then(|id| candidate_ids.iter().position(|candidate| candidate == id))
                .map(|r| r + 1);
            let result = CaseResult {
                request: case.request.clone(),
                candidates: candidates.clone(),
                search_trace,
                id: case.id,
                description: case.request.description.clone(),
                expected: case.expected,
                expected_local_id,
                candidate_ids,
                expected_rank,
                predicted_id: None,
                matched: None,
                correct: None,
                error: None,
                enrichment: None,
                latency_ms: if enricher.is_none() {
                    started.elapsed().as_secs_f64() * 1000.0
                } else {
                    0.0
                },
            };
            inputs.push((case.request, Ok(candidates)));
            starts.push(started);
            batch_results.push(result);
        }
        if batch_results.is_empty() {
            break;
        }
        if let Some(enricher) = &enricher {
            let (outcomes, evidence) = enricher.enrich_batch_candidates_traced(inputs).await;
            for (index, (result, outcome)) in batch_results.iter_mut().zip(outcomes).enumerate() {
                result.enrichment = evidence.get(index).cloned();
                match outcome {
                    Ok(response) => {
                        result.predicted_id = match response.merchant {
                            MerchantResult::Matched { data } => Some(data.id),
                            MerchantResult::Unresolved { .. } => None,
                        };
                        result.matched = Some(result.predicted_id.is_some());
                        result.correct = match &result.expected {
                            Expected::Unresolved => Some(result.predicted_id.is_none()),
                            Expected::Unlabeled => None,
                            Expected::Matched { .. } => Some(
                                result.expected_local_id.is_some()
                                    && result.predicted_id == result.expected_local_id,
                            ),
                        };
                    }
                    Err(error) => result.error = Some(error.to_string()),
                }
            }
        }
        for (mut result, started) in batch_results.into_iter().zip(starts) {
            if enricher.is_some() {
                result.latency_ms = started.elapsed().as_secs_f64() * 1000.0;
            }
            results.push(result);
            if let Some(progress) = progress {
                progress.store(results.len(), std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    let metrics = summarize(&results, mode);
    Ok(Report {
        report_version: 1,
        started_at_unix,
        suite: suite.name,
        suite_fingerprint: fingerprint(contents.as_bytes()),
        code_version: env!("CARGO_PKG_VERSION").into(),
        code_fingerprint: fingerprint(
            concat!(
                include_str!("lib.rs"),
                include_str!("batch.rs"),
                include_str!("store.rs"),
                include_str!("postgres_store.rs"),
                include_str!("../migrations/001_postgres.sql"),
                include_str!("../migrations/011_search_words.sql"),
                include_str!("postgres_store/read_cache.rs"),
                include_str!("search_score.rs"),
                include_str!("search_pipeline.rs"),
                include_str!("interpretation.rs"),
                include_str!("bank_formats.rs"),
                include_str!("regex_rules.rs"),
                include_str!("import.rs"),
                include_str!("eval.rs"),
                include_str!("location.rs"),
                include_str!("../migrations/003_locations.sql")
            )
            .as_bytes(),
        ),
        mode,
        candidate_limit: 10,
        model: (mode == Mode::Enrich).then_some(model),
        threshold: (mode == Mode::Enrich).then_some(threshold),
        metrics,
        results,
    })
}
fn ratio(n: usize, d: usize) -> Option<f64> {
    (d > 0).then(|| n as f64 / d as f64)
}
pub fn summarize(results: &[CaseResult], mode: Mode) -> Metrics {
    let known = results
        .iter()
        .filter(|r| matches!(r.expected, Expected::Matched { .. }))
        .count();
    let labeled_unresolved = results
        .iter()
        .filter(|r| matches!(r.expected, Expected::Unresolved))
        .count();
    let labeled = known + labeled_unresolved;
    let labeled_matches = results
        .iter()
        .filter(|r| r.predicted_id.is_some() && !matches!(r.expected, Expected::Unlabeled))
        .count();
    let hits = results.iter().filter(|r| r.expected_rank.is_some()).count();
    let matches = results.iter().filter(|r| r.predicted_id.is_some()).count();
    let correct_matches = results
        .iter()
        .filter(|r| r.predicted_id.is_some() && r.correct == Some(true))
        .count();
    let correct_unresolved = results
        .iter()
        .filter(|r| matches!(r.expected, Expected::Unresolved) && r.correct == Some(true))
        .count();
    let mut latencies: Vec<_> = results.iter().map(|r| r.latency_ms).collect();
    latencies.sort_by(f64::total_cmp);
    let enrich = mode == Mode::Enrich;
    Metrics {
        cases: results.len(),
        labeled_merchants: known,
        labeled_unresolved,
        unlabeled: results.len() - labeled,
        candidate_coverage: ratio(
            results
                .iter()
                .filter(|r| !r.candidate_ids.is_empty())
                .count(),
            results.len(),
        ),
        unresolved: enrich.then_some(
            results
                .iter()
                .filter(|r| r.predicted_id.is_none() && r.error.is_none())
                .count(),
        ),
        missing_source_references: results
            .iter()
            .filter(|r| {
                matches!(
                    r.expected,
                    Expected::Matched {
                        merchant: MerchantRef::External(_)
                    }
                ) && r.expected_local_id.is_none()
            })
            .count(),
        retrieval_hits: hits,
        candidate_recall: ratio(hits, known),
        top1_recall: ratio(
            results
                .iter()
                .filter(|r| r.expected_rank == Some(1))
                .count(),
            known,
        ),
        cases_without_candidates: results
            .iter()
            .filter(|r| r.candidate_ids.is_empty())
            .count(),
        errors: results.iter().filter(|r| r.error.is_some()).count(),
        matches: enrich.then_some(matches),
        correct_matches: (enrich && labeled > 0).then_some(correct_matches),
        false_matches: (enrich && labeled > 0).then_some(labeled_matches - correct_matches),
        correct_unresolved: (enrich && labeled > 0).then_some(correct_unresolved),
        accuracy: enrich
            .then(|| ratio(correct_matches + correct_unresolved, labeled))
            .flatten(),
        match_precision: enrich
            .then(|| ratio(correct_matches, labeled_matches))
            .flatten(),
        merchant_recall: enrich.then(|| ratio(correct_matches, known)).flatten(),
        match_rate: enrich.then(|| ratio(matches, results.len())).flatten(),
        median_latency_ms: latencies[latencies.len() / 2],
        p95_latency_ms: latencies[(latencies.len() * 95).div_ceil(100).saturating_sub(1)],
    }
}

#[cfg(test)]
#[path = "../tests/unit/eval/tests.rs"]
mod tests;
