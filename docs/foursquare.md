# Foursquare merchant imports

Import a filtered Foursquare OS Places CSV export as merchant and outlet knowledge.
Eligible places with a usable street address and country also create linked
location records. No merchant descriptor aliases, matching regexes or logos are generated.
Foursquare fills business coverage gaps; it supplies no transaction ground truth.

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
# Or download and prepare with reviewed grouping in one command:
cargo run -- sources import foursquare --region ca \
  --examples brands.json --dry-run
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

Downloads retain DuckDB's CSV on disk. Default preparation without reviewed
brand mappings reads CSV incrementally and sorts 5,000-row runs on disk, merging
at most 64 runs at once. Applying knowledge streams chunks of 5,000 source
records by default. `--chunk-size 10000` changes the import bound (positive values
only); larger chunks use more memory. For example:

```sh
cargo run -- sources import foursquare --region ca --offline --chunk-size 10000
```

Each chunk is reconciled against the database identity index; it never loads the merchant catalog.
Disk space is still required for downloads, sorting and prepared bundles.
Reviewed brand grouping currently retains its original in-memory preparation.
Use `--foursquare-limit` for smaller trials. Limited exports can have incomplete
brand membership, so heed the refresh limitations below. Manually exported CSVs
remain supported through `--input`. Cached downloads work without a token or
DuckDB using `--offline`.
The separate `--limit` flag caps records reconciled into the database,
not places downloaded.

Required columns are `fsq_place_id`, `name`, `country`, `fsq_category_ids`, and
`date_closed`. Array columns must contain JSON arrays of strings (configure the
export with `to_json(fsq_category_ids)` rather than an engine's list formatting).
Optional fields include `website`, `unresolved_flags`, `fsq_category_labels`,
address/geography and source refresh dates. Only absolute HTTP(S) URLs without
credentials are used as merchant websites. Invalid upstream websites are omitted
from merchant fields, with the original value and validation marker retained in
the prepared bundle; they do not reject the merchant or abort the import. Explicit
reviewed brand websites remain strictly validated.
See the [source schema](https://docs.foursquare.com/data-products/docs/places-os-data-schema).

Rows marked closed, rows with known disqualifying quality flags, uncategorized
places and Foursquare's published non-commercial category exclusions are skipped.
Names that normalize to empty (for example, punctuation or symbols only) are also
skipped unless a reviewed brand supplies a valid name. These rows contribute to
the skipped count. Foursquare adapter version 2 rebuilds prepared bundles with
this validation while retaining older bundles intact.
Country filtering is optional. Missing category data is not treated as commercial.
Duplicate identical place rows are skipped; conflicting rows with one ID fail.
Historical transactions at closed businesses need a separately designed policy.

```sh
cargo run -- sources import foursquare --input places-ca.csv --region ca \
  --examples brands.json --dry-run
```

The report contains retained place counts, skipped place counts, merchant record
counts and a bundle path. Inspect `knowledge.json` before applying it. Its empty
transaction sample files do not constitute an accuracy benchmark.

## Reviewed brand grouping

`--examples brands.json` is optional and contains reviewed brand membership:

```json
{
  "brands": [
    {
      "id": "starbucks",
      "name": "Starbucks",
      "website": "https://www.starbucks.com/",
      "evidence": "Place identities checked against the official outlet directory",
      "place_ids": ["ACTUAL_FSQ_ID_FOR_BROMONT", "ACTUAL_FSQ_ID_FOR_TORONTO"]
    }
  ]
}
```

Replace the example IDs and evidence with reviewed facts. This creates one source
record `foursquare/brand:starbucks`; contributing place IDs and selected source
fields remain in the prepared bundle. The brand name is the supplied reviewed
name, not a name guessed by stripping a locality. Brand IDs must remain stable.
One place cannot belong to multiple brands. Membership listed outside the current
export is allowed, so the same review file can serve multiple country exports.

Unmapped places retain distinct source identities `foursquare/place:FSQ_ID`.
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
# Reconcile at most 1,000 prepared merchant source records.
cargo run -- sources import foursquare --region ca --offline --limit 1000
# Inspect retained merchant inputs and source metadata.
cargo run -- sources records foursquare --external-id brand:starbucks
```

Existing source keys preserve local merchant IDs and manual corrections. Apply
compares incoming records with the catalog and each other, including across
sources. Merges combine aliases, markets and metadata, retain source links and matching inputs,
and redirect retired IDs and location/resolution references. Existing IDs take
precedence over new IDs; conflicting manual identities remain separate.

Import requires no provider credentials. `--dedupe-dry-run` previews deterministic
decisions against the database without applying records or merges. Plain
`--dry-run` still only prepares the bundle and needs neither a database nor Jev.
Competing writers are serialized for the import transaction. Each chunk checks
existing indexed merchant identities and groups its own incoming duplicates.
Later chunks reuse earlier matches. A preview runs this same pipeline and rolls
back. Duplicate source keys or malformed JSON discovered late also roll back
every earlier chunk. Reconciliation details returned in JSON are capped at
5,000 records/decisions with `details_truncated`; total counts remain complete,
and database audit records retain the full bounded merge decisions.
Import progress on stderr starts with an exact count of the prepared bundle
and the selected total after `--limit`. Counting uses a bounded streaming pass
before opening the database write transaction, so edited bundles are counted
correctly. During application, one terminal line tracks progress across all
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
in batches of 5,000, within one atomic transaction. Merchants store their operating
countries directly in `markets`; imports do not write or reconcile separate
country-evidence records. Search rows and identity keys update only when their
indexed values change. New merchants skip deletion of old alias rows.

Evidence removal changes the fresh schema only; no upgrade migration is provided.
Recreate an existing database to use this layout. Batches do not commit partial
imports, and a failure rolls back the entire import.

## Attribution

Prepared bundles include `LICENSE.txt` and `NOTICE.txt`, preserving Foursquare's
notice and describing our transformations. When serving Foursquare-derived data,
include the notice prominently in developer documentation as described by
[Foursquare](https://opensource.foursquare.com/places-notice-txt/). The bundled
notice is also available at [../data/foursquare/NOTICE.txt](../data/foursquare/NOTICE.txt).
No production deployment or production import is performed by preparation.
