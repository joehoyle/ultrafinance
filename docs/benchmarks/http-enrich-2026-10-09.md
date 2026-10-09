# HTTP enrichment benchmark — 2026-10-09

Full `POST /v1/enrich` throughput plateaued at approximately **27 successful
requests/second** for `{"description":"SQ* JULIUS BROMONT"}` against the local
catalog. Every request followed the provider path through Jev and local audit
persistence. No application optimizations were made for this measurement.

| Concurrency | Requests | Successful req/s | p50 latency | p95 latency | HTTP errors |
| --- | --- | --- | --- | --- | --- |
| 1 | 16 | 3.83 | 260 ms | 298 ms | 0 |
| 4 | 64 | 13.31 | 285 ms | 396 ms | 0 |
| 8 | 64 | 23.46 | 315 ms | 488 ms | 0 |
| 16 | 64 | 24.47 | 566 ms | 875 ms | 0 |
| 32 | 128 | 25.90 | 1,189 ms | 1,415 ms | 0 |
| 32 | 512 | 26.92 | 1,160 ms | 1,338 ms | 1 |
| 64 | 256 | 27.09 | 2,320 ms | 2,468 ms | 0 |

The longer concurrency-32 run lasted 18.98 seconds; concurrency 64 lasted 9.45
seconds. Concurrency 8 gave most of the measured throughput with substantially
lower latency. Increasing concurrency beyond 32 added latency with little gain.
These short measurements do not establish long-term capacity or error rates.

The single HTTP 502 came from a Jev transport failure: the connection closed
before the message completed. Audit verification across the preflight and seven
measured runs found 1,105 provider-method records: 1,104 sent provider requests
and one attempted request. All 1,104 HTTP 200 responses returned `unresolved`.
This measures completed fuzzy evaluations, not successful merchant matches.

Environment: Apple M1 Max, 10 CPU cores, 64 GB RAM; optimized release build of
the current dirty workspace at base revision `839b527d571a`; local PostgreSQL
catalog containing 1,709,845 merchants; Jev model `jev-latest`. API, load
generator, and PostgreSQL were on the same machine. One preflight request took
538 ms, including server lazy initialization; the database was already running.

The load generator uses persistent HTTP/1.1 connections and closed-loop load.
It measures individual request latency, including connection setup when needed,
and divides successful completions by total wall time including final drain.
Repeated descriptors warm retrieval caches. There are no retries in the load
generator. Local HTTP measurements exclude Lambda, CloudFront, and deployed
database/network conditions; they do not predict throughput for a varied
descriptor mix or other provider models.

The PostgreSQL store executes jobs through one worker and one client, and callers
block while waiting for jobs. This is a plausible source of the plateau, but
these measurements do not isolate database time from provider time. Profiling
those stages is the next step before choosing an optimization.

Reproduction instructions are in
[the benchmark README](../../crates/ultrafinance-core/benches/README.md#full-http-throughput).
[Raw measurements](http-enrich-2026-10-09.json) preserve all runs and timings.
