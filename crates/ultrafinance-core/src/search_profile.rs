//! Opt-in aggregate profiling. Descriptors and merchant data are never logged.
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    ExactSql,
    TokenSql,
    TrigramSql,
    Decode,
    Score,
    Evidence,
}
const NAMES: [&str; 6] = [
    "exact_sql",
    "token_sql",
    "trigram_sql",
    "decode",
    "score",
    "evidence",
];
static ENABLED: OnceLock<bool> = OnceLock::new();
static NANOS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static CALLS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var("ULTRAFINANCE_PROFILE_SEARCH").as_deref() == Ok("1"))
}
pub(crate) fn timed<T>(stage: Stage, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let started = Instant::now();
    let result = f();
    NANOS[stage as usize].fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    CALLS[stage as usize].fetch_add(1, Ordering::Relaxed);
    result
}
pub(crate) fn report() -> Option<serde_json::Value> {
    if !enabled() {
        return None;
    }
    Some(
        NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                (
                    name.to_string(),
                    serde_json::json!({
                        "milliseconds": NANOS[i].load(Ordering::Relaxed) as f64 / 1_000_000.0,
                        "calls": CALLS[i].load(Ordering::Relaxed)
                    }),
                )
            })
            .collect::<serde_json::Map<String, serde_json::Value>>()
            .into(),
    )
}
