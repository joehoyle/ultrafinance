use anyhow::{Context, Result, bail};
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};
use ultrafinance_core::{
    datasets::Sample,
    eval::{self, Metrics, Mode},
    store::MerchantStore,
};

pub struct Options {
    pub datasets_dir: PathBuf,
    pub suites_dir: PathBuf,
    pub output: PathBuf,
    pub database_url: Option<String>,
    pub mode: Mode,
    pub limit: Option<u32>,
    pub model: String,
    pub threshold: f64,
}
struct Job {
    name: String,
    path: PathBuf,
    samples: bool,
}

pub fn contents(file: &Path, samples: bool, limit: Option<u32>) -> Result<String> {
    let input =
        fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    if !samples && limit.is_none() {
        return Ok(input);
    }
    let mut suite: Value = if samples {
        let cases: Vec<_> = input.lines().filter(|line| !line.trim().is_empty()).map(|line| -> Result<Value> {
            let sample: Sample = serde_json::from_str(line)?;
            Ok(json!({"id":sample.id,"request":sample.request,"expected":sample.expected.unwrap_or_else(|| json!({"status":"unlabeled"}))}))
        }).collect::<Result<_>>()?;
        json!({"version":1,"name":file.display().to_string(),"cases":cases})
    } else {
        serde_json::from_str(&input)?
    };
    if let Some(limit) = limit
        && let Some(cases) = suite["cases"].as_array_mut()
    {
        cases.truncate(limit as usize);
    }
    Ok(serde_json::to_string(&suite)?)
}
fn entries(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let mut paths = fs::read_dir(path)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    Ok(paths)
}
fn discover(suites: &Path, datasets: &Path) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    for path in entries(suites)? {
        if path.is_file() && path.extension().is_some_and(|s| s == "json") {
            jobs.push(Job {
                name: format!("suite/{}", path.file_stem().unwrap().to_string_lossy()),
                path,
                samples: false,
            });
        }
    }
    for source in entries(datasets)?.into_iter().filter(|p| p.is_dir()) {
        let mut latest: BTreeMap<String, (SystemTime, PathBuf)> = BTreeMap::new();
        for version in entries(&source)?.into_iter().filter(|p| p.is_dir()) {
            if version
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            let manifest = version.join("manifest.json");
            if !manifest.is_file() {
                continue;
            }
            let metadata: Value = serde_json::from_str(&fs::read_to_string(&manifest)?)
                .with_context(|| format!("invalid manifest {}", manifest.display()))?;
            let modified = fs::metadata(&manifest)?.modified()?;
            let region = metadata["region"].as_str().unwrap_or("global").to_string();
            if latest
                .get(&region)
                .is_none_or(|(time, path)| (modified, &version) > (*time, path))
            {
                latest.insert(region, (modified, version));
            }
        }
        for (region, (_, version)) in latest {
            let manifest: Value =
                serde_json::from_str(&fs::read_to_string(version.join("manifest.json"))?)?;
            if manifest["holdout_samples"].as_u64() == Some(0) {
                continue;
            }
            let path = version.join("holdout.jsonl");
            if !path.is_file() {
                bail!("missing holdout in latest snapshot {}", version.display());
            }
            jobs.push(Job {
                name: format!(
                    "dataset/{}{}",
                    source.file_name().unwrap().to_string_lossy(),
                    if region == "global" {
                        String::new()
                    } else {
                        format!("/{region}")
                    }
                ),
                path,
                samples: true,
            });
        }
    }
    if jobs.is_empty() {
        bail!("no eval suites or prepared holdouts found");
    }
    Ok(jobs)
}
fn percentage(value: Option<f64>) -> String {
    value
        .map(|v| format!("{:.1}%", v * 100.0))
        .unwrap_or_else(|| "-".into())
}
fn row(name: &str, m: &Metrics) -> Vec<String> {
    vec![
        crate::output::text(name),
        m.cases.to_string(),
        (m.labeled_merchants + m.labeled_unresolved).to_string(),
        percentage(m.candidate_coverage),
        percentage(m.candidate_recall),
        percentage(m.match_rate),
        percentage(m.accuracy),
        m.errors.to_string(),
    ]
}

