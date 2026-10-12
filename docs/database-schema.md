# Database lookup columns

Schema version 14 stores fixed catalog and matching fields in columns in
PostgreSQL. Application lookups and sorting use those columns and
indexes. JSON remains for variable evidence, source payloads and arrays whose
searchable values have relational indexes.

| Table | Column fields and relationships |
| --- | --- |
| `merchants`, `manual_merchants` | ID, name, website, logo URL/source, markets, aliases and sources; name ordering and country-list indexes on `merchants` |
| `merchant_identity_keys` | Shared Rust-normalized name and parsed host, plus deterministic name/host keys; B-tree rule lookup and name/host indexes, with a trigram candidate index |
| `catalog_revision` | Transactional revision advanced by catalog-write triggers; guards provider dedupe against concurrent edits |
| `merchant_search` | Normalized name/alias text and generated lexeme vector; full-text index retrieves bounded merchant pools |
| `merchant_search_words` | Distinct search lexemes with B-tree identity and GIN trigram indexes; fuzzy spelling expansion without merchant-sized trigram posting lists |
| `aliases` | Merchant ID and normalized alias; indexed exact-name lookup |
| `source_records` | Source identity and metadata; typed input name, website, logo, markets, aliases, sources, country hints and dataset region; composite source identity and pattern-selection index |
| `location_records` | Source identity, required canonical merchant ID and optional source-merchant reference, name, precision, address, city, region, postal code, country, store number, coordinates, pattern, manual override and shared source-provenance FK; merchant, country/city and merchant/store-number indexes |
| `source_provenance` | Shared source attribution, licence and dataset URL; distinct metadata variants are preserved |
| `descriptor_resolutions` | ID derived from context hash, merchant FK, description, country, amount, currency, supplied location fields, verified flag, review evidence, created and updated timestamps; merchant/status and description/country indexes |
| `enrichment_log` | Existing ID, batch ID, status, merchant ID and timestamps; status/merchant/time indexes |
| `merchant_redirects` | Retired ID and surviving merchant ID |

Source refreshes and import rebuilds read typed fields and country arrays
without parsing source JSON. Full source payloads and duplicate merchant JSON are not stored. Source identities,
versions, attribution, licenses and typed matching fields are retained; source
documents are reconstructed in Rust from them. Re-download or re-import the original
dataset when other fields are needed. Raw projections contain only country hints,
negative aliases, transaction patterns and parent references. Changes only to
discarded fields do not trigger catalog updates. Catalog
arrays preserve declarations and display metadata; alias lookup uses `aliases`
and country filtering uses the GIN index on `merchants.markets_json::jsonb`. Mapping JSON contains only
variable `extra` context and provenance, not a duplicated merchant snapshot.
Enrichment logs retain full audit payloads. Merge reports are returned to callers
and are not stored; merchant and location redirects preserve retired identities.

The application reads authoritative columns directly and maps PostgreSQL rows to
Rust structs. JSON is parsed only for variable fields such as alias arrays,
external place evidence, and resolution provenance/context. There are no
`*_documents` views or SQL conversions of complete catalog objects into JSON.
Normal writes must still use the store so derived alias/search/market indexes
stay consistent with source refreshes, links and manual corrections.

Market evidence is recomputed transactionally on source refresh, linking,
merchant changes and outlet imports. Its country index supports filtered browsing
without loading every merchant first. Missing market coverage still does not
exclude transaction candidates.

Descriptor mappings reference the current merchant. Deduplication moves their
foreign keys to the surviving merchant before retiring the duplicate. Verified
mappings require review evidence; model-derived results cannot overwrite a
reviewed identity or silently change an existing mapping to another merchant.

## Upgrade

