-- Historic catalogs did not have the additive pagination subsystem.
-- Reconstruct the pre-28 provider map for historic migration fixtures.
ALTER TABLE location_records ADD COLUMN IF NOT EXISTS place_ids JSONB NOT NULL DEFAULT '{}';
UPDATE source_records SET input_id=external_id WHERE input_id IS NULL;
-- Remove version-18 additions before constructing a historical schema.
DROP VIEW source_identities;
DROP TABLE foursquare_unlocated;
ALTER TABLE source_records DROP CONSTRAINT source_records_no_foursquare;
-- Test-only reconstruction of the pre-normalization source columns.
ALTER TABLE source_records ADD COLUMN attribution TEXT,ADD COLUMN license TEXT,ADD COLUMN url TEXT;
UPDATE source_records s SET attribution=p.attribution,license=p.license,url=p.url
FROM source_provenance p WHERE p.id=s.source_provenance_id;
ALTER TABLE source_records ALTER COLUMN attribution SET NOT NULL,ALTER COLUMN license SET NOT NULL,ALTER COLUMN url SET NOT NULL;
ALTER TABLE source_records DROP COLUMN source_provenance_id;
UPDATE ultrafinance_schema SET version=16;
-- Test-only downgrade of schema 15 to exercise real historic upgrade paths.
DROP INDEX merchants_markets;
ALTER TABLE merchants
    ALTER COLUMN markets DROP DEFAULT, ALTER COLUMN markets DROP NOT NULL,
    ALTER COLUMN aliases DROP DEFAULT, ALTER COLUMN aliases DROP NOT NULL,
    ALTER COLUMN sources DROP DEFAULT, ALTER COLUMN sources DROP NOT NULL;
ALTER TABLE manual_merchants
    ALTER COLUMN markets DROP DEFAULT, ALTER COLUMN markets DROP NOT NULL,
    ALTER COLUMN aliases DROP DEFAULT, ALTER COLUMN aliases DROP NOT NULL,
    ALTER COLUMN sources DROP DEFAULT, ALTER COLUMN sources DROP NOT NULL;
ALTER TABLE location_records
    ALTER COLUMN aliases DROP DEFAULT, ALTER COLUMN aliases DROP NOT NULL,
    ALTER COLUMN place_ids DROP DEFAULT, ALTER COLUMN place_ids DROP NOT NULL;
ALTER TABLE merchants
    ALTER COLUMN markets TYPE TEXT USING to_json(markets)::text,
    ALTER COLUMN aliases TYPE TEXT USING to_json(aliases)::text,
    ALTER COLUMN sources TYPE TEXT USING to_json(sources)::text;
ALTER TABLE manual_merchants
    ALTER COLUMN markets TYPE TEXT USING to_json(markets)::text,
    ALTER COLUMN aliases TYPE TEXT USING to_json(aliases)::text,
    ALTER COLUMN sources TYPE TEXT USING to_json(sources)::text;
ALTER TABLE location_records
    ALTER COLUMN aliases TYPE TEXT USING to_json(aliases)::text,
    ALTER COLUMN place_ids TYPE TEXT USING place_ids::text;
ALTER TABLE merchants RENAME COLUMN markets TO markets_json;
ALTER TABLE merchants RENAME COLUMN aliases TO aliases_json;
ALTER TABLE merchants RENAME COLUMN sources TO sources_json;
ALTER TABLE manual_merchants RENAME COLUMN markets TO markets_json;
ALTER TABLE manual_merchants RENAME COLUMN aliases TO aliases_json;
ALTER TABLE manual_merchants RENAME COLUMN sources TO sources_json;
ALTER TABLE location_records RENAME COLUMN aliases TO aliases_json;
ALTER TABLE location_records RENAME COLUMN place_ids TO place_ids_json;
CREATE INDEX merchants_markets ON merchants USING GIN((markets_json::jsonb));
UPDATE ultrafinance_schema SET version=14;
