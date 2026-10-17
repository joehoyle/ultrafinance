# Database lookup columns

Schema version 18 stores fixed catalog and matching fields in columns in
PostgreSQL. Application lookups and sorting use those columns and
indexes. Merchant markets, aliases and sources and location aliases use native `text[]`
columns. JSON remains for keyed external place IDs (`jsonb`), variable resolution
evidence/context, and audit logs.

| Table | Column fields and relationships |
| --- | --- |
| `merchants`, `manual_merchants` | ID, name, website, logo URL/source, markets, aliases and sources; name ordering and country-list indexes on `merchants` |
| `merchant_identity_keys` | Shared Rust-normalized name and parsed host, plus deterministic name/host keys; B-tree rule lookup and host indexes, with a trigram candidate index |
| `catalog_revision` | Transactional revision advanced by catalog-write triggers; guards provider dedupe against concurrent edits |
| `merchant_search` | Normalized name/alias text and generated lexeme vector; full-text index retrieves bounded merchant pools |
| `merchant_search_words` | Distinct search lexemes with B-tree identity and GIN trigram indexes; fuzzy spelling expansion without merchant-sized trigram posting lists |
| `aliases` | Merchant ID and normalized alias; indexed exact-name lookup |
| `source_records` | Non-Foursquare source identity, version and shared provenance reference; typed input name, website, logo, markets, aliases, sources, country hints and dataset region; composite source identity and pattern-selection index |
| `location_records` | Source identity, required canonical merchant ID and optional source-merchant reference, name, precision, address, city, region, postal code, country, store number, coordinates, pattern, manual override and shared source-provenance FK; merchant, country/city and merchant/store-number indexes |
| `foursquare_unlocated` | Only the external place ID and canonical merchant ID for Foursquare places without an outlet |
| `source_identities` | View combining other providers’ source identities with Foursquare outlet and addressless identities; no copied data |
| `source_provenance` | Shared attribution, licence and dataset URL for source records and locations; distinct metadata variants are preserved |
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
and country filtering uses the GIN index on `merchants.markets`. Mapping JSON contains only
variable `extra` context and provenance, not a duplicated merchant snapshot.
Enrichment logs retain full audit payloads. Merge reports are returned to callers
and are not stored; merchant and location redirects preserve retired identities.

The application reads authoritative columns directly and maps PostgreSQL rows to
Rust structs. Lists are decoded directly as PostgreSQL arrays; JSON is decoded only for
external place maps and resolution provenance/context. There are no
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

## Foursquare identity-only storage (version 18)

Foursquare does not retain per-place merchant source evidence. Addressed places
use `location_records(source,external_id)` as their import key and reference the
current canonical merchant directly. `foursquare_unlocated(external_id,merchant_id)`
keeps only IDs for places without an outlet; an addressed refresh moves identity
to the outlet in the same transaction. `source_identities` is a non-materialized
union for source lookup, listings, and statistics. Other providers continue using
`source_records` and their existing evidence-based refresh behavior.

An existing Foursquare provider ID retains its merchant mapping. New IDs use the
shared deterministic matcher. A merchant supported by one Foursquare ID and no other source or manual input
refreshes directly from that incoming record. Shared merchant names/aliases/coverage
are combined on import; there is no historical source replay or automatic regrouping of existing
IDs from changed original names/websites. Outlet fields refresh in place, retaining
local outlet IDs and manual overrides. A refresh without a usable address retains
an existing outlet's last known location. Addressed merchants derive current country
coverage from outlets, other providers, and manual inputs; addressless identities
retain current merchant coverage because their original country is not stored.

Foursquare source listings return only provider ID and merchant mapping, marked
`identity_only`, rather than an original source document. Matching provenance uses
current merchant data and shared outlet attribution, marked `identity_only`; it is
not a historical provider snapshot. Per-place dataset version, input name/website,
input arrays, country hints, negative aliases and source regexes are not retained.

Migration 18 preserves merchant/outlet IDs and canonical ownership, detaches source
references from outlets, moves unmatched provider IDs into `foursquare_unlocated`,
and deletes Foursquare rows from `source_records`. A check constraint prevents
new evidence rows for Foursquare. This is intentional loss of original input
history, not a reversible normalization. Run `database init` with imports stopped;
use a backup to restore the old evidence if needed. Deleting rows makes their space
reusable after vacuum; it does not immediately shrink existing database files.

## Upgrade

PostgreSQL requires `ultrafinance
database init` with the schema-owner connection before running this version.
The version-17 migration shares source attribution, licence and URL through
`source_provenance`, preserving exact metadata variants and each source's version.
A composite foreign key prevents a source from borrowing another provider's
provenance. Source exports and canonical rebuilds join the shared metadata; refreshes
select/create a variant instead of changing metadata used by other source rows.
Existing explicit source reader/writer grants are extended to the provenance table
and, for writers, its identity sequence. The migration locks and updates the source
table, validates references and drops the three repeated text columns atomically.
Custom dependent views block the migration rather than being dropped automatically.
Allow maintenance time and disk space for the backfill and WAL. Existing table
files do not shrink just because columns were dropped; ordinary vacuum makes dead
space reusable, while returning it to the filesystem requires a separate rewrite.
Newly loaded catalogs get the compact layout immediately. No reset is necessary.

