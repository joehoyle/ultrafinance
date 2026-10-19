# Lunch Money eval timing — 2026-10-19

The full Jev-backed eval completed in **100.20 seconds**, compared with
approximately **198.17 seconds** inferred from the historical report: 1.98× as
fast, or 49.4% less elapsed time.

| Measurement | Historical before | Current after |
| --- | ---: | ---: |
| Full eval, 197 cases | ~3m 18s inferred | 1m 40s measured |
| Same 159 purchases: median batch-inclusive latency | 59.75s | 28.27s |
| Same 159 purchases: p95 batch-inclusive latency | 101.03s | 48.51s |
| Same 159 purchases: merchant resolved | 88 | 90 |
| Errors, all cases | 0 | 0 |

Both use the original 197 holdout inputs, `jev-latest`, a 0.75 threshold and
100-item batches. The new run used the optimized release binary, the local
PostgreSQL catalog and evaluation mode with learned-match persistence disabled.
Compilation is excluded; the measured current wall time includes process startup
and report writing. All cases are unlabeled: resolved counts are not accuracy.

The historical report retains 159 purchases plus 38 excluded results. Its actual
wall time was not recorded. Rejoining those results to the original holdout order
gives maximum case timers of 91.35s and 106.82s in the two batches. Their sum
approximates the evaluation loop because each timer starts before serial candidate
retrieval and stops after the whole batch completes. This excludes historical
startup/serialization and is explicitly an inference, not a measured wall time.
The individual enrich latency figures likewise include waiting for other items in
the batch; they are not single-request service latency.

This is a historical comparison, not an isolated code A/B test. Catalog contents
and provider behavior may have changed. A separate local-only search run preceded
the full run and could warm PostgreSQL buffers; each run had a fresh process and
fresh process-local candidate caches.

The local-only retrieval eval took **114.72s** for 197 cases (median 517.37ms,
p95 1,172.54ms), with no errors and no provider calls. Its timing is separate from
the full run and must not be added to it. It confirms that the earlier 6.55ms warm
recurring-descriptor microbenchmark does not describe a varied real-input eval.

Aggregate measurements are in
[lunchmoney-latency-comparison-2026-10-19.json](lunchmoney-latency-comparison-2026-10-19.json).
Private per-case reports remain in the ignored `evals/reports/` directory;
transaction inputs and provider credentials are not included in these aggregate
benchmark artifacts.
