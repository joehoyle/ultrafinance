# Database lookup columns

Schema version 8 stores fixed catalog and matching fields in columns in
PostgreSQL. Application lookups and sorting use those columns and
indexes. JSON remains for variable evidence, source payloads and arrays whose
searchable values have relational indexes.

| Table | Column fields and relationships |
| --- | --- |
| `merchants`, `manual_merchants` | ID, name, website, logo URL/source, markets, aliases and sources; name ordering and country-list indexes on `merchants` |
| `merchant_identity_keys` | Shared Rust-normalized name and parsed host, plus deterministic name/host keys; B-tree rule lookup and name/host indexes, with a trigram candidate index |
| `catalog_revision` | Transactional revision advanced by catalog-write triggers; guards provider dedupe against concurrent edits |
| `aliases` | Merchant ID and normalized alias; indexed exact-name lookup |
| `source_records` | Source identity and metadata; typed input name, website, logo, markets, aliases, sources, country hints and dataset region; composite source identity and pattern-selection index |
| `location_records` | Source identity, merchant or source-merchant reference, name, precision, address, city, region, postal code, country, store number, coordinates, pattern, manual override and attribution fields; merchant, country/city and merchant/store-number indexes |
| `descriptor_resolutions` | ID derived from context hash, merchant FK, description, country, amount, currency, supplied location fields, verified flag, review evidence, created and updated timestamps; merchant/status and description/country indexes |
| `enrichment_log` | Existing ID, batch ID, status, merchant ID and timestamps; status/merchant/time indexes |
| `merchant_redirects` | Retired ID and surviving merchant ID |
| `merchant_merge_runs` | Run ID; the remaining audit JSON is not used for filtering |

Source refreshes and import rebuilds read typed fields and country arrays
without parsing source JSON. Full source payloads and duplicate merchant JSON are not stored. Source identities,
versions, attribution, licenses and typed matching fields are retained; source
documents are reconstructed from them. Re-download or re-import the original
dataset when other fields are needed. Raw projections contain only country hints,
negative aliases, transaction patterns and parent references. Changes only to
discarded fields do not trigger catalog updates. Catalog
arrays preserve declarations and display metadata; alias lookup uses `aliases`
and country filtering uses the GIN index on `merchants.markets_json::jsonb`. Mapping JSON contains only
variable `extra` context and provenance, not a duplicated merchant snapshot.
Historical enrichment and merge logs retain full audit payloads.

`*_documents` views reconstruct the existing application JSON representations
from columns. Their `data` values are computed read projections, not stored
catalog blobs. Changing a scalar column changes its representation immediately.
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

Grant the runtime role SELECT on the document views and SELECT, INSERT, UPDATE
and DELETE on the underlying application tables, including the new market, identity-key, catalog-revision and resolution tables.
Revision triggers require UPDATE on `catalog_revision`. Set the owner's default privileges for future tables/views
if that is the established database role policy. Credentials belong in the
configured environment, not command arguments.

This migration changes the stored schema. Earlier application versions cannot
run against version 8; coordinate the database upgrade with the new application
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
tables and merchant evidence JSON have been removed. No upgrade migration for
the former evidence layout is provided; existing databases require recreation.