PostgreSQL requires `ultrafinance
database init` with the schema-owner connection before running this version.
The version-14 migration drops all five document views after transferring their
explicit privileges to the main tables and the supporting tables needed for
location and resolution joins. Runtime reads now use typed row decoders. IDs,
source evidence, mappings, and catalog revisions are preserved. Initialization
is transactional; custom views depending on these projections cause a rollback
rather than being removed with CASCADE.
The version-13 migration backfills every outlet's canonical merchant ID from
its source reference, requires a merchant FK, and keeps source-linked outlet IDs
in sync through database triggers on source relinking. Unresolvable legacy
references abort the migration. Location reads use the canonical merchant index.
It moves location attribution, licence and URL into shared `source_provenance`
rows and preserves existing location projection grants. Dataset URL/licence
variants remain distinct; no outlet IDs or external place evidence are discarded.
Source references remain as provenance and continue following explicit links.
The version-12 migration adds a partial B-tree index on accepted business website
hosts. Import brand reconciliation uses it to find affected host buckets without
repeated scans of the complete identity table. It also refreshes all derived
identity keys using the current website safeguards, including untouched legacy
records. It does not change merchant IDs, source evidence, or the catalogue revision.
The version-11 migration backfills `merchant_search_words` from existing search
vectors and installs statement-level insert/update triggers. Imports and manual
corrections extend this derived vocabulary in the same transaction as search
rows; failed imports roll it back. Unused words may remain after deletion or
renaming, but all merchant IDs and evidence are retrieved from current rows.
Backfill and index creation take a schema-maintenance lock; run initialization
with imports stopped. It adds no merchant identities or source evidence.
The version-10 migration drops `merchant_merge_runs` and `location_merge_runs`,
including all historical reports and snapshots. Merge and import operations no
longer recreate or write these tables. Operation `run_id` values remain in returned
reports for caller correlation, without a persisted audit lookup.
The version-9 migration removes obsolete market-evidence columns and tables and
rebuilds document views without the retired `market_evidence` field. It preserves
merchant country coverage, source links and explicit view privileges, including
databases already marked version 8 with legacy view definitions.
The version-7 migration builds merchant identity keys in bounded 5,000-row pages
and installs catalog revision triggers. Application writes keep these keys in
sync with merchant refreshes and manual changes. Imports consult this index
within one transaction instead of loading catalog snapshots.
The version-6 migration removes stored source blobs after preserving matching inputs.
Dropping columns does not immediately shrink existing relation files; compaction
can reclaim that space separately. The version-5 migration backfills typed source inputs from
existing documents. This one-time backfill can take time on a large catalog;
run it while imports are stopped. Migrations preserve IDs and source links and convert
legacy descriptor JSON into columns, following existing merchant redirects.
Invalid legacy rows cause rollback. Fingerprints use canonical application
serialization, so SQL JSON formatting does not change catalog identity.

Grant the runtime role SELECT, INSERT, UPDATE
and DELETE on the application tables, including `source_provenance`, `merchant_search_words` and the market, identity-key, catalog-revision and resolution tables. INSERT into `source_provenance` also requires USAGE on its identity sequence.
Revision triggers require UPDATE on `catalog_revision`. Set the owner's default privileges for future tables/views
if that is the established database role policy. Credentials belong in the
configured environment, not command arguments.

This migration changes the stored schema. Earlier application versions cannot
run against version 14; coordinate the database upgrade with the new application
release. A rollback to an earlier binary also requires restoring the earlier
schema from a database backup. No production migration is performed by merely
editing this repository or running the disposable tests.

## Local development and tests

PostgreSQL is the only backend. `dev/postgres.sh up` starts the PostgreSQL 17
Compose service and initializes the local schema. CLI/API default to that server;
production sets `ULTRAFINANCE_DATABASE_URL` explicitly. The Compose volume retains
the development catalog across stops.

Tests create independent temporary PostgreSQL databases on the server specified
by `ULTRAFINANCE_TEST_DATABASE_URL`, or the local Docker server. The fixture role
needs permission to create databases and extensions. The last store clone closes
its worker and drops its own temporary database. Tests never implicitly use the
production database URL. CI provides PostgreSQL, and the Docker build's test stage
starts its own disposable server; the runtime image contains no database server.


The fresh schema stores operating countries in `markets_json`. Country evidence
tables and merchant evidence JSON have been removed. Version 9 migrates the former evidence layout while preserving country coverage.

## Source inputs and projections

`source_records` retains import identity and matching inputs:
refreshes rebuild merchants from these inputs, regex matching uses source patterns,
and negative aliases exclude incorrect matches. `manual_merchants` retains reviewed
inputs separately so source refreshes cannot overwrite corrections or delete
manually created merchants. Removing either input store requires changing those
product behaviors or replacing its persistence; neither is an enrichment cache.

`place_ids_json` contains external provider evidence such as Apple and Google
place IDs for one outlet; these differ from the source's `external_id` and can
connect records from different datasets. Some providers repeat their own source
identity in this map, but deleting the map would discard independent evidence.

All Rust test code live under each crate's `tests/` directory. Modules in
`tests/unit/` are included as unit tests using `#[path]` to retain private-code
coverage; command-level integration tests remain at the directory root.
