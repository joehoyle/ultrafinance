# Ultrafinance

A small synchronous merchant enrichment API in Rust. Axum serves HTTP, and the
independent `ultrafinance-core` crate retrieves merchant candidates from SQLite
and uses Jev to evaluate fuzzy matches. Unique normalized exact aliases resolve
locally without a provider call.
One request returns a completed result; there are no background jobs.

## Run

```sh
cargo run -p ultrafinance-api
```

The server binds to `127.0.0.1:3000` and opens `data/ultrafinance.sqlite`.
An empty database returns `unresolved` without calling any provider.

Open `http://127.0.0.1:3000/` for the project website, copyable HTTP/CLI examples,
and an interactive merchant lookup. The form calls `/v1/enrich` on the same
origin. Static hosting needs a same-origin proxy for `/v1/enrich` to support
live lookups.
The standalone page lives in `website/index.html` and can also be hosted by any
static web server, without a frontend build step.

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
{"merchant":{"status":"unresolved","data":null}}
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

Optionally install the binary for shorter commands:

```sh
cargo install --path crates/ultrafinance-cli
ultrafinance enrich 'LS' --country CA
```

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `ULTRAFINANCE_BIND` | `127.0.0.1:3000` | Listening address |
| `ULTRAFINANCE_DB` | `data/ultrafinance.sqlite` | SQLite merchant database |
| `ULTRAFINANCE_MERCHANTS` | unset | Optional JSON catalog instead of SQLite |
| `TYPESAFE_API_KEY` | unset | Jev credential |
| `JEV_MODEL` | `jev-latest` | Jev model |
| `ULTRAFINANCE_MATCH_THRESHOLD` | `0.95` | Minimum chosen probability and model confidence |

The threshold is provisional, not an accuracy guarantee. Evaluate against labeled
transactions before trusting matches automatically. Search retrieves the top 10 candidates for Jev, plus any exact alias collisions.
The database can contain more than 254 merchants; Jev receives at most 254
candidates, reserving its final choice for insufficient evidence.

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
website and verified aliases take precedence over imported records. `--database FILE` overrides the SQLite path globally.

Search removes accents, folds case, and normalizes punctuation and whitespace.
Exact aliases are indexed separately; SQLite FTS5 token/prefix and trigram
indexes retrieve a bounded fuzzy pool. Rust ranks candidates using edit distance
and token overlap. Search scores are retrieval scores, not match probabilities.
Short descriptors such as `LS` don't generate fuzzy candidates from letters alone.
A unique exact match of at least three normalized characters resolves locally;
ambiguous exact aliases and fuzzy results go to Jev. Country narrows retrieval
while records with unknown country remain eligible.

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

Reports include per-case ranks, predictions, correctness, errors, latency,
suite/database/code fingerprints, model and threshold. Save reports for each
change and compare the same suite and database snapshot when isolating algorithm
improvements. Compare database fingerprints too when measuring coverage growth.
Evaluation fails if the database changes during the run. No aliases or links are
learned or modified by evaluation. `evals/private/` and `evals/reports/` are ignored
by Git; reports still contain labels and merchant identifiers.

## Current scope

This version stores merchants and aliases in SQLite and evaluates retrieved
candidates. It does not yet discover merchants through web research, use
embeddings, or cache transaction results. Retrieval currently uses the description
and country; Jev considers all supplied fields and `extra` when evaluating the
shortlist. No candidates means `unresolved`; fuzzy candidates without a configured
Jev key produce a configuration error. The server defaults to a local binding.
The API is public without client authentication. The function URL setup has no
application request throttle.
Provider requests have a 20-second timeout,
and enrichment has a 25-second overall deadline. No provider response bodies or
credentials are logged. SQLite operations during API enrichment run on Tokio's
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
