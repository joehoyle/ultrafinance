# Adaptive fuzzy retrieval and processor-first eval — 2026-10-19

| 197-case Lunch Money eval | Before | After | Less elapsed time |
| --- | ---: | ---: | ---: |
| Local retrieval, no provider | 114.72s | 68.77s | 40.1% |
| Full enrichment including Jev | 100.20s | 77.70s | 22.5% |

The local-only run returned identical complete candidate records, order and scores
for all 197 cases. The full run also supplied identical candidate records to
enrichment in all 197 cases. Both full runs sent 27 recognition requests and 20
identity requests. The final run had zero errors. Jev changed three predictions;
resolved cases changed from 93 to 94. These inputs are unlabeled, so this is not
an accuracy comparison.

## Final implementation

Fuzzy vocabulary lookup first uses the existing GIN index at similarity 0.5.
When that yields four words, they are also the original top four at 0.3. Words
with fewer results are searched again at the original 0.3 threshold. Both passes
retain the exact-word exclusion and descending similarity / alphabetical tie
ordering. The transaction threshold resets to 0.3 afterwards. Results enter the
cache only once the lookup succeeds; misses are cached as before.

Of 426 distinct words extracted from the Lunch Money inputs, 389 completed at
0.5 and 37 required the original search. Every completed first-pass result matched
the legacy top four. Regression cases exercise cutoff ties, accented words,
exact-word exclusion, misses, cached reads and restoring the threshold.

The eval now resolves processors before its one candidate lookup and records the
pool and search trace actually used. It retains the historical pool policy:
unchanged descriptions use the eval's mixed pool, while processor-stripped ones
use the primary-role pool. A regression removes broad-search storage and proves
that a wrapped description succeeds using its cleaned, reviewed mapping. An
unnecessary lookup of the original descriptor would fail that test. None of these
197 historical inputs triggers a documented format, so the duplicate-removal gain
is covered by that separate regression rather than this timing suite.

## Experiments excluded from the final change

A GiST KNN index over the approximately 13-million-word vocabulary took 3,775ms
for the public example/cafe query. It visited 183,032 shared buffers and regressed
on this catalog. The experimental index and schema-26 migration were removed;
local schema remains 25. The final implementation requires no new index or schema
upgrade. The warmed GIN plans measured 73ms at 0.5 versus 262ms at 0.3 for the
same public two-word query. Those individual plans are illustrative, not a
per-request latency guarantee.

An intermediate eval revision changed unchanged-input retrieval from the
historical mixed pool to separate merchant and financial-institution pools. It
took 166.63s. That extra matching-policy change was reverted before the final
run; the final candidate-equivalence checks above confirm the restored policy.

[Adaptive GIN plans](enrich-adaptive-gin-query-plans-2026-10-19.txt) and
[discarded GiST plan](enrich-nearest-query-plan-2026-10-19.txt) retain the public
query evidence. [Aggregate measurements](enrich-adaptive-retrieval-2026-10-19.json)
contain no transaction descriptions or provider credentials. Private per-case
reports remain in ignored `evals/reports/` files.

## Validation and measurement limits

The complete core suite passed 276 tests with five ignored, using four test
workers. Seven eval regressions and workspace Clippy passed after the final eval
pool-policy correction. An initial parallel full run hit the existing dedupe
mock-server socket timeout; the bounded-concurrency complete rerun passed.

Both full timings are measured wall time, excluding compilation and including
process startup and report writing. Runs use fresh optimized release processes,
the same 197 inputs, local PostgreSQL, `jev-latest`, threshold 0.75 and batches of
100, with learned-match persistence disabled. The local-only measurement preceded
the full run in both comparisons. No deliberate tests or other benchmark loads
ran concurrently with final timings. Database buffers were warmed, especially by
query comparisons; these are observed runs rather than a controlled cold-database
experiment. Provider latency can vary between runs.

Per-case enrich timers include waiting for the rest of the batch and should not
be compared as individual request service times. Local retrieval still has a
315ms median and 654ms p95 on this varied-input suite. The universal 10ms target
is not met; the earlier 6.55ms result applies to repeated warmed descriptors.
