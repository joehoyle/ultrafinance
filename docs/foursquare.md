# Foursquare merchant imports

Import a filtered Foursquare OS Places CSV export as merchant and outlet knowledge.
Eligible places with a usable street address and country also create linked
location records. No merchant descriptor aliases, matching regexes or logos are generated.
Foursquare fills business coverage gaps; it supplies no transaction ground truth.

After reconciliation links places to a merchant, candidate `provenance` contains
consolidated Foursquare merchant evidence rather than one document per branch.
Distinct attribution URLs and matching rules remain separate. The contribution's
`raw.source_record_count` records how many upstream mappings it represents;
its external ID is a representative existing place source key. Consolidation
does not establish or merge identities.
Per-place keys remain available through source browsing for refreshes, relinking
and audit, while outlet records retain addresses, coordinates and place IDs.
The projection also applies to existing imports and remembered matches without
a data migration or reimport.

## Export and prepare

Current releases require access through the [Places Portal](https://places.foursquare.com/)
or approved [Hugging Face access](https://huggingface.co/datasets/foursquare/fsq-os-places).
The CLI can download directly with a **Places Portal access token**, distinct from
an ordinary Places API key. **The authenticated Foursquare downloader requires
the DuckDB CLI** with Iceberg REST catalog support. On macOS, install it with
[Homebrew](https://formulae.brew.sh/formula/duckdb):

```sh
brew install duckdb
duckdb --version
```

The application Docker image includes a pinned ARM64 DuckDB CLI and the signed
`httpfs` and `iceberg` extensions, so production CLI tasks do not need to install
them through the VPC. Images built before this support need a new build or a
temporary manual installation.

DuckDB must be on your `PATH`, or configured through `--duckdb-cli` or
`ULTRAFINANCE_DUCKDB_CLI`. It is required for `sources download foursquare` and
`sources import foursquare` when fetching fresh data. Imports using `--offline`
or `--input` do not require DuckDB. Other source adapters do not require it.

Supply the token via `ULTRAFINANCE_FOURSQUARE_TOKEN` in your
local environment. Do not paste it into chat or pass it as a command argument.

```sh
# With ULTRAFINANCE_FOURSQUARE_TOKEN set locally:
cargo run -- sources download foursquare --region ca
cargo run -- sources import foursquare --region ca --offline --dry-run
```

The engine connects read-only to Foursquare's authenticated Iceberg catalog,
using temporary vended credentials. Extensions `httpfs` and `iceberg` are installed
on first use. Override the executable with `--duckdb-cli` or
`ULTRAFINANCE_DUCKDB_CLI`. Tokens are piped to an in-memory DuckDB session; they are
not stored in snapshots, SQL files, command arguments or logs. Engine diagnostics
are suppressed because they can contain tokens or storage credentials. Invalid or
expired tokens fail the download without replacing the latest valid snapshot.

By default, downloads include **all open places in the selected region** (or
all countries with `--region global`). `--foursquare-limit X` optionally caps
downloaded places to a positive integer, ordered by place ID. Metadata records
the country and whether a cap was requested. There is no fixed export size or
time limit. The importer further excludes unsuitable rows.

Downloads retain DuckDB's CSV on disk. Foursquare import sorts 5,000-row runs
on disk and merges at most 64 runs at once. The final merge produces ordered
parts of at most 100,000 CSV rows. A streaming pass validates duplicates and
counts eligible places, without writing prepared records. Import then converts
sorted rows into source records in memory and reconciles them directly into
PostgreSQL. **No `knowledge.json` or prepared JSON bundle is created by Foursquare
import, including `--dry-run` and `--dedupe-dry-run`.**

Database imports commit independent chunks of at most 5,000 source records by
default. `--chunk-size 10000` changes this bound; larger chunks use more memory.
Each chunk includes merchant reconciliation, source deltas, outlets and indexes
in one transaction. An error rolls back the current chunk, while earlier chunks
remain committed. The importer avoids counting the whole merchant catalog on
every chunk. New merchants reuse the canonical fields computed from the
incoming records when building aliases, search rows and identity keys, avoiding
a readback of their newly inserted source rows. Established merchants still
rebuild from retained source evidence and outlet countries. Keys rebuilt during
bulk writes are not refreshed again at the end of the same chunk; unchanged
sources still receive the final key refresh so rule changes can take effect.
Alias refreshes retain unchanged names instead of deleting and reinserting them.
When newly added Foursquare sources repeat an established merchant's exact name,
existing website/logo and already-known aliases, provenance and countries, the
importer reuses its canonical fields and alias/search rows instead of rereading
its source/outlet history or refreshing those indexes. Identity rule keys still
refresh. Every incoming record for that merchant must qualify. Source replacements
rebuild from retained inputs; source and outlet records are always written normally.
For newly added Foursquare sources, the importer also checks each established
merchant without a manual override for retained Foursquare evidence that sorts
before this batch, including a website/logo when its canonical fields contain
them. A bounded index probe establishes first-source precedence even when later
IDs already exist elsewhere. Qualifying merchants combine new aliases,
provenance and countries in PostgreSQL source order without reading their full
source/outlet history. Canonical changes stay in the same chunk transaction.
Append-only alias updates compare normalized names, retain existing index rows
and submit only new normalized aliases. Variants that normalize identically
do not rewrite alias or search rows.
Merchants without this proof, manually overridden merchants and source replacements
retain the full rebuild path. Identical-evidence reuse remains available when
the ordering proof fails.
New merchant buckets are matched and grouped before insertion. The final
duplicate-bucket reconciliation checks established merchants, including repaired
keys and legacy duplicates, rather than probing every new identity a second time.
Only targets found in the stored identity index are read back from the merchant
table; new UUIDs do not incur primary-key lookups before insertion.
Import transactions use 64 MiB of PostgreSQL `work_mem` per sort/hash operation
to reduce disk spills; this setting ends with the transaction. Concurrent
operations and parallel query workers can each use this allowance.

Source and merchant lookups use bounded primary-key probes, even before
PostgreSQL has current statistics for a growing catalog. The ordered-evidence
check first selects a merchant's own source rows, then checks source, ordering
and website/logo requirements. The SQL `OFFSET 0` boundaries are intentional:
without them, the planner can repeatedly build a bitmap of the entire earlier
Foursquare source-key range for each merchant in a batch. This preserves the
existing reconciliation rules, indexes and per-chunk transactions; it does not
require an infrastructure change or schema migration.

With `--verbose`, the importer reports the ordered-evidence check separately
and prints each committed chunk's record count, database duration, rate and
cumulative committed count. Chunk duration includes reconciliation and commit,
but excludes CSV decoding/preparation and checkpoint saving.

To compare release binaries against the same initialized local catalog, use
`dev/bench-foursquare.py`. Supply a CSV window containing fresh source IDs to
measure append performance, or previously imported IDs to measure refreshes:

```sh
ULTRAFINANCE_DATABASE_URL='postgresql://ultrafinance@127.0.0.1:55432/ultrafinance_bench?sslmode=disable' \
  python3 dev/bench-foursquare.py \
  --binary /tmp/ultrafinance-before --binary target/release/ultrafinance \
  --input /tmp/places-ca-window.csv --region ca --limit 10000 \
  --repeats 3 --output /tmp/foursquare-comparison
```

This prepares the CSV once, alternates binaries, and runs reconciliation previews
that roll back. Use an isolated catalog: previews acquire writer locks and
generate real database work. The JSON summary includes wall time and phase
timings; phase timings are rounded and nested, so they are not additive.
Run without concurrent builds or imports. Preview throughput excludes commit
costs and is not a full-import forecast. Compare both fresh-statistics and
lagging-statistics cases; automatic analysis after an import stops can conceal
the plan that was slow while the catalog was growing.

Crash-safe resume requires durable PostgreSQL commits. The local Compose database
defaults to disposable development mode (`fsync=off`, `full_page_writes=off`,
`synchronous_commit=off`) for import throughput. Graceful interruption can resume,
but after a host/VM crash the database may be corrupt or behind the on-disk cursor.
Rebuild and re-import from source with a fresh checkpoint in that case; the
remaining merge files cannot recover already-deleted chunks. Use durable database
settings when this recovery trade-off is unacceptable.


A small checkpoint under `--output` (default `data/datasets/foursquare/*.streaming`)
records the database target, input fingerprint, completed chunks, sorted-file byte
offset, and the previous row for duplicate validation. After an error or
interruption, rerun the same command with the same input, region, output directory
and database:

```sh
cargo run -- sources import --offline foursquare
```

If the original CSV/download cache has been deleted after application began,
resume directly from the checkpoint directory (or its `checkpoint.json` file):

```sh
cargo run -- sources import foursquare --region global \
  --resume data/datasets/foursquare/NAME.streaming --chunk-size 100000
```

This mode needs no download, token, DuckDB, or CSV fingerprint pass. It requires
completed sorting and validation plus an application checkpoint matching the
region, adapter version, and database target. All remaining sorted files must
exist. The saved cursor and duplicate boundary are retained, and a completed
checkpoint is a no-op rather than a fresh refresh pass. `--resume` cannot be used
with `--input`, `--offline`, `--examples`, or either dry-run mode; consumed rows
are not available for a fresh preview. Use the normal input-based command for
new snapshots or full refreshes.

The CSV is hashed again, but completed sorting and application are reused. The
cursor advances only after a chunk commits. If the commit succeeds but saving
the cursor fails, the next run replays that chunk through delta reconciliation.
Completed sorted parts are deleted only after the database reports a successful
commit and the saved cursor is flushed to disk.
An unfinished part remains, and resume seeks directly to its saved byte offset.
The original cached CSV is preserved. Once an import completes, a new invocation
starts a fresh delta/refresh pass from the CSV.

`--limit` is the cumulative number of source records selected for that import;
rerunning the same limit adds no further records. Increase or remove the limit
to continue from the saved offset. Reports include `resume.previously_committed`,
`resume.committed_this_run`, and completion status; `delta` covers this invocation.
A checkpoint rejects a different database target. Use a separate `--output`
directory to import the same snapshot into another database.

Plain `--dry-run` validates and counts without connecting to PostgreSQL.
`--dedupe-dry-run` streams reconciliation through one rollback-only transaction
so matches across chunks are previewed accurately. Both keep sorted data in a
separate `*.preview` directory and do not advance an applied import's cursor.
The report's `bundle` is null; `checkpoint` identifies the working directory.
License and notice files remain beside the checkpoint.

Sorting can replay up to 64 unsaved runs (320,000 rows). Checkpoints survive
process exit, but not deletion of their output disk or an ephemeral CLI task's
scratch volume. Imports failed using the older importer cannot recover its
deleted temporary work; their CSV can still be reused with `--offline`.

Disk space is required for the original CSV, sorted runs, and PostgreSQL storage.
Merge replacement files temporarily overlap their inputs. Use
`--output /path/on/larger/disk` for working files on another disk. The large
expanded prepared JSON copy is no longer part of the import's space requirement.

Uncapped downloads stream without a global SQL sort; capped downloads still
order by place ID to select a stable subset. Preparation supplies deterministic
place ordering and duplicate validation in both cases. DuckDB spill files use
the export's temporary directory, which follows `TMPDIR`. Its memory budget
defaults to half the detected Linux container limit, or 4 GiB when no bounded
container limit is detected. Set `ULTRAFINANCE_DUCKDB_MEMORY_LIMIT` to a DuckDB
memory size such as `16GiB` to override the budget.

For example:

```sh
cargo run -- sources import foursquare --region ca --offline --chunk-size 10000
```

Each chunk is reconciled against the database identity index; it never loads the merchant catalog.
Disk space is still required for downloads, sorting and database storage.
Use `--foursquare-limit` for smaller trials. Limited exports have incomplete
place coverage, so heed the refresh limitations below. Manually exported CSVs
remain supported through `--input`. Cached downloads work without a token or
DuckDB using `--offline`.
The separate `--limit` flag caps records reconciled into the database,
not places downloaded.

Downloads first count the open places selected by the region and optional
download cap, then show CSV records written against that total, percentage,
CSV size, and elapsed time on stderr. Counting handles quoted multiline fields
and excludes the CSV header. A startup indicator remains visible while the
catalog and count are loading. Terminal output updates one line; redirected
logs receive summaries every five seconds. A final CSV scan can briefly show
`Checking CSV` after DuckDB finishes. `Export complete` marks the end of the
DuckDB phase; saving, validating and hashing the cached CSV each report their
own byte progress afterwards. `snapshot saved` confirms the download cache
and latest pointer are ready. Engine output remains suppressed to
protect credentials. No percentage is shown until the selected total is known.

Required columns are `fsq_place_id`, `name`, `country`, `fsq_category_ids`, and
`date_closed`. Array columns must contain JSON arrays of strings (configure the
export with `to_json(fsq_category_ids)` rather than an engine's list formatting).
Optional fields include `website`, `unresolved_flags`, `fsq_category_labels`,
address/geography and source refresh dates. Only absolute HTTP(S) URLs without
credentials are used as merchant websites. Invalid upstream websites are omitted
from merchant fields, with the original value and validation marker retained in
the in-memory place evidence; they do not reject the merchant or abort the import.
See the [source schema](https://docs.foursquare.com/data-products/docs/places-os-data-schema).

Rows marked closed, rows with known disqualifying quality flags, uncategorized
places and Foursquare's published non-commercial category exclusions are skipped.
Names that normalize to empty (for example, punctuation or symbols only) are also
skipped and counted. Foursquare adapter version 5 rebuilds prepared bundles
without manual brand mappings while retaining older bundles intact.
Country filtering is optional. Missing category data is not treated as commercial.
Places with missing or malformed country codes are skipped and
included in `skipped_places`; one such place does not abort preparation of the
remaining dataset.
Place names and addresses that cannot form an outlet alias of at least three
normalized characters do not create an outlet. The merchant source record and
original place evidence remain eligible; one weak alias does not abort a chunk.
Duplicate identical place rows are skipped; conflicting rows with one ID fail.
Historical transactions at closed businesses need a separately designed policy.

```sh
cargo run -- sources import foursquare --input places-ca.csv --region ca \
  --dry-run
```

The report contains retained place counts, skipped place counts, merchant record
counts and a checkpoint path. Foursquare supplies no transaction accuracy benchmark.

## Automatic identity reconciliation

Every retained place has a stable source identity `foursquare/place:FSQ_ID`.
There are no manual brand mappings or review files.

Preparation does not consolidate them. When applied, all merchant imports use the
deterministic identity rules shared with `merchants dedupe`: matching canonical
names and business website hosts are accepted by rule. Legal suffixes, case,
accents and punctuation are normalized. Shared platforms and directory domains
are excluded. Import performs no fuzzy pair scan and makes no Jev requests;
uncertain identities remain separate.
Accepted matches share a local merchant ID and retain all their source records.
When both websites are absent or blank, identical complete normalized names
also merge. This rule normalizes case, accents, punctuation and whitespace,
but does not strip legal suffixes, use aliases or infer a chain from a partial
name. Blank websites do not match populated websites, and conflicting manual
identities remain separate. This intentionally groups same-name businesses with
no websites even when their addresses or countries differ; all original source
identities are retained. A shared website alone does not trigger an automatic merge.

## Apply and refresh

```sh
# Applies merchant knowledge to the configured database.
cargo run -- sources import foursquare --region ca --offline
# Commit up to the first 1,000 eligible source records.
cargo run -- sources import foursquare --region ca --offline --limit 1000
# Inspect retained merchant inputs and source metadata.
cargo run -- sources records foursquare --external-id place:FSQ_ID
```

Existing source keys preserve local merchant IDs and manual corrections. Apply
compares incoming records with the catalog and each other, including across
sources. Merges combine aliases, markets and metadata, retain source links and matching inputs,
and redirect retired IDs and location/resolution references. Existing IDs take
precedence over new IDs; conflicting manual identities remain separate.

Import requires no provider credentials. `--dedupe-dry-run` previews deterministic
decisions against the database without applying records or merges. Plain
`--dry-run` validates and counts sorted places and needs neither a database nor Jev.
Competing writers are serialized for the import transaction. Each chunk checks
existing indexed merchant identities and groups its own incoming duplicates.
Later chunks reuse earlier matches. A preview runs this same pipeline and rolls
back. Foursquare validates duplicate CSV identities before applying any database chunks.
A later database failure preserves its already committed chunks. Other source
imports using prepared bundles retain whole-import atomic transactions. Reconciliation details returned in JSON are capped at
5,000 records/decisions with `details_truncated`; total counts remain complete,
and merge decisions are not persisted in the database.
Foursquare progress tracks sorted CSV rows and committed source counts.
Other sources start with an exact count of the prepared bundle
and the selected total after `--limit`. Counting uses a bounded streaming pass
before dispatching database work, so edited bundles are counted correctly and
long reads do not hold an acquired database connection. The database worker
checks its connection after counting and reconnects if PostgreSQL expired it;
database write jobs are not retried inside the worker; Foursquare cursor recovery
explicitly replays unacknowledged chunks through delta reconciliation. During application, one terminal line tracks progress across all
selected source records, elapsed time, average rate, estimated time remaining,
and cumulative locations submitted for upsert. The current stage updates this
same line; chunk percentages and per-chunk summaries are hidden. Redirected
stderr receives periodic overall summaries instead of terminal escape codes.
The rebuild stage now reports source input reads, existing outlet country reads,
canonical identity writes, alias writes, and search writes separately.
Use `sources import ... --verbose` for detailed timings for each phase. A final
commit message confirms persistence; processing completion alone does not mean
the transaction has committed. Preparation uses separate phase progress before
application starts.

Source writes and merchant/search rebuilding use binary COPY and set-based SQL
in batches of 5,000, within each import transaction. Merchants store their operating
countries directly in `markets`; imports do not write or reconcile separate
country-evidence records. Search rows and identity keys update only when their
indexed values change. New merchants skip deletion of old alias rows.

Evidence removal changes the fresh schema only; no upgrade migration is provided.
Recreate an existing database to use this layout. Foursquare chunks commit independently and resume from their saved sorted-file
offset; other source imports remain atomic.

## Production sizing benchmark

On 2026-10-13, the production CLI was sized to 8 ARM64 vCPUs, 32 GiB RAM,
and a fresh encrypted 1 TiB gp3 scratch disk. The shell starts in `/scratch`;
temporary files use `/scratch/tmp`. The disk is deleted when the task stops.
The default 24-hour lifetime is a hard stop, not an estimated import duration.

The read-only Foursquare catalog contained 101,805,864 open places. A global
export capped at 100,000 places downloaded and prepared in 30 seconds, retained
86,893 records, and used about 111 MiB for the cached CSV and prepared bundle.
A production reconciliation preview of 10,000 retained records took 161 seconds
including connection/counting overhead (142 seconds in the application progress
timer). It completed with rollback and no committed catalog changes. A larger
preview was cancelled after this result, also without committing.

Applying the sample retention fraction suggests roughly 88 million records;
linear extrapolation from the preview gives about 15–16 days on the current
database/import pipeline, well beyond the CLI lifetime. This is a single sample,
not a reliable full-run forecast: existing identities, catalog growth, caching,
database I/O, and transaction commit costs can change the rate. The current
Aurora capacity ceiling is 1 ACU. Database work dominated the preview; increasing
CLI cores alone does not resolve it. Rebenchmark after tuning the database and
import pipeline before starting a global production import.

## Local growth benchmark (2026-10-17)

The Canada download contained 2,284,369 open places (548.9 MiB), of which
2,077,583 were eligible. A local PostgreSQL 17 run reproduced a slowdown while
loading the catalog: the ordered-evidence query built a source-key bitmap of
425,666 rows **500 times** for 500 merchant probes. `EXPLAIN (ANALYZE, BUFFERS)`
reported 6,030 ms; restricting each probe to its merchant first took 18 ms.
After automatic analysis caught up, the original query also became fast.

End-to-end reconciliation comparisons used the `a50bd45` release binary and the
bounded-probe fix against the same isolated catalog, alternating three rollback
previews per binary. Each comparison selected 10,000 fresh records from a
50,000-row CSV window shared by both binaries; the window differed between
catalog sizes. Preparation was cached; startup, CSV reads and reconciliation were
included, but commits were not. Results are medians:

| Existing source records | Source statistics | Before | After | Speedup |
| --- | --- | --- | --- | --- |
| 500,000 | Absent | 24.96 s | 3.56 s | 7.0× |
| 1,000,000 | Absent | 26.90 s | 5.25 s | 5.1× |
| 1,000,000 | Fresh | 5.66 s | 5.55 s | About equal |

The absent-statistics comparisons used a restored catalog with source
auto-analysis disabled **only for the benchmark**, to hold the bad-plan
condition observed during the growing import constant. Auto-analysis was then
restored and `VACUUM (ANALYZE)` run for the control. Selected-record and merge
candidate counts agreed across all runs. These measurements do not establish a
consistent 10× whole-import improvement or a ten-million-record completion time.

The fixed importer separately committed another 500,000 records, growing the
catalog from 500,000 to one million, in 264.09 seconds of database work including
commits (about 1,893 records/s; 117 chunks). This excludes CSV preparation and
checkpoint writes. The server retained 128 MiB shared buffers and durable
commits (`fsync`, `full_page_writes`, `synchronous_commit` enabled) on a host with
two vCPUs and 15 GiB RAM. A raw CSV `COPY` control loaded 50,000 rows into an
unindexed temporary table in 0.22 seconds; that is a transport baseline, not a
comparable indexed/reconciled import. The remaining measured work includes
location writes and merchant/search index maintenance.

## Attribution

Prepared bundles include `LICENSE.txt` and `NOTICE.txt`, preserving Foursquare's
notice and describing our transformations. When serving Foursquare-derived data,
include the notice prominently in developer documentation as described by
[Foursquare](https://opensource.foursquare.com/places-notice-txt/). The bundled
notice is also available at [../crates/ultrafinance-core/data/foursquare/NOTICE.txt](../crates/ultrafinance-core/data/foursquare/NOTICE.txt).
No production deployment or production import is performed by preparation.
