# Ultrafinance

A small synchronous merchant enrichment API in Rust. Axum serves HTTP, and the
independent `ultrafinance-core` crate retrieves merchant candidates from PostgreSQL (or local SQLite)
and uses Jev to evaluate fuzzy matches. Unique normalized exact aliases resolve
locally without a provider call.
One request returns a completed result; there are no background jobs.

## Run

```sh
cargo run -p ultrafinance-api
```

The server binds to `127.0.0.1:3000`. Set `ULTRAFINANCE_DATABASE_URL` to use PostgreSQL; without it, local development opens `data/ultrafinance.sqlite`.
An empty database returns `unresolved` without calling any provider.

Open `http://127.0.0.1:3000/` for the project website, copyable HTTP/CLI examples,
an interactive merchant lookup, and a searchable merchant explorer. The form calls `/v1/enrich` on the same
origin. Static hosting needs a same-origin proxy for `/v1/enrich` to support
live lookups.
The standalone page lives in `website/index.html` and can also be hosted by any
static web server, without a frontend build step.

The website's canonical URL is `https://ultrafinance.app/`. Search metadata,
Open Graph and Twitter cards, and JSON-LD are in the initial HTML. A branded
1200×630 share image and favicon files live in `website/assets/`. The API embeds
and serves these assets alongside `/robots.txt` and `/sitemap.xml`; they require
no writable filesystem at runtime. Health and enrichment responses carry
`X-Robots-Tag: noindex`, while the landing page is indexable.

Validate metadata with `python3 website/check_metadata.py`. Regenerate brand
assets with `python3 website/generate_assets.py` (requires Pillow and Arial or
DejaVu fonts). When changing the public domain, update the HTML, robots file,
sitemap, and share image together. After deploying, verify the domain in Google
Search Console and submit `https://ultrafinance.app/sitemap.xml`.

Add verified merchants using the CLI commands below. Set `TYPESAFE_API_KEY` in
your environment to evaluate fuzzy candidates. The API sees database updates on
subsequent requests. For an isolated JSON catalog, set `ULTRAFINANCE_MERCHANTS`
to its path; this loads the catalog into memory at startup instead of SQLite.
Transaction fields including `extra` and candidate records are sent to TypeSafe
when evaluation runs. Keep unrelated personal information out of requests.

```sh
curl -s http://127.0.0.1:3000/v1/enrich \
  -H 'Content-Type: application/json' \
  -d '{"description":"LS","amount":"142.97","currency":"CAD","country":"CA","extra":{"bank_category":["Food and Drink","Restaurants"]}}'
```

```json
{"merchant":{"status":"unresolved","data":null},"location":{"status":"unresolved","data":null}}
```

A supported match returns `merchant.status = "matched"` with the catalog merchant
in `merchant.data`. Merchant IDs belong to this service's catalog and must remain
stable when names or aliases change. The `country` field narrows the candidate
pool, not the location of a specific outlet.

Only `description` is required. Optional structured fields are `amount` (a decimal
string), `currency`, `date` (ISO date string), and `country`. `extra` is a JSON
object accepting nested evidence and free text. Unknown top-level fields are
rejected so spelling errors cannot silently disappear. Input is limited to 64 KiB.
Service or provider failures return HTTP errors, rather than `unresolved`.

## API documentation

Open `/docs` for the interactive Scalar reference, including request examples and
an API client. `/openapi.json` serves the generated OpenAPI document. Both are
served by the API on the same origin; static website hosting also needs to proxy
these paths. Scalar's browser JavaScript loads from jsDelivr.

Schemas come from the core request and response types using the optional `openapi`
feature. Register HTTP API handlers with `utoipa_axum::routes!` in `api_router()`
so serving the handler also includes it in the document. Add descriptions,
examples and error responses alongside the handler and field definitions.

The committed `crates/ultrafinance-api/openapi.json` is a generated review snapshot.
The API tests compare it with the served document, and CI runs those checks for
pull requests. Contract changes require an explicit snapshot update, so reviewers
can inspect changes to endpoints and schemas. Export it without opening a database
or calling a provider:

```sh
cargo run -p ultrafinance-api -- --print-openapi > crates/ultrafinance-api/openapi.json
cargo test -p ultrafinance-api
```

