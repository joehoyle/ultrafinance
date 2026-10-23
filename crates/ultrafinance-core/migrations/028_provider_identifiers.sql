-- (source,external_id) is the authoritative provider identifier.
ALTER TABLE location_records DROP COLUMN IF EXISTS place_ids;
-- Store a legacy group reference only when it differs from the place key.
ALTER TABLE foursquare_categories ALTER COLUMN identity_external_id DROP NOT NULL;
UPDATE foursquare_categories SET identity_external_id=NULL WHERE identity_external_id=external_id;
DROP INDEX IF EXISTS foursquare_categories_identity;
CREATE INDEX foursquare_categories_identity ON foursquare_categories(identity_external_id) WHERE identity_external_id IS NOT NULL;
-- Distinct original object IDs are retained; equal IDs are derived on reads.
ALTER TABLE source_records ALTER COLUMN input_id DROP NOT NULL;
UPDATE source_records SET input_id=NULL WHERE input_id=external_id;
UPDATE ultrafinance_schema SET version=28;
UPDATE catalog_revision SET revision=revision+1;