The version-16 migration drops two unused indexes: `merchant_search_trigrams`
(on the complete search text) and `merchant_identity_name` (a B-tree on normalized
names). Fuzzy retrieval uses the word dictionary and full-text index; identity
matching uses rule keys, website hosts and the separate normalized-name trigram
index. This migration does not rewrite tables or change evidence or IDs.

The version-15 migration converts merchant/manual markets, aliases and sources
and location aliases from JSON text to `text[]`, and external place maps to
`jsonb`. It renames the columns without the `_json` suffix, supplies empty-list/map
defaults, and replaces the country GIN expression index with a native array index.
API representations, list order, IDs, ownership and provenance are preserved.
This rewrites populated merchant and location tables and rebuilds their indexes;
allow maintenance time and free disk space. It runs transactionally, and invalid
legacy data or dependent custom views cause rollback. Regular runtime writes do
not need schema-owner privileges; the optional `--initial-load` mode does because
it rebuilds indexes and validates foreign keys before committing an empty-catalog
bootstrap.

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
run against version 18; coordinate the database upgrade with the new application
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


The fresh schema stores operating countries in `markets` (`text[]`). Country evidence
tables and merchant evidence JSON have been removed. Version 9 migrates the former evidence layout while preserving country coverage.

## Source inputs and projections

`source_records` retains import identity and matching inputs:
refreshes rebuild merchants from these inputs, regex matching uses source patterns,
and negative aliases exclude incorrect matches. `manual_merchants` retains reviewed
inputs separately so source refreshes cannot overwrite corrections or delete
manually created merchants. Removing either input store requires changing those
product behaviors or replacing its persistence; neither is an enrichment cache.

`place_ids` (`jsonb`) contains external provider evidence such as Apple and Google
place IDs for one outlet; these differ from the source's `external_id` and can
connect records from different datasets. Some providers repeat their own source
identity in this map, but deleting the map would discard independent evidence.

All Rust test code live under each crate's `tests/` directory. Modules in
`tests/unit/` are included as unit tests using `#[path]` to retain private-code
coverage; command-level integration tests remain at the directory root.

## Import and schema review (2026-10-17)

The initial loader now avoids existing-source probes and the duplicate temporary
source-key index: the empty-catalog lock establishes that all source inputs are
new. A grouped loader combines evidence with the shared rules and writes each
final canonical merchant once. Catalog primary/unique keys are rebuilt and
validated before commit, rejecting duplicates within or across batches. Initial locations use their already reconciled owner;
the row ownership resolver is suspended inside this transaction, while source-relink
and custom triggers remain active. Before publication, foreign keys and a full
source/location ownership comparison must pass, and the resolver's original mode
is restored. Normal imports keep the row resolver and manual-override behavior. The vocabulary triggers
are suspended only inside the locked initial-load transaction; distinct words are
built from final search rows once, and each trigger's original enabled mode is
restored before commit. Errors and previews roll back the data and trigger changes.
Incremental imports retain their existing duplicate detection and refresh behavior.

On the one-million-source Canada benchmark, the two removed indexes occupied
79 MiB (46 MiB search text trigrams, 33 MiB normalized-name B-tree). This conclusion
comes from tracing application queries, not from zero scan counts on an import-only
workload. Required search and identity indexes remain in place.

The remaining apparent duplication serves separate purposes:

- `merchants` is the current canonical read model; `source_records` holds the
  individual refreshable inputs. Deleting those inputs would prevent correct
  recomputation when a source changes or a merchant is relinked.
- `manual_merchants` stores reviewed inputs independently of source refreshes.
- `aliases`, `merchant_search`, and `merchant_identity_keys` are derived lookup
  structures for exact search, full-text search, and reconciliation respectively.
- Locations retain addresses and provider identities; their source references
  allow ownership to follow merchant relinks. `place_ids` is a variable provider
  map, so JSONB is appropriate here. Merchant lists are already native arrays.
- `merchant_search_words` may retain obsolete vocabulary after ordinary updates.
  It is a candidate expansion dictionary, not authoritative merchant evidence;
  search still retrieves current merchant rows.

Schema 17 addresses the repeated source attribution, licence and URL described in
the initial review: those values now share the existing provenance table. The
10,000-row Canada sample averaged 126 bytes per source for these three fields;
the replacement reference is an eight-byte integer, with shared-table and index
overhead paid per distinct metadata variant. Exact fresh-load savings are measured
in the accompanying benchmark rather than inferred solely from column widths.
Text merchant IDs also repeat across many indexes; an internal numeric key could
reduce storage but requires a broader migration while keeping public IDs stable.
The geographic and store-number location indexes deserve a query-plan review:
current location selection primarily retrieves by merchant and ranks in Rust.
No source inputs, manual corrections, locations or provider evidence were removed.

New merchant buckets also COPY directly into `merchants`, avoiding a temporary
copy and speculative upsert. Existing canonical merchants still use guarded
updates. Regular imports retain active primary keys; initial loads rebuild them
before commit. An unexpected new-ID collision aborts the transaction rather than
overwriting another merchant.
