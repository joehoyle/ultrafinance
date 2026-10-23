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
