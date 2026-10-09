use anyhow::{Context, Result, bail, ensure};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};
use ultrafinance_core::{
    EnrichRequest, Enricher, enrich_locally, interpretation,
    store::{LOCAL_DATABASE_URL, MerchantStore, normalize},
};

const DESCRIPTION: &str = "SQ* JULIUS BROMONT";
const SAMPLES: usize = 30;
const MIN_BATCH: Duration = Duration::from_millis(10);

fn measure_jev(
    runtime: &tokio::runtime::Runtime,
    store: MerchantStore,
    request: &EnrichRequest,
    samples: usize,
) -> Result<()> {
    let key = std::env::var("TYPESAFE_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .context("set TYPESAFE_API_KEY to run --jev")?;
    let model = std::env::var("JEV_MODEL").unwrap_or_else(|_| "jev-latest".into());
    let threshold = std::env::var("ULTRAFINANCE_MATCH_THRESHOLD")
        .unwrap_or_else(|_| "0.90".into())
        .parse()
        .context("invalid match threshold")?;
    let enricher = Enricher::with_store(Some(key), model, threshold, store)?;
    println!(
        "End-to-end enrichment: {samples} requests, including configured providers and persistence (no calibration or extra warmup calls)."
    );
    let mut timings = Vec::with_capacity(samples);
    let mut provider_calls = 0;
    for index in 0..samples {
        let started = Instant::now();
        let (response, details) = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(55),
                enricher.enrich_with_details(request),
            )
            .await
            .context("end-to-end enrichment timed out")
        })?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        let response = response?;
        let calls = details["provider_requests"].as_array().map_or(0, Vec::len);
        provider_calls += calls;
        println!(
            "end_to_end request {}: {elapsed:.2} ms; method {}; Jev calls {calls}; merchant {}",
            index + 1,
            details["method"].as_str().unwrap_or("unknown"),
            match response.merchant {
                ultrafinance_core::MerchantResult::Matched { .. } => "matched",
                ultrafinance_core::MerchantResult::Unresolved { .. } => "unresolved",
            }
        );
        timings.push(elapsed);
    }
    timings.sort_by(f64::total_cmp);
    println!(
        "end_to_end mean {:.2} ms  p50 {:.2} ms  p95 {:.2} ms; {provider_calls} Jev calls total",
        timings.iter().sum::<f64>() / samples as f64,
        timings[samples / 2],
        timings[(samples * 95).div_ceil(100) - 1]
    );
    if provider_calls == 0 {
        println!(
            "The production path bypassed Jev for every request; these samples do not measure Jev latency."
        );
    }
    Ok(())
}

// Batch short operations to keep clock overhead small. Percentiles describe
// batch averages, not individual request tail latency.
fn measure(name: &str, mut operation: impl FnMut() -> Result<()>) -> Result<()> {
    let mut iterations = 1;
    loop {
        let started = Instant::now();
        for _ in 0..iterations {
            operation()?;
        }
        if started.elapsed() >= MIN_BATCH || iterations >= 65_536 {
            break;
        }
        iterations *= 2;
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        for _ in 0..iterations {
            operation()?;
        }
        samples.push(started.elapsed().as_secs_f64() * 1_000_000.0 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / SAMPLES as f64;
    println!(
        "{name:24} mean {mean:10.2} us  p50 {:10.2} us  p95 {:10.2} us  ({iterations} ops/sample)",
        samples[SAMPLES / 2],
        samples[(SAMPLES * 95).div_ceil(100) - 1]
    );
    Ok(())
}

fn main() -> Result<()> {
    let mut jev = false;
    let mut jev_samples = 3;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bench" => {} // Added by cargo bench for a custom harness.
            "--jev" => jev = true,
            "--jev-samples" => {
                jev_samples = args
                    .next()
                    .context("--jev-samples requires a count")?
                    .parse::<usize>()?;
                ensure!(
                    (1..=100).contains(&jev_samples),
                    "--jev-samples must be 1..100"
                );
                jev = true;
            }
            _ => bail!("unknown benchmark argument {arg}; use --jev [--jev-samples N]"),
        }
    }
    if jev {
        ensure!(
            std::env::var("TYPESAFE_API_KEY")
                .ok()
                .is_some_and(|key| !key.trim().is_empty()),
            "set TYPESAFE_API_KEY to run --jev"
        );
    }
    let request: EnrichRequest = serde_json::from_value(serde_json::json!({
        "description": DESCRIPTION,
    }))?;
    let runtime = tokio::runtime::Runtime::new()?;
    let database_url =
        std::env::var("ULTRAFINANCE_DATABASE_URL").unwrap_or_else(|_| LOCAL_DATABASE_URL.into());
    let database = reqwest::Url::parse(&database_url).context("invalid database URL")?;
    ensure!(
        matches!(
            database.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]")
        ),
        "enrichment benchmarks require a local PostgreSQL database"
    );
    // Connect to the existing catalog without migrations, fixture inserts, or
    // the provider/persistence behavior of Enricher::enrich.
    let store = MerchantStore::postgres(&database_url)?;
    println!("Descriptor: {DESCRIPTION:?} (description only)");
    println!("Read-only local enrichment; {SAMPLES} samples per stage.");
    let started = Instant::now();
    let initial = runtime.block_on(enrich_locally(&request, store.clone()))?;
    println!(
        "First local enrichment: {:.2} ms; {} candidates; merchant {}",
        started.elapsed().as_secs_f64() * 1000.0,
        initial.candidates.len(),
        match initial.response.merchant {
            ultrafinance_core::MerchantResult::Matched { .. } => "matched",
            ultrafinance_core::MerchantResult::Unresolved { .. } => "unresolved",
        }
    );
    if initial.candidates.is_empty() {
        println!(
            "No candidates: database timings measure an empty-result lookup. Populate the catalog for representative retrieval timings."
        );
    }
    measure("normalize", || {
        black_box(normalize(black_box(DESCRIPTION)));
        Ok(())
    })?;
    measure("interpret", || {
        black_box(interpretation::interpret(black_box(&request)));
        Ok(())
    })?;
    measure("retrieve", || {
        black_box(store.search_request(black_box(&request), 10)?);
        Ok(())
    })?;
    measure("enrich_locally", || {
        black_box(runtime.block_on(enrich_locally(black_box(&request), store.clone()))?);
        Ok(())
    })?;
    if jev {
        measure_jev(&runtime, store, &request, jev_samples)?;
    }
    Ok(())
}
