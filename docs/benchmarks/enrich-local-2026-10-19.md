# Local enrichment benchmark — 2026-10-19

Local retrieval dominates the benchmark. PostgreSQL JIT adds substantial avoidable
latency: turning it off **only for the benchmark connection** reduced warm local
enrichment by about 37% and the cleaned-descriptor local path by about 25%.
No application query optimizations or database configuration changes were made.

| Stage | JIT on, mean | JIT off, mean |
| --- | --- | --- |
| `normalize` | 0.000040 ms | 0.000040 ms |
| `interpret` | 0.003500 ms | 0.003530 ms |
| `processor_formats` | 16.663820 ms | 21.142660 ms |
| `retrieve` | 436.281800 ms | 390.223760 ms |
| `enrich_locally` | 335.913730 ms | 212.198240 ms |
| `merchant_pool_cleaned` | 77.662800 ms | 55.018760 ms |
| `bank_pool_cleaned` | 69.314690 ms | 35.618420 ms |
| `enrich_locally_cleaned` | 485.199720 ms | 363.763000 ms |

The original `retrieve` stage calls the general `search_request` path. Production
merchant lookup uses role-scoped retrieval. `enrich_locally` exercises that local
primary path on the original descriptor, without processor resolution. The new
`enrich_locally_cleaned` stage runs it on `JULIUS BROMONT`, as produced by the
unique documented Square format. Processor matching is measured separately.
Neither local path measures the entire production enrichment request.

These rows overlap; do not add them. Each type-specific pool is one search, while
request retrieval also searches distinct interpretation hypotheses. Each primary
search text queries both merchant and financial-institution pools. Removing the
processor changes the interpretation hypotheses, so a cleaned request can do
more retrieval work than the original descriptor.

## What causes the cost

`PostgresStore::search_typed` applies `SET LOCAL enable_seqscan=off`. Read-cache
validation then runs `SELECT revision FROM catalog_revision` within that same
transaction. The forced-scan planner penalty raises its cost above 10 billion,
triggering JIT, including inlining and optimization, for a one-row result.

A read-only `EXPLAIN (ANALYZE, BUFFERS)` showed 41.8 ms of compilation and 51.3 ms
execution for this lookup with JIT; the corresponding JIT-off diagnostic took
0.22 ms. The compilation includes compiler initialization, so that single-query
comparison is **not** a steady-state per-lookup estimate. The full benchmark A/B
above measures the effect on repeated warm operations.

The remaining cost is database retrieval and repeated work across hypotheses
and roles. One representative two-word financial-institution full-text pool
spent 10.7 ms executing and 5.6 ms planning, including two joins against about
10,167 financial-institution keys. This isolated plan is evidence of remaining
work, not a complete SQL-level attribution of the benchmark.

Processor matching takes about 17–21 ms. Its current implementation reloads
rules, compiles every enabled regex, and fetches each associated processor on
every call. Revision-keyed caching is a candidate optimization; this benchmark
does not isolate the contribution of each of those operations. Normalization
and interpretation are negligible by comparison.

First local calls were 724 ms (JIT on) and 527 ms (JIT off), after database/runtime
setup. These are process-local initialization measurements, not cold database
runs. The original benchmark before the added stages also showed about 433 ms
retrieval and 324 ms local enrichment, consistent with the retained baseline.

## Reproduce and limits

```sh
ULTRAFINANCE_DATABASE_URL='postgresql://ultrafinance@127.0.0.1:55432/ultrafinance?sslmode=disable' \
  cargo bench --locked -p ultrafinance-core --bench enrich
ULTRAFINANCE_DATABASE_URL='postgresql://ultrafinance@127.0.0.1:55432/ultrafinance?sslmode=disable&options=-c%20jit%3Doff' \
  cargo bench --locked -p ultrafinance-core --bench enrich
```

Optimized bench profile, dirty workspace at base `e0159eb`, Rust 1.98.0, local
PostgreSQL 17.11 on ARM64, approximately 66.4 million merchants by PostgreSQL's
statistics estimate, and seven enabled processor formats. Both runs used
`SQ* JULIUS BROMONT` with no country or other context, and returned unresolved
merchant results. Runs were sequential, JIT on first, on shared local hardware;
cache state and other machine load can affect the comparison.

Thirty warm samples per stage report batch averages, not individual-request
tail latency. No Jev/discovery calls, audit writes, descriptor learning, HTTP,
Lambda or CloudFront were measured. In particular, these results do not establish
current Jev latency or end-to-end service throughput. The older HTTP benchmark
used a much smaller catalog and is not a direct comparison.

[Raw measurements](enrich-local-2026-10-19.json) retain means and percentiles.
