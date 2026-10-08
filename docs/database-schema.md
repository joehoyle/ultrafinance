# Database lookup columns

Schema version 4 stores fixed catalog and matching fields in columns in
PostgreSQL. Application lookups and sorting use those columns and
indexes. JSON remains for variable evidence, source payloads and arrays whose
searchable values have relational indexes.

| Table | Column fields and relationships |
| --- | --- |
| `merchants`, `manual_merchants` | ID, name, website, logo URL and source; name ordering index on `merchants` |
| `aliases` | Merchant ID and normalized alias; indexed exact-name lookup |
| `source_records` | Source, external ID, merchant ID, version, attribution, license, URL, transaction pattern and parent ID; composite source identity and pattern-selection index |
| `location_records` | Source identity, merchant or source-merchant reference, name, precision, address, city, region, postal code, country, store number, coordinates, pattern, manual override and attribution fields; merchant, country/city and merchant/store-number indexes |
| `merchant_market_evidence` | Merchant ID, country, source, external ID, evidence kind and confidence; indexed country filtering |
| `descriptor_resolutions` | ID derived from context hash, merchant FK, description, country, amount, currency, supplied location fields, verified flag, review evidence, created and updated timestamps; merchant/status and description/country indexes |
| `enrichment_log` | Existing ID, batch ID, status, merchant ID and timestamps; status/merchant/time indexes |
| `merchant_redirects` | Retired ID and surviving merchant ID |
| `merchant_merge_runs` | Run ID; the remaining audit JSON is not used for filtering |

The source merchant snapshot and raw payload are retained as evidence. Catalog
arrays preserve declarations and display metadata; alias lookup uses `aliases`
and market lookup uses `merchant_market_evidence`. Mapping JSON contains only
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
Migrations backfill scalar values, preserve IDs and source links, and convert
legacy descriptor JSON into columns, following existing merchant redirects.
Invalid legacy rows cause rollback. Fingerprints use canonical application
serialization, so SQL JSON formatting does not change catalog identity.

Grant the runtime role SELECT on the document views and SELECT, INSERT, UPDATE
and DELETE on the underlying application tables, including the new market and
resolution tables. Set the owner's default privileges for future tables/views
if that is the established database role policy. Credentials belong in the
configured environment, not command arguments.

This migration changes the stored schema. Earlier application versions cannot
run against version 4; coordinate the database upgrade with the new application
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