pub async fn run(options: Options) -> Result<()> {
    let jobs = discover(&options.suites_dir, &options.datasets_dir)?;
    // Validate every input before any provider usage.
    let inputs = jobs
        .iter()
        .map(|job| -> Result<String> {
            let input = contents(&job.path, job.samples, options.limit)?;
            eval::parse_suite(&input)?;
            Ok(input)
        })
        .collect::<Result<Vec<_>>>()?;
    let store = MerchantStore::configured(options.database_url.as_deref())?;
    let directory = options.output.join(format!("run-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&directory)?;
    let terminal = io::stderr().is_terminal();
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header([
            "Suite",
            "Cases",
            "Labeled",
            "Candidates",
            "Recall",
            "Match rate",
            "Accuracy",
            "Errors",
        ]);
    if !io::stdout().is_terminal() {
        table.set_width(140);
    }
    let mut results = Vec::new();
    let mut summaries = Vec::new();
    let mut failures = BTreeMap::new();
    for (index, (job, input)) in jobs.iter().zip(inputs).enumerate() {
        eprintln!(
            "[{}/{}] {}{}",
            index + 1,
            jobs.len(),
            crate::output::text(&job.name),
            options
                .limit
                .map(|n| format!(" (limit {n})"))
                .unwrap_or_default()
        );
        let started = Instant::now();
        let completed = std::sync::atomic::AtomicUsize::new(0);
        let count = eval::parse_suite(&input)?.cases.len();
        let future = eval::run_with_progress(
            &input,
            store.clone(),
            options.mode,
            env::var("TYPESAFE_API_KEY").ok(),
            options.model.clone(),
            options.threshold,
            Some(&completed),
        );
        tokio::pin!(future);
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let outcome = loop {
            tokio::select! {
                report = &mut future => break report,
                _ = interval.tick(), if terminal => {
                    eprint!("\r  Running... {}/{} cases · {}s", completed.load(std::sync::atomic::Ordering::Relaxed), count, started.elapsed().as_secs());
                    let _ = io::stderr().flush();
                }
            }
        };
        if terminal {
            eprint!("\r\x1b[2K");
        }
        match outcome {
            Ok(mut report) => {
                if job.name == "dataset/lunchmoney" || job.name.starts_with("dataset/lunchmoney/") {
                    report.purchases_only();
                    print_purchase_filter(&report);
                }
                let path = directory.join(format!("{:02}.json", index + 1));
                fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
                table.add_row(row(&job.name, &report.metrics));
                summaries.push(json!({"name":job.name,"input":job.path,"report":path,"metrics":report.metrics}));
                results.extend(report.results);
                eprintln!(
                    "  Done: {} cases in {:.1}s",
                    report.metrics.cases,
                    started.elapsed().as_secs_f64()
                );
            }
            Err(error) => {
                let message = format!("{error:#}");
                eprintln!("  Failed: {}", crate::output::text(&message));
                table.add_row(vec![
                    crate::output::text(&job.name),
                    "-".into(),
                    "-".into(),
                    "-".into(),
                    "-".into(),
                    "-".into(),
                    "-".into(),
                    "FAILED".into(),
                ]);
                failures.insert(job.name.clone(), message);
            }
        }
    }
    let metrics = if results.is_empty() {
        None
    } else {
        Some(eval::summarize(&results, options.mode))
    };
    if let Some(metrics) = &metrics {
        table.add_row(row("Total", metrics));
    }

    let summary = directory.join("summary.json");
    fs::write(
        &summary,
        serde_json::to_vec_pretty(
            &json!({"version":1,"mode":options.mode,"limit_per_suite":options.limit,"metrics":metrics,"suites":summaries,"failures":failures}),
        )?,
    )?;
    let rendered = format!(
        "{table}\n\nCandidates: any candidate retrieved. Recall: correct merchant retrieved in labeled merchant cases.\nMatch rate: returned matches. Accuracy: labeled cases only. '-' means not measured.\nReports: {}",
        directory.display()
    );
    match writeln!(io::stdout().lock(), "{rendered}") {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => return Err(error.into()),
        _ => {}
    }
    if !failures.is_empty() || metrics.as_ref().is_some_and(|m| m.errors > 0) {
        bail!("some evaluations failed; see {}", summary.display());
    }
    Ok(())
}

/// Shared single-suite runner for file-based and registered-source evaluations.
pub struct FileOptions {
    pub purchases_only: bool,
    pub file: PathBuf,
    pub samples: bool,
    pub details: bool,
    pub limit: Option<u32>,
    pub output: Option<PathBuf>,
    pub database_url: Option<String>,
    pub mode: Mode,
    pub model: String,
    pub threshold: f64,
}
pub async fn run_file(options: FileOptions) -> Result<()> {
    let FileOptions {
        purchases_only,
        file,
        samples,
        details,
        limit,
        output,
        database_url,
        mode,
        model,
        threshold,
    } = options;
    let contents = contents(&file, samples, limit)?;
    let mut report = ultrafinance_core::eval::run(
        &contents,
        MerchantStore::configured(database_url.as_deref())?,
        mode,
        env::var("TYPESAFE_API_KEY").ok(),
        model,
        threshold,
    )
    .await?;
    if purchases_only {
        report.purchases_only();
        print_purchase_filter(&report);
    }
    let json = serde_json::to_string_pretty(&report)?;
    if let Some(path) = output {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &json)?;
        eprintln!("Saved report to {}", path.display());
    } else {
        println!("{}", crate::result_json::pretty(&report)?);
    }
    if details {
        print_details(&report)?;
    }
    eprintln!("{}", render_eval_tables(&report));
    Ok(())
}

fn render_eval_tables(report: &eval::Report) -> String {
    let mut results = Table::new();
    results
        .load_style(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_width(140)
        .set_header([
            "Description",
            "Kind",
            "Merchant",
            "Outcome",
            "Stopped at",
            "Reason",
        ]);
    for case in &report.results {
        let detail = case.enrichment.as_ref().unwrap_or(&Value::Null);
        let merchant = detail["response"]["merchant"]["data"]["name"]
            .as_str()
            .or_else(|| {
                case.candidates
                    .iter()
                    .find(|candidate| Some(&candidate.merchant.id) == case.predicted_id.as_ref())
                    .map(|candidate| candidate.merchant.name.as_str())
            })
            .unwrap_or("—");
        let outcome = if case.error.is_some() {
            "Error"
        } else if report.mode == Mode::Search {
            "Search only"
        } else if case.predicted_id.is_some() {
            "Matched"
        } else {
            "Unresolved"
        };
        results.add_row([
            crate::output::text(&case.description),
            crate::output::text(detail["response"]["kind"].as_str().unwrap_or("—")),
            crate::output::text(merchant),
            outcome.into(),
            case.failure
                .as_ref()
                .map(|f| f.stage.replace('_', " "))
                .unwrap_or_else(|| "—".into()),
            case.failure
                .as_ref()
                .map(|f| f.reason.replace('_', " "))
                .unwrap_or_else(|| "—".into()),
        ]);
    }
    let m = &report.metrics;
    let mut summary = Table::new();
    summary
        .load_style(UTF8_FULL_CONDENSED)
        .set_header(["Metric", "Result"]);
    if let Some(filter) = &report.purchase_filter {
        summary.add_row(["Total inputs".into(), filter.total_cases.to_string()]);
        summary.add_row([
            "Excluded (non-purchase / unknown / error)".into(),
            filter.excluded_results.len().to_string(),
        ]);
        summary.add_row([
            "Errors among excluded".into(),
            filter.excluded_errors.to_string(),
        ]);
    }
    summary.add_row([
        if report.purchase_filter.is_some() {
            "Predicted purchases"
        } else {
            "Evaluated cases"
        }
        .into(),
        m.cases.to_string(),
    ]);
    if let Some(matches) = m.matches {
        summary.add_row(["Matched".into(), matches.to_string()]);
        summary.add_row(["Unresolved".into(), m.unresolved.unwrap_or(0).to_string()]);
        summary.add_row(["Match coverage".into(), percentage(m.match_rate)]);
    }
    summary.add_row(["Errors".into(), m.errors.to_string()]);
    summary.add_row([
        "Candidate coverage".into(),
        percentage(m.candidate_coverage),
    ]);
    if m.labeled_merchants + m.labeled_unresolved > 0 {
        summary.add_row(["Accuracy".into(), percentage(m.accuracy)]);
        summary.add_row(["Match precision".into(), percentage(m.match_precision)]);
        summary.add_row(["Candidate recall".into(), percentage(m.candidate_recall)]);
    }
    let note = if m.unlabeled > 0 {
        "\nUnlabeled cases measure coverage, not accuracy."
    } else {
        ""
    };
    let mut funnel = Table::new();
    funnel.load_style(UTF8_FULL_CONDENSED).set_header([
        "Matching funnel",
        "Remaining",
        "Dropped",
        "% of evaluated",
    ]);
    for step in &m.funnel {
        funnel.add_row([
            step.stage.replace('_', " "),
            step.remaining.to_string(),
            step.dropped.to_string(),
            percentage((m.cases > 0).then(|| step.remaining as f64 / m.cases as f64)),
        ]);
    }
    let mut stages = Table::new();
    stages
        .load_style(UTF8_FULL_CONDENSED)
        .set_header(["Stopped at", "Reason", "Cases"]);
    for (key, count) in &m.failures_by_reason {
        let (stage, reason) = key.split_once(':').unwrap();
        stages.add_row([
            stage.replace('_', " "),
            reason.replace('_', " "),
            count.to_string(),
        ]);
    }
    let breakdown = if m.failures_by_reason.is_empty() {
        String::new()
    } else {
        format!("\n\n{stages}")
    };
    format!("{results}\n\n{summary}\n\n{funnel}{breakdown}{note}")
}

/// Render retrieval scores separately from the provider's assessment.
fn print_details(report: &eval::Report) -> Result<()> {
    let mut stdout = io::stdout().lock();
    for (index, result) in report.results.iter().enumerate() {
        let rendered = render_case(index, result, report.mode)?;
        if let Err(error) = writeln!(stdout, "{rendered}") {
            if error.kind() == io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(error.into());
        }
    }
    Ok(())
}

fn percent(value: &Value) -> String {
    value
        .as_f64()
        .map(|v| format!("{:.1}%", v * 100.0))
        .unwrap_or_else(|| "—".into())
}

fn candidate_table(candidates: &[Value], answer: &Value, matched: Option<&str>) -> String {
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header([
            "#",
            "Merchant",
            "Search similarity",
            "Jev probability",
            "Result",
            "Merchant ID",
        ]);
    if !io::stdout().is_terminal() {
        table.set_width(140);
    }
    for (index, candidate) in candidates.iter().enumerate() {
        let key = format!("candidate_{index}");
        table.add_row([
            (index + 1).to_string(),
            crate::output::text(candidate["merchant"]["name"].as_str().unwrap_or("—")),
            format!("{:.3}", candidate["score"].as_f64().unwrap_or(0.0)),
            percent(&answer["probabilities"][&key]),
            if matched == candidate["merchant"]["id"].as_str() {
                "MATCHED".into()
            } else if answer["choice"].as_str() == Some(key.as_str()) {
                "Jev choice".into()
            } else {
                String::new()
            },
            crate::output::text(candidate["merchant"]["id"].as_str().unwrap_or("—")),
        ]);
    }
    table.to_string()
}

fn jev_summary(answer: &Value, candidates: &[Value], threshold: &Value) -> String {
    let Some(choice) = answer["choice"].as_str() else {
        return "Jev: no answer returned".into();
    };
    let selected = choice
        .strip_prefix("candidate_")
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|n| candidates.get(n))
        .map(|c| crate::output::text(c["merchant"]["name"].as_str().unwrap_or("—")))
        .unwrap_or_else(|| crate::output::text(choice));
    let reason = if choice == "none" {
        " · Jev selected no merchant"
    } else if answer["probabilities"][choice]
        .as_f64()
        .zip(threshold.as_f64())
        .is_some_and(|(v, t)| v < t)
    {
        " · below match threshold"
    } else {
        ""
    };
    format!(
        "Jev choice: {selected} · probability {} · confidence (derived diagnostic) {} · probability threshold {}{reason}",
        percent(&answer["probabilities"][choice]),
        percent(&answer["confidence"]),
        percent(threshold)
    )
}

fn render_case(index: usize, result: &eval::CaseResult, mode: Mode) -> Result<String> {
    let mut rendered = format!(
        "\n{}. {}\nDescription: {}\n",
        index + 1,
        crate::output::text(&result.id),
        crate::output::text(&result.description)
    );
    // Omit empty fields; retain meaningful input context without dumping nulls.
    let request = serde_json::to_value(&result.request)?;
    let context: serde_json::Map<_, _> = request
        .as_object()
        .unwrap()
        .iter()
        .filter(|(key, value)| {
            key.as_str() != "description"
                && !value.is_null()
                && !value.as_object().is_some_and(|o| o.is_empty())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !context.is_empty() {
        rendered.push_str(&format!("Context: {}\n", serde_json::to_string(&context)?));
    }
    let trace = result.enrichment.as_ref().unwrap_or(&Value::Null);
    let discovery = trace["method"] == "discovery";
    let catalog: Vec<Value> = result.candidates.iter().map(|c| json!(c)).collect();
    let catalog_answer = if discovery {
        &trace["catalog_provider_answer"]
    } else {
        &trace["provider_answer"]
    };
    if result.candidates.is_empty() {
        rendered.push_str("Catalog candidates: none\n");
    } else {
        rendered.push_str(&candidate_table(
            &catalog,
            catalog_answer,
            result.predicted_id.as_deref(),
        ));
        rendered.push('\n');
    }
    if mode == Mode::Search {
        rendered.push_str("Jev: not evaluated (search mode)\n");
    } else if discovery {
        rendered.push_str(&format!(
            "Catalog {}\n",
            jev_summary(catalog_answer, &catalog, &trace["threshold"])
        ));
        let candidates = trace["candidates"]
            .as_array()
            .context("missing discovery candidates")?;
        rendered.push_str("Discovery candidates:\n");
        rendered.push_str(&candidate_table(
            candidates,
            &trace["provider_answer"],
            result.predicted_id.as_deref(),
        ));
        rendered.push_str(&format!(
            "\n{}\n",
            jev_summary(&trace["provider_answer"], candidates, &trace["threshold"])
        ));
    } else if !trace["provider_answer"].is_null() {
        rendered.push_str(&format!(
            "{}\n",
            jev_summary(&trace["provider_answer"], &catalog, &trace["threshold"])
        ));
    } else {
        let reason = match trace["method"].as_str() {
            Some("exact") => "not called (trusted exact match)",
            Some("verified_descriptor") => "not called (verified descriptor)",
            Some("no_candidates") => "not called (no candidates)",
            _ => "no answer returned",
        };
        rendered.push_str(&format!("Jev: {reason}\n"));
    }
    let outcome = if let Some(error) = &result.error {
        format!("Error: {}", crate::output::text(error))
    } else if let Some(id) = &result.predicted_id {
        let name = result
            .candidates
            .iter()
            .find(|c| &c.merchant.id == id)
            .map(|c| c.merchant.name.as_str())
            .or_else(|| {
                trace["candidates"]
                    .as_array()
                    .and_then(|cs| cs.iter().find(|c| c["merchant"]["id"] == *id))
                    .and_then(|c| c["merchant"]["name"].as_str())
            });
        format!(
            "Matched: {} [{}]",
            crate::output::text(name.unwrap_or("merchant")),
            crate::output::text(id)
        )
    } else if mode == Mode::Search {
        "Match: not evaluated (search mode)".into()
    } else {
        "Unresolved".into()
    };
    rendered.push_str(&format!("Outcome: {outcome}"));
    Ok(rendered)
}

#[cfg(test)]
#[path = "../tests/unit/batch/detail_tests.rs"]
mod detail_tests;

fn print_purchase_filter(report: &eval::Report) {
    if let Some(filter) = &report.purchase_filter {
        eprintln!(
            "Purchase-only metrics: {} of {} inputs; {} excluded (including {} errors). Excluded evidence is retained in the report.",
            report.metrics.cases,
            filter.total_cases,
            filter.excluded_results.len(),
            filter.excluded_errors
        );
    }
}
