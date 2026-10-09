# Enrichment performance

Start the local PostgreSQL server with `./dev/postgres.sh up` from the workspace
root, then run:

```sh
cargo bench --locked -p ultrafinance-core --bench enrich
```

The only transaction is `SQ* JULIUS BROMONT`, with no country or other context.
The benchmark uses the existing local catalog. Set `ULTRAFINANCE_DATABASE_URL`
to select another local catalog (only loopback hosts are accepted); the benchmark does not load `.env` files. Use the same
catalog and machine when comparing changes. An empty catalog is supported and
prints a notice because it does not represent populated retrieval performance.

Stages measure normalization, descriptor interpretation (including geography),
candidate retrieval, and read-only local enrichment. Local enrichment includes
retrieval, exact-match handling, location extraction, and interpretation. Fuzzy
matches remain unresolved: these timings exclude TypeSafe/discovery calls,
HTTP handling, audit writes, and descriptor learning.

The first local enrichment is reported separately, after database connection and
runtime setup. It includes process-local lazy initialization, but is not a cold
database measurement. Each stage then calibrates its batch size and collects
30 warm samples, reporting mean, p50, and p95 in microseconds per operation.
Percentiles describe batch averages, not individual request tail latency.
Errors fail the benchmark instead of becoming fast successful samples.

Include end-to-end enrichment through Jev against that same local database:

```sh
set -a
source .env
set +a
cargo bench --locked -p ultrafinance-core --bench enrich -- --jev
```

Skip the environment-loading lines if `TYPESAFE_API_KEY` is already exported.
This adds three individual requests through the production enrichment path,
including retrieval, Jev evaluation, location extraction, configured discovery,
audit logging, and persistence. It uses the CLI defaults for `JEV_MODEL` and
`ULTRAFINANCE_MATCH_THRESHOLD`, and the same 55-second deadline. Database/runtime
and client setup are excluded; API HTTP handling is also excluded.
Use `--jev-samples N` to change the request count (1–100). There are no extra
provider warmup or calibration calls. Provider calls consume API credits and
successful results can create remembered mappings in the local database.

Each request reports its decision method and observed Jev call count. The
production path can bypass Jev for exact/verified matches or empty retrieval;
the benchmark reports that explicitly instead of forcing a different path.
Mean, p50, and p95 describe individual end-to-end requests, including the first;
three samples provide a quick check, not a robust tail-latency estimate.

## Full HTTP throughput

Build the API in release mode, then start it against the local catalog with
provider credentials exported. The API does not load `.env` automatically.

```sh
cargo build --locked --release -p ultrafinance-api
set -a
source .env
set +a
ULTRAFINANCE_DATABASE_URL='postgresql://ultrafinance@127.0.0.1:55432/ultrafinance?sslmode=disable' \
ULTRAFINANCE_BIND=127.0.0.1:3001 target/release/ultrafinance-api
```

In another terminal, run the standard-library Python load generator:

```sh
python3 dev/bench-http.py --requests 1
python3 dev/bench-http.py --concurrency 32 --requests 512 --output /tmp/http-enrich.json
```

It uses persistent HTTP/1.1 connections with one outstanding request per worker
(closed-loop load), and reports successful requests per wall-clock second,
individual latency percentiles, HTTP status counts, and merchant outcomes.
Only local HTTP targets are accepted. Every request follows the real API path,
including audit writes and any configured providers; these runs consume provider
credits and write local enrichment history. The default descriptor is
`SQ* JULIUS BROMONT`; use `--body` to change the JSON payload.

Run a single preflight before the measured sweep to initialize the server. Check
the corresponding `enrichment_log.data` records for `method = provider` and
`provider_requests[].status = sent`: repeated descriptors can bypass Jev if
catalog aliases or reviewed mappings resolve them locally. Successful HTTP 200
responses can be either matched or unresolved. This benchmark rejects other
merchant outcomes or malformed responses and exits nonzero if requests fail.

Sweep concurrency sequentially and repeat longer runs near the throughput
plateau. Measurements include connection setup and the final drain. Short runs,
a repeated descriptor with warm retrieval caches, local database transport, and
shared load-generator/server hardware limit how well results predict varied
production traffic. These measurements exclude Lambda and CloudFront.
