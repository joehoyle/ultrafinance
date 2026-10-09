# Foursquare merchant imports

Import a filtered Foursquare OS Places CSV export as merchant knowledge. No
location records, descriptor aliases, matching regexes or logos are generated.
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

The current pipeline loads the CSV and prepared records into memory; large
regional/global imports require substantial memory and disk space. This removes
the pilot caps but does not make the pipeline streaming. Use an explicit
`--foursquare-limit` for smaller trials. Limited exports can have incomplete brand
membership, so heed the refresh limitations below. Manually exported CSVs remain
supported through `--input`. Cached downloads work without a token or DuckDB using
`--offline`. The separate `--limit` flag caps records reconciled into the database,
not places downloaded.

Required columns are `fsq_place_id`, `name`, `country`, `fsq_category_ids`, and
`date_closed`. Array columns must contain JSON arrays of strings (configure the
export with `to_json(fsq_category_ids)` rather than an engine's list formatting).
Optional fields include `website`, `unresolved_flags`, `fsq_category_labels`,
address/geography and source refresh dates. Only absolute HTTP(S) URLs without
credentials are used as merchant websites. Invalid upstream websites are omitted
from merchant fields, with the original value and validation marker retained in
source evidence; they do not reject the merchant or abort the import. Explicit
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
fields remain in its raw provenance. The brand name is the supplied reviewed
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
A shared name or website alone does not trigger an automatic merge.

## Apply and refresh

```sh
# Applies merchant knowledge to the configured database.
cargo run -- sources import foursquare --region ca --offline
# Reconcile at most 1,000 prepared merchant source records.
cargo run -- sources import foursquare --region ca --offline --limit 1000
# Inspect retained source evidence.
cargo run -- sources records foursquare --external-id brand:starbucks
```

Existing source keys preserve local merchant IDs and manual corrections. Apply
compares incoming records with the catalog and each other, including across
sources. Merges combine aliases, markets and metadata, preserve raw place evidence,
and redirect retired IDs and location/resolution references. Existing IDs take
precedence over new IDs; conflicting manual identities remain separate.

Import requires no provider credentials. `--dedupe-dry-run` previews deterministic
decisions against the database without applying records or merges. Plain
`--dry-run` still only prepares the bundle and needs neither a database nor Jev.
Concurrent catalog changes prevent the whole import from committing.
`merchants dedupe` is a separate operation for provider-assisted fuzzy review;
`merchants link` remains available for reviewed links.

The shared `--limit X` flag selects the first X prepared merchant source records
before reconciliation, including existing matches and updates. It works with
cached or file input and leaves the full snapshot and bundle intact. Brand
grouping happens before selection, so each selected brand retains all its
prepared place evidence. Repeating the same limit selects the same prefix.
`--foursquare-limit` separately caps downloaded places. `--limit` does not reduce
download or first-time preparation work. Repeat imports reuse an existing bundle
when the snapshot, reviewed mappings, region and adapter version match, retaining
edits to saved knowledge. Limited imports scan that bundle but allocate only the
selected source records. For a 10,000-record trial using an existing download:

```sh
cargo run -- sources import foursquare --region ca --offline --limit 10000
```

Each input must contain complete membership for every brand included in that
import. A later subset for the same brand replaces its source evidence rather
than appending to it; combine country subsets before applying a global brand.
Imports are additive/updating: omitted and newly closed places do not delete
previous records. Automatic delta removals and global ingestion are
future work. Geography is retained as evidence but creates no location result.

Compare retrieval and enrichment on an independently labeled transaction suite
before and after import. Same-name places can compete for shortlist space until
brand grouping and geography-aware retrieval are expanded.

## Attribution

Prepared bundles include `LICENSE.txt` and `NOTICE.txt`, preserving Foursquare's
notice and describing our transformations. When serving Foursquare-derived data,
include the notice prominently in developer documentation as described by
[Foursquare](https://opensource.foursquare.com/places-notice-txt/). The bundled
notice is also available at [../data/foursquare/NOTICE.txt](../data/foursquare/NOTICE.txt).
No production deployment or production import is performed by preparation.
