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
    pub database: PathBuf,
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
    let store = MerchantStore::configured(&options.database, options.database_url.as_deref())?;
    let fingerprint = store.fingerprint()?;
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
            Ok(report) => {
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
    if store.fingerprint()? != fingerprint {
        bail!("database changed during batch evaluation; reports cannot be compared as one run");
    }
    let metrics = if results.is_empty() {
        None
    } else {
        Some(eval::summarize(&results, options.mode))
    };
    if let Some(metrics) = &metrics {
        table.add_row(row("Total", metrics));
    }
    if let Some(profile) = ultrafinance_core::store::search_profile() {
        eprintln!(
            "Search profile: {}",
            serde_json::to_string_pretty(&profile)?
        );
    }
    let summary = directory.join("summary.json");
    fs::write(
        &summary,
        serde_json::to_vec_pretty(
            &json!({"version":1,"mode":options.mode,"database_fingerprint":fingerprint,"limit_per_suite":options.limit,"metrics":metrics,"suites":summaries,"failures":failures}),
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