## Local CLI

The CLI is the default workspace executable. `cargo run` displays its help.
It uses Clap for commands, argument validation, and generated help.
Use the same enrichment core directly, without starting the server:

```sh
cargo run -- enrich 'LS' --country CA --amount 142.97 \
  --currency CAD --extra '{"bank_category":["Food and Drink","Restaurants"]}'
```

The CLI prints pretty JSON to stdout, diagnostics to stderr, and exits nonzero on
validation, configuration, or provider errors. It uses the same environment
variables and default SQLite database as the API. Provide `--merchants FILE`,
`--model MODEL`, or `--threshold NUMBER` to override settings. Set
`TYPESAFE_API_KEY` in the environment for Jev evaluation.

Read a complete request from a file or stdin:

```sh
cargo run -- enrich --input transaction.json
printf '%s' '{"description":"LS","extra":{"notes":"Possibly lunch"}}' | \
  cargo run -- enrich --input -
```

Use `--dry-run` to validate and inspect the input without a provider call:

```sh
cargo run -- enrich 'LS' --country CA --dry-run
cargo run -- enrich --help
```

Enrich multiple transactions through the same core batching path:

```sh
cargo run -- enrich-batch --input transactions.json
curl -X POST http://localhost:3000/v1/enrich/batch \
  -H 'Content-Type: application/json' --data-binary @transactions.json
```

Both accept `{"transactions":[{"description":"EXAMPLE CAFE","country":"CA"}, ...]}`
with 1–100 transactions and a 1 MiB body limit. Results appear in input order as
`{"results":[{"status":"success","data":{"merchant":...}},
{"status":"error","code":"enrichment_failed","message":"..."}]}`.
Validation and provider errors are reported per transaction; the CLI prints all
results and exits nonzero if any failed. Invalid JSON or an invalid batch envelope
rejects the whole request. The bulk API has a 55-second overall deadline; exceeding
it returns 504 for the whole batch.

Single enrichment, bulk enrichment, and `eval --mode enrich` share the same
provider logic. Verified exact matches and empty shortlists resolve locally.
Remaining transactions become independent named Choice questions in Jev's
`/v1/systemone` request, with each transaction's evidence inside its own question
and an empty shared state. Requests hold up to 32 questions, with conservative
48 KiB encoded request and 24 KiB per-question budgets, and at most four provider
calls in flight per batch. These byte budgets leave context headroom rather than
estimating model tokens. Oversized question evidence returns a per-item error;
it is never silently truncated. Catalog imports continue to update merchant
knowledge without calling the provider.

