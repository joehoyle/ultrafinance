# Local enrichment latency improvements — 2026-10-19

The warm recurring-descriptor processor-plus-local path reached **6.55 ms mean**
and **7.15 ms p95 of sampled batch averages**, within the 10 ms warm target. The
previous cleaned local path alone averaged 485 ms. The first lookup of a new
descriptor remains far above 10 ms: this change does not meet a universal 10 ms
latency target.

| Warm stage | Mean | p95 batch average |
| --- | --- | --- |
| `processor_formats` | 1.67 ms | 1.98 ms |
| `retrieve` | 5.97 ms | 7.02 ms |
| `enrich_locally` | 5.95 ms | 7.10 ms |
| `enrich_locally_cleaned` | 5.03 ms | 6.19 ms |
| `processor_plus_local` | 6.55 ms | 7.15 ms |

`processor_plus_local` includes DB-backed processor matching followed by local
enrichment of the extracted merchant descriptor. It excludes providers,
persistence/audit writes, HTTP and production orchestration. Other rows overlap
and must not be summed. The benchmark baseline is `SQ* JULIUS BROMONT`, with no
country or other context, on the same large local catalog as the earlier report.

## Changes

- Disable JIT within retrieval transactions, retaining the existing index-use
  policy. Database-wide JIT configuration is unchanged.
- Cache compiled processor regexes and their linked identities against catalog
  revision. Schema 25 adds a statement trigger so direct SQL and other writers
  invalidate those caches too. Published logo fallbacks are still read freshly.
- Retain at most 64 query pools / 8,192 candidates, keyed by the original search
  text, country, role and pool limit. Pool data includes provenance and negative
  alias effects. Every cache read checks revision in a database snapshot; there
  is no whole-request decision cache or change to matching thresholds.
- Fetch warm pools for all hypotheses and roles in one job and transaction.
  Apply logo fallbacks once to the combined candidates.
- Batch outlet reads, redirect resolution and revision checking. Retain up to
  512 outlet identities instead of clearing the cache when a request crosses
  64; the existing 50,000-record memory budget remains.
- Return reviewed, context-scoped descriptor mappings before broad retrieval.
  Unverified mappings and changed contexts still use normal candidate search.
- Reuse the HTTP client in the read-only local helper.

## First lookups are still expensive

Each entry below is one first lookup of a new descriptor in the warmed process,
including processor matching and local enrichment. These are synthetic/public
benchmark inputs, not private transactions. Some vocabulary and query pools can
be shared with earlier entries. This small sample is not a tail-latency estimate.

| Descriptor | First lookup |
| --- | --- |
| `SUMUP*Example Cafe` | 2571.14 ms |
| `PAYPAL *Northwind Books` | 2859.98 ms |
| `AWX*Example Studio` | 601.90 ms |
| `Xsolla_Example Game` | 2233.45 ms |
| `WPY*Example Shop` | 660.58 ms |
| `PayPal Point of Sale *Example Bakery` | 285.29 ms |
| `SQ *JULIUS BROMONT #0042` | 131.54 ms |
| `Example Test Merchant` | 2061.19 ms |

A representative read-only plan for new `example` / `cafe` vocabulary lookups
spent **2,781 ms** in fuzzy-word expansion. The trigram GIN path read **38,846
buffers**, visited **40,129 heap blocks**, rechecked about **64,000 candidate
words per token**, and sorted similarity scores to retain only four words. A
representative `cafe` merchant full-text pool took 21 ms executing and 15 ms
planning. [Query plans](enrich-cold-query-plans-2026-10-19.txt) retain the evidence.

The next cold-path work is an indexed nearest-neighbor vocabulary lookup that
can retrieve the best few similar words without the large bitmap/heap scan,
followed by reducing repeated planning and round trips for uncached pools.
That index/query change is not included here; fuzzy-match candidate semantics
and retrieval coverage remain unchanged.

## Validation and reproduction

The full core suite passed 274 tests with five ignored. Focused tests also check
cross-connection identity/type edits, direct SQL processor-rule updates, preview
rollback, and verified-context bypass with broad-search storage unavailable.
The format CLI integration test and workspace Clippy passed. An earlier stressed
parallel run hit a mock-server socket timeout; the subsequent full run passed.

```sh
ULTRAFINANCE_DATABASE_URL='postgresql://ultrafinance@127.0.0.1:55432/ultrafinance?sslmode=disable' \
  cargo bench --locked -p ultrafinance-core --bench enrich
```

Optimized build, dirty workspace at base `e0159eb`, schema 25, seven enabled
processor formats, local PostgreSQL 17.11 on ARM64, approximately 66.4 million
merchant rows by PostgreSQL's statistics estimate. No deliberate tests or
benchmark runs competed during the final measurement; other application
activity was present on the shared local database. The initial process-local
request is recorded separately and is not a cold-database measurement.

Thirty warm samples per stage use calibration and batch averages. The p95 shown
here is not individual request tail latency or an API SLA. Raw values and first
lookups are in [the measurements JSON](enrich-latency-optimization-2026-10-19.json).