The question packing uses TypeSafe's documented [parallel questions](https://docs.typesafe.ai/introduction)
and [structured instructions](https://docs.typesafe.ai/primitives/advanced).

Optionally install the binary for shorter commands:

```sh
cargo install --path crates/ultrafinance-cli
ultrafinance enrich 'LS' --country CA
```

## Transaction locations

Enrichment returns independent top-level `merchant` and `location` results.
Location status is `matched` for a supported catalog outlet, `extracted` for
partial geography or a store identifier, and `unresolved` with null data when
there is insufficient evidence. A merchant can match without a location, and
geography can be extracted without identifying a merchant. Successful responses
always include `location`, including single, batch, and CLI enrichment.

```json
{
  "merchant": {"status": "unresolved", "data": null},
  "location": {
    "status": "extracted",
    "data": {
      "id": null,
      "precision": "city",
      "address": null,
      "city": "Hialeah",
      "region": "FL",
      "postal_code": null,
      "country": "US",
      "store_number": "10241"
    }
  }
}
```

The initial descriptor extractor recognizes a small reviewed list of city/region
suffixes in the US, Canada, and Australia. It preserves store identifiers such as
`#0006` and `STORE R483`, ignores card/reference markers, and abstains on known
billing/processor descriptions and truncated or ambiguous cities. It does not
geocode descriptions, infer coordinates, or copy addresses from merchant records.
Precision is `country`, `region`, `city`, `address`, or `outlet`; it is null when
only a store number or postal code is known.

Supply optional structured transaction evidence through request `location`, or
through the CLI:

```sh
cargo run -- enrich 'CAFE PURCHASE' --location '{"city":"Bromont","country":"CA"}'
```

Structured evidence appears as `extracted` unless a unique outlet is identified.
Conflicting descriptor geography is discarded rather than combined with caller
fields. Existing top-level `country` remains a merchant retrieval hint and is
never copied into the location result. Outlet matching requires a matched
merchant plus a unique alias/pattern, store number, or street address, without
contradictory location evidence. Merely knowing a city does not pick an outlet.
Coordinates come only from reviewed outlet records and must be a valid pair.

Reviewed outlets are imported explicitly with `locations import FILE` and
inspected with `locations list MERCHANT_ID`. Records retain source identities,
place IDs, attribution, and optional protected manual corrections. Reimports
preserve service location IDs. Parent/child relationships and company addresses
are not automatically treated as transaction locations. See the
[starter catalog](data/locations/README.md) and separate location-only evaluation:

```sh
cargo run -- locations eval evals/location-smoke.json
```

SQLite adds outlet storage when opened. PostgreSQL requires migration 003 through
`database init`. This additive migration preserves the version-2 merchant/log
contract, so the previous application remains compatible and release rollback
continues to work. Apply it before deploying the location-enabled application.
The Lambda runtime role also needs `SELECT` on `location_records`;
the import role needs `SELECT`, `INSERT`, and `UPDATE`. This change does not run
migrations against production or modify infrastructure grants.

## Repeatable dataset imports

Prepare a local download with a source-specific adapter:

```sh
cargo run -- datasets import --source merchant-studio \
  --input data/imports/merchant-studio.json \
  --examples data/imports/merchant-studio-tests.json
cargo run -- datasets import --source open-enrichment \
  --input data/imports/open-enrichment-global.csv --region global
cargo run -- datasets import --source dodatathings --input data/imports/dodatathings.csv
cargo run -- datasets import --source moneyvis --input data/imports/moneyvis.csv
```

Download Merchant Studio's `merchant_aliases.json` and
`sample_test_descriptors.json` from its [public data directory](https://jtvargas.github.io/merchant-studio/data/index.json).
Other adapters accept the native CSV exports from
[Open Enrichment](https://github.com/steveharrison/openenrichment),
[DoDataThings](https://huggingface.co/datasets/DoDataThings/us-bank-transaction-categories-v2),
and [MoneyData / MoneyVis](https://data.mendeley.com/datasets/dnxtg6n4rv/1).
Importing is offline and does not call Jev or change the merchant database.

Each command prints a versioned bundle path under `data/datasets/`. Its manifest
records attribution, license, input fingerprints, adapter version, and counts.
Bundles contain `knowledge.json`, `development.jsonl`, and `holdout.jsonl`, plus
`*.eval.json` suites when merchant labels are available. The fixed split assigns
approximately 80% of normalized descriptions to development and 20% to holdout.
Duplicates share a split across sources; input ordering does not affect it.
New downloads produce new bundles. Identical downloads reuse the existing bundle
and preserve any labels you have edited. Raw downloads and prepared data are Git-ignored.

Apply the printed bundle's merchant knowledge explicitly, then benchmark retrieval:

```sh
cargo run -- datasets apply <BUNDLE>/knowledge.json
cargo run -- eval <BUNDLE>/holdout.eval.json --mode search \
  --output evals/reports/source-holdout.json
```

Refreshes preserve local merchant IDs and manual overrides. Existing merchants
from different sources remain distinct until explicitly linked with `merchants link`.
Held-out descriptions are excluded from that bundle's imported aliases and raw
example evidence. Merchant names and other independent source knowledge remain
available, so these source-derived suites measure consistency, not independent
real-world accuracy. Keep a separately labeled real-world holdout for that.

Merchant Studio and Open Enrichment provide merchant labels. A missing merchant
label is retained as unlabeled, never assumed to mean `unresolved`. Open Enrichment
child places are retained as unlabeled examples but excluded from the brand catalog;
its icons are not imported.

Open Enrichment's retained `transaction_text_regexp` rules also generate search
candidates from original descriptors and a form with common payment processor
prefixes removed. Exact aliases rank first, followed by regex hits ordered by
matched substring length, then fuzzy candidates. Equally specific rules remain
ambiguous; regex hits still go through Jev rather than bypassing it as trusted
manual aliases do. Invalid, empty-matching, oversized, or unsupported PCRE rules
(such as lookaround and backreferences) are ignored by the bounded Rust regex
engine. Existing imported bundles benefit without reimporting.

DoDataThings is synthetic category-labeled data; MoneyVis has real descriptions
without merchant labels. MoneyVis account numbers,
sort codes, and balances are discarded. Repeated descriptions are deduplicated,
so output counts differ from transaction row counts.

For manual labeling, set a sample's `expected` to either
`{"status":"matched","merchant":{"source":"merchant-studio","external_id":"ID"}}`
or `{"status":"unresolved"}`, then export the labeled subset:

```sh
cargo run -- datasets export-eval <BUNDLE>/holdout.jsonl --output evals/private/labeled.json
```

To measure coverage without merchant labels, evaluate the JSONL samples directly:

```sh
cargo run -- eval <BUNDLE>/holdout.jsonl --samples --mode search \
  --output evals/reports/coverage-search.json
cargo run -- eval <BUNDLE>/holdout.jsonl --samples --mode enrich --limit 100 \
  --output evals/reports/coverage-enrich.json
```

Search mode reports **candidate coverage**, the proportion with at least one
retrieved candidate. Enrich mode runs the actual matcher (including Jev when needed)
and reports matched, unresolved, errors, and **match rate** over all cases. Each
result has `matched: true/false`; it is `null` for search-only runs or errors.
Unlabeled results have `correct: null` and never contribute to accuracy or match
precision. Labeled cases in a mixed sample file still contribute to those metrics.
Errors remain in the match-rate denominator and are counted separately from unresolved.
Use `--limit` to try a smaller run before evaluating the full dataset with provider calls.
Coverage does not establish whether predicted merchants are correct.

Unlabeled samples are omitted by `datasets export-eval`. Development samples can be used for matching rules
or future training; this importer does not train a model.

For search profiling, run:

```sh
ULTRAFINANCE_PROFILE_SEARCH=1 cargo run -- eval --all --limit 500
```

This prints aggregate SQLite timings for exact, token, and trigram SQL,
JSON decoding, fuzzy scoring, and provenance reads. It logs no descriptors or
merchant records. The flag is off by default. `--limit` applies to each suite;
use an unrestricted run for the complete benchmark. Enrichment evaluations
reuse the measured candidate shortlist rather than searching twice per case.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `ULTRAFINANCE_BIND` | `127.0.0.1:3000` | Listening address |
| `ULTRAFINANCE_DATABASE_URL` | unset | PostgreSQL URL; takes precedence over local SQLite and API JSON catalogs |
| `ULTRAFINANCE_REQUIRE_POSTGRES` | unset locally; `true` in Docker | Refuse API startup without PostgreSQL |
| `ULTRAFINANCE_DB` | `data/ultrafinance.sqlite` | Local SQLite fallback |
| `ULTRAFINANCE_MERCHANTS` | unset | Optional JSON catalog instead of SQLite |
| `TYPESAFE_API_KEY` | unset | Jev credential |
| `JEV_MODEL` | `jev-latest` | Jev model |
| `ULTRAFINANCE_MATCH_THRESHOLD` | `0.95` | Minimum chosen probability and model confidence |

The threshold is provisional, not an accuracy guarantee. Evaluate against labeled
transactions before trusting matches automatically. Search retrieves the top 10 candidates for Jev, plus any exact alias collisions.
The database can contain more than 254 merchants; Jev receives at most 254
candidates, reserving its final choice for insufficient evidence.

## PostgreSQL and production imports

Set `ULTRAFINANCE_DATABASE_URL` securely in your environment. Production URLs
should require TLS (`sslmode=require`) and use the provider's trusted certificate.
The client validates certificates and hostnames. For a disposable local server,
`postgresql://localhost/ultrafinance?sslmode=disable` is supported.
The application image installs Canada Central's public RDS CA certificates from
`deploy/rds-ca/` so Aurora's TLS certificate can be validated.

Initialize with a schema administration role, then migrate the authoritative
SQLite catalog with a write role:

```sh
cargo run -- database init
cargo run -- database migrate-sqlite data/ultrafinance.sqlite
cargo run -- merchants list --json
```

Migration requires an empty destination catalog and opens SQLite read-only.
It copies source records, links, local IDs, manual overrides, and logos in one
transaction, then rebuilds search indexes and verifies the complete catalog fingerprint before committing. Do not seed production from
`deploy/catalog.json` or `export-catalog.py`: those flattened exports do not
preserve the full import and correction history. Keep the source SQLite backup
until the production catalog has been verified. Migration refuses to overwrite
an existing catalog.

Once connected to production, the usual commands update the shared database:

```sh
cargo run -- datasets apply data/datasets/YOUR_BUNDLE/knowledge.json
cargo run -- merchants import data/imports/merchant-studio.json --format merchant-studio
cargo run -- merchants add --id mer_EXISTING --name 'Corrected merchant' --country CA
cargo run -- merchants link --source merchant-studio --external-id SOURCE_ID --merchant-id mer_EXISTING
```

For private Aurora access without a management instance, open an interactive
shell in the existing Ultrafinance image on an on-demand ARM64 Fargate task:

```sh
python3 deploy/prod-cli.py
# Inside the container:
ultrafinance merchants list --json
exit
```

CLI infrastructure is provisioned automatically with Aurora and the application
image when you apply with `./infra/tofu.sh`. Install the AWS CLI and
[Session Manager plugin](https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html)
on your machine, and run the shell command in an interactive terminal.
The runner uses ECS Exec to open `/bin/sh` in the immutable image behind
Lambda's `live` alias. Tasks use the existing private subnets and NAT gateway
without a public IP. The runner stops the task when the shell closes, including
on connection failure or interruption; tasks also expire after one hour.

The shell automatically receives the same `ULTRAFINANCE_DATABASE_URL` as
Lambda. Set `database_url` in the private infrastructure variables and apply
once; OpenTofu populates the shared Secrets Manager URL used by ECS. Use a
non-administrator application login with catalog read/write permissions so
both the API and CLI can use it. Schema administration remains separate.
Use `--image ECR_REPOSITORY@sha256:DIGEST` to select a newer image before an
initial PostgreSQL cutover. The shell filesystem is temporary.

Imports validate the batch before writing and commit atomically. Writers use a
transaction advisory lock to serialize imports, corrections, and links across
processes. API reads use consistent snapshots and see committed changes on the
next request. PostgreSQL uses indexed token-prefix and substring-trigram
retrieval; the final Rust similarity score, provenance, negative aliases, and
verified exact-match rules remain the same. Candidate ranking may differ from
SQLite FTS, so compare held-out evaluations before production cutover.

Schema creation is an explicit CLI operation, never an API startup side effect.
Use a shared application role with `CONNECT`, schema `USAGE`, and `SELECT`,
`INSERT`, `UPDATE`, and `DELETE` on application tables for Lambda and the CLI.
Use a separate administration role to install `pg_trgm` and run schema
migrations. Configure managed database backups and a connection budget before
cutover: each Lambda execution environment opens at most one connection on its
first catalog operation. Static pages and health checks do not connect to the
database. PostgreSQL closes sessions idle for 60 seconds; before each operation
the worker probes the connection and reconnects if necessary. It never replays
a job after execution begins, so uncertain write outcomes aren't duplicated.
Size Lambda concurrency to the database's connection budget. RDS Proxy keeps
connections open and prevents Aurora auto-pause, so it is not used here.

Aurora Serverless v2 can pause after five minutes without connections with
`aurora_min_acu = 0`. With idle session expiry, that is roughly six minutes after
the last database activity, provided no other clients hold connections open.
The app allows 35 seconds to connect and 50/55 seconds for catalog/enrichment
requests, bounded by 60-second Lambda/CloudFront limits. The browser waits 65
seconds. The first lookup after a pause can take 15–30+ seconds to resume;
unusually slow resumes can still time out and require a fresh request.

The PostgreSQL integration test runs against an **empty disposable database**:

```sh
ULTRAFINANCE_TEST_DATABASE_URL=postgresql://localhost/ultrafinance_test?sslmode=disable \
  cargo test -p ultrafinance-core postgres_ -- --ignored
```

## Merchant database and search

```sh
cargo run -- merchants add --name 'Julius Café' --country CA \
  --alias 'JULIUS CAFE BROMONT'
cargo run -- merchants list
cargo run -- merchants list --limit 50 --offset 50
cargo run -- merchants search 'Julus cafe' --country CA
cargo run -- enrich 'JULIUS CAFE BROMONT' --country CA
```

`merchants list` (alias `ls`) displays an alphabetic table of IDs, names,
countries, websites, and alias counts. Use `--json` for complete merchant records
with `total`, `limit`, and `offset` for pagination. The default limit is 50 (maximum
1000). `--country CA` filters declared countries only; imported country hints
remain evidence and do not count as a declared country. Listing never calls Jev.

Generated merchant IDs remain stable. To replace a record and its aliases, use
`merchants add --id EXISTING_ID ...`. Import an existing JSON catalog with
`merchants import FILE`. Imports update source/external ID pairs; repeat imports preserve local IDs.
Imported aliases remain unverified and cannot bypass Jev. Manual names, country,
website and verified aliases take precedence over imported records. `--database FILE` overrides the local SQLite path globally. `ULTRAFINANCE_DATABASE_URL` (or `--database-url`) selects PostgreSQL for merchant commands, dataset application, enrichment, and evaluations. Prefer the environment variable so credentials do not appear in shell history.

Search removes accents, folds case, and normalizes punctuation and whitespace.
Exact aliases are indexed separately; SQLite FTS5 token/prefix and trigram
indexes retrieve a bounded fuzzy pool. Rust ranks candidates using edit distance
and token overlap. Search scores are retrieval scores, not match probabilities.
Short descriptors such as `LS` don't generate fuzzy candidates from letters alone.
A unique exact match of at least three normalized characters resolves locally;
ambiguous exact aliases and fuzzy results go to Jev. Country narrows retrieval
while records with unknown country remain eligible.

## Merchant logos

Merchants optionally include `logo_url` and `logo_source`. Add a verified brand
logo URL manually:

```sh
cargo run -- merchants add --name 'Example Café' --country CA \
  --logo-url 'https://example.com/logo.png' --logo-source 'official website'
```

The source defaults to `manual`. Native imports accept the same optional fields,
with the import namespace as the default logo source. Manual records and imported
source records keep their logo fields independently, so an import refresh cannot
overwrite a manually set logo. Existing records without logos remain valid.
Logos appear in JSON records and enrichment responses, not as images in the
terminal table. URLs are validated as HTTP(S); the service does not download or
verify the image. Merchant Studio's `iconSlug` remains source evidence and does
not become a guessed logo URL. Automatic discovery is not implemented yet.

## External datasets

Merchant Studio has a dedicated adapter. Download its `merchant_aliases.json`
from the [source repository](https://github.com/jtvargas/merchant-studio), then:

```sh
cargo run -- merchants import data/imports/merchant-studio.json --format merchant-studio
cargo run -- merchants search 'amzn mktp' --country CA
```

Source records retain the original JSON, namespace, external ID, dataset version,
attribution and license. Search output includes `provenance`, and `trusted` tells
whether the query matched a manually verified name or alias. Country hints stay
in source evidence; they do not claim the merchant operates only in those countries.
Negative aliases exclude contradictory imported candidates. Matches derived from
source-linked merchants include `attributions` in enrichment output.

No automatic cross-source merge is made using names alone. To link an external
record to a merchant you have verified:

```sh
cargo run -- merchants link --source merchant-studio \
  --external-id amazon --merchant-id mer_YOUR_LOCAL_ID
```

Links survive refreshes. `merchants add --id ...` sets manual corrections; importing
again replaces only the external record, not those corrections. Import batches
are validated and committed atomically. Absent records are retained until an
explicit removal feature is added. For native JSON, `--source NAME` sets the
external namespace (default `catalog`); its input IDs are source IDs, not forced
local IDs. Use a stable source name across refreshes.

Merchant Studio data: **Enrichment from Merchant Studio by Jonathan Taveras**,
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
Adapter transformations normalize website URLs, preserve evidence, and assign
local IDs. The downloaded snapshot and local SQLite database are ignored by Git.
The dataset's own confidence values are evidence, not our measured accuracy.

## Evaluations

Run the top-level suites in `evals/` and each prepared dataset's latest holdout
(snapshot manifest modification time, separately for each region) together:

```sh
cargo run -- eval --all
cargo run -- eval --all --mode enrich --limit 100
```

The default search run is offline. Enrich mode runs actual matching and may call
Jev; `--limit` applies to each suite. The CLI shows completed/total cases and elapsed time while each suite runs, then a summary
table with case counts, labels, candidate coverage, retrieval recall, match rate,
accuracy, and errors. Totals are calculated across cases, not averaged across
suites. Unlabeled cases contribute to coverage and match rate; accuracy uses only
labeled cases. Search mode leaves matching metrics unmeasured.

Every run saves individual reports and `summary.json` in a unique directory under
`evals/reports/all/`. Use `--output DIR` to change the batch report directory,
`--datasets-dir DIR` to change snapshot discovery, or `--suites-dir DIR` for suite
files. Batch discovery excludes development files, older snapshots, nested report
directories, and temporary preparations. All inputs are validated before provider
calls. Execution failures are displayed, remaining suites continue, and the command
exits nonzero if any suite or case errors. Reports identify the exact input paths.


Maintain a labeled JSON suite: each case contains a transaction request and an
explicit expected merchant or `unresolved`. Missing labels are rejected. See
[`evals/smoke.json`](evals/smoke.json) for a small synthetic example, not an
accuracy benchmark representative of real transactions.

```sh
# Offline candidate retrieval: no Jev calls, regardless of configured credentials.
cargo run -- eval evals/smoke.json --output evals/reports/search-baseline.json

# Entire enrichment pipeline, including Jev when required.
cargo run -- eval evals/smoke.json --mode enrich \
  --output evals/reports/enrich-baseline.json
```

Enrich mode sends the suite's transaction data to Jev when evaluation requires it,
and consumes provider usage. Cases run sequentially with the normal request
limits and thresholds. Provider failures are recorded individually and do not
silently become correct abstentions. All requests and labels are validated before
provider evaluation begins.

| Metric | Meaning |
| --- | --- |
| Candidate recall | Known-merchant cases where retrieval included the correct merchant / all known-merchant cases |
| Top-1 recall | Known-merchant cases where the correct merchant ranked first / all known-merchant cases |
| Match rate | Returned matches / all cases |
| Match precision | Correct merchant matches / returned matches |
| Merchant recall | Correct merchant matches / all known-merchant cases |
| Accuracy | Correct merchant matches plus correct unresolved results / all cases |
| False matches | Returned matches with the wrong identity, including cases expected to be unresolved |
| Errors | Provider or enrichment failures; counted against overall accuracy |

Rates in saved JSON are fractions from 0 to 1; `null` means unavailable or no
applicable denominator. Search mode reports retrieval metrics only, not final
matching accuracy. Its candidate list includes exact collisions in addition to
the nominal top 10, matching normal enrichment behavior. Missing expected source
references count as retrieval misses and failed known-merchant matches, not as
correct abstentions.

A case can identify a merchant by local ID or by a stable external reference:

```json
{
  "id": "amazon-typo",
  "request": {"description": "amazn marketplace", "country": "CA"},
  "expected": {
    "status": "matched",
    "merchant": {"source": "merchant-studio", "external_id": "amazon"}
  }
}
```

Use `"merchant": {"merchant_id": "mer_..."}` for manually added merchants.
For genuinely insufficient evidence use `"expected": {"status": "unresolved"}`.
Don't label a known merchant unresolved simply because the database lacks it.
External references follow explicit source links to local IDs.

Start a private suite in `evals/private/` with 50–100 independently verified real
examples: exact aliases, typos, processor prefixes, similar merchant names,
conflicting country hints, and ambiguous abbreviations. Keep a fixed holdout
suite separate from examples used to add aliases or tune thresholds. Verified
merchant identity should come from independent evidence rather than the matcher
or imported dataset being evaluated. Grow coverage as bugs are found, tracking
which cases were used for tuning. Include a natural mix of cases; report curated
stress cases separately rather than treating their accuracy as production accuracy.

Enrichment evals process windows of up to 100 cases and reuse their retrieved
shortlists. Enrichment case latency includes waiting within that window for
retrieval and shared provider calls; use the suite wall time to assess throughput.
Search-only latency remains the individual catalog lookup time.

Reports include per-case ranks, predictions, correctness, errors, latency,
suite/database/code fingerprints, model and threshold. Save reports for each
change and compare the same suite and database snapshot when isolating algorithm
improvements. Compare database fingerprints too when measuring coverage growth.
Evaluation fails if the database changes during the run. No aliases or links are
learned or modified by evaluation. `evals/private/` and `evals/reports/` are ignored
by Git; reports still contain labels and merchant identifiers.

## Enrichment history

Every transaction submitted to the enrichment core is recorded in `enrichment_log`
for API, CLI, batch, and enrichment evaluations. Entries include the full input,
ranked candidate snapshots with scores and provenance, exact/provider/no-candidate
method, model and threshold, the individual provider answer (including confidence),
and final response or error. Each attempt has its own ID, batch ID and input index,
with UTC start and completion timestamps. Repeated transactions keep separate entries.
A `started` row without a completion timestamp indicates an interrupted or still
running attempt, including an API deadline. Requests rejected before reaching the
core, dry runs, and search-only evaluations do not create entries.

```sh
cargo run -- logs --limit 50
cargo run -- logs --status unresolved
cargo run -- logs --status error
cargo run -- logs --merchant-id mer_example --limit 100 --offset 0
```

Output is JSON, newest first; limit is 1–1000. Inspection uses the configured
SQLite or PostgreSQL database and is available through the CLI, with no public
history endpoint. Input `extra` and candidate evidence are retained as supplied.
History has no automatic expiry. A log write failure fails enrichment so a result
is never reported as successfully completed without its history being saved.

Existing SQLite databases gain the table when opened. For PostgreSQL, run
`cargo run -- database init` with the schema administration role to apply migration
002 before running the updated application. The runtime role also needs `SELECT`,
`INSERT`, and `UPDATE` on `enrichment_log`. This migration preserves the catalog.

## Current scope

This version stores merchants and aliases in PostgreSQL (or local SQLite) and evaluates retrieved
candidates. It does not yet discover merchants through web research, use
embeddings, or cache transaction results. Retrieval currently uses the description
and country; Jev considers all supplied fields and `extra` when evaluating the
shortlist. No candidates means `unresolved`; fuzzy candidates without a configured
Jev key produce a configuration error. The server defaults to a local binding.
The API is public without client authentication. The function URL setup has no
application request throttle.
Provider requests have a 20-second timeout,
and enrichment has a 55-second overall deadline, including database resume. Enrichment history retains each transaction’s provider answer in the database; credentials are never included by the logging code. Store operations during API enrichment run on Tokio's
blocking thread pool.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Tests run locally without credentials or live provider calls.

## Hosting

See [the OpenTofu deployment guide](infra/README.md) for CloudFront, a Lambda function URL,
and a Rust container on Lambda using the `joehoyle` AWS profile. The initial
storage approach packages a SQLite catalog snapshot into each image. After the
initial infrastructure setup, `./deploy/deploy.sh` builds, checks and promotes a
release through the stable `live` alias. GitHub Actions can run the same release
process from `main` using AWS OIDC.

## Merchant explorer

The website browses the live catalog using `GET /v1/merchants`. Optional query
parameters are `q` (name or alias), `country` (uppercase two-letter code),
`limit` (1–100, default 20), and `offset` (0–1000000, default 0). The response
contains `merchants`, `total`, `limit`, and `offset`. Browsing is alphabetical
and paginated across the full catalog. Search paginates a ranked shortlist of
up to 100 candidates (up to 255 for exact alias collisions); its total describes
that shortlist. Search includes merchants with unknown country when a country
is supplied. Browsing filters to the specified country exactly. Catalog queries
do not call the AI provider. Cards show merchant IDs, websites, and aliases;
“Try lookup” fills the enrichment form without submitting it.

The development profile optimizes the enrichment core, JSON/edit-distance dependencies, and bundled SQLite engine
while retaining debug symbols, so offline evals are practical with `cargo run`.
Search reuses query tokenization and similarity scores within each request and
scores each retrieved merchant once. For production timing, use `cargo run --release`.
