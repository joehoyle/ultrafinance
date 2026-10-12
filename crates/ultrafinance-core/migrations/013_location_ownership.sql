-- Every outlet has an indexed canonical merchant and shared source metadata.
LOCK TABLE source_records, location_records IN SHARE ROW EXCLUSIVE MODE;
-- Preserve explicit runtime grants when replacing the projection.
CREATE TEMP TABLE location_view_grants ON COMMIT DROP AS
SELECT grantee, privilege_type, is_grantable FROM information_schema.role_table_grants
WHERE table_schema=current_schema() AND table_name='location_records_documents';
DROP VIEW location_records_documents;
ALTER TABLE location_records DROP CONSTRAINT location_records_check;
-- Bulk validation avoids a random merchant heap lookup for every backfilled outlet.
-- Restore and validate the same constraint before this transaction can commit.
ALTER TABLE location_records DROP CONSTRAINT location_records_merchant_id_fkey;

-- Prepare shared metadata first so source-backed outlets are rewritten only once.
CREATE TABLE source_provenance (
 id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
 source TEXT NOT NULL,
 attribution TEXT NOT NULL,
 license TEXT NOT NULL,
 url TEXT NOT NULL,
 UNIQUE(source,attribution,license,url),
 UNIQUE(source,id)
);
INSERT INTO source_provenance(source,attribution,license,url)
SELECT DISTINCT source,attribution,license,url FROM location_records;
ANALYZE source_provenance;
ALTER TABLE location_records ADD COLUMN source_provenance_id BIGINT;
UPDATE location_records l SET merchant_id=s.merchant_id,source_provenance_id=p.id
FROM source_records s,source_provenance p
WHERE l.merchant_source=s.source AND l.merchant_external_id=s.external_id
 AND (l.source,l.attribution,l.license,l.url)=(p.source,p.attribution,p.license,p.url);
UPDATE location_records l SET source_provenance_id=p.id FROM source_provenance p
WHERE l.merchant_source IS NULL
 AND (l.source,l.attribution,l.license,l.url)=(p.source,p.attribution,p.license,p.url);
ALTER TABLE location_records ALTER COLUMN merchant_id SET NOT NULL;
ALTER TABLE location_records ADD CONSTRAINT location_records_source_reference CHECK
 ((merchant_source IS NULL AND merchant_external_id IS NULL)
  OR (merchant_source IS NOT NULL AND merchant_external_id IS NOT NULL));
ALTER TABLE location_records ADD CONSTRAINT location_records_merchant_id_fkey
 FOREIGN KEY(merchant_id) REFERENCES merchants(id) NOT VALID;
ALTER TABLE location_records VALIDATE CONSTRAINT location_records_merchant_id_fkey;

CREATE FUNCTION resolve_location_merchant() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.merchant_source IS NOT NULL THEN
  SELECT merchant_id INTO NEW.merchant_id FROM source_records
  WHERE source=NEW.merchant_source AND external_id=NEW.merchant_external_id;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER location_merchant BEFORE INSERT OR UPDATE OF merchant_id,merchant_source,merchant_external_id
ON location_records FOR EACH ROW EXECUTE FUNCTION resolve_location_merchant();
CREATE FUNCTION relink_source_locations() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 UPDATE location_records SET merchant_id=NEW.merchant_id
 WHERE merchant_source=NEW.source AND merchant_external_id=NEW.external_id
 AND merchant_id IS DISTINCT FROM NEW.merchant_id;
 RETURN NEW;
END $$;
CREATE TRIGGER source_location_merchant AFTER UPDATE OF merchant_id ON source_records
FOR EACH ROW WHEN (OLD.merchant_id IS DISTINCT FROM NEW.merchant_id)
EXECUTE FUNCTION relink_source_locations();

-- A source can publish different dataset URLs/licences. Preserve each variant.
ALTER TABLE location_records ALTER COLUMN source_provenance_id SET NOT NULL;
ALTER TABLE location_records ADD CONSTRAINT location_records_provenance_fk
 FOREIGN KEY(source,source_provenance_id) REFERENCES source_provenance(source,id) NOT VALID;
ALTER TABLE location_records VALIDATE CONSTRAINT location_records_provenance_fk;
ALTER TABLE location_records DROP COLUMN attribution, DROP COLUMN license, DROP COLUMN url;
CREATE VIEW location_records_documents AS SELECT l.*, (jsonb_build_object('source',l.source,'external_id',external_id,'merchant',CASE WHEN merchant_source IS NOT NULL THEN jsonb_build_object('source',merchant_source,'external_id',merchant_external_id) ELSE jsonb_build_object('merchant_id',merchant_id) END,'location',jsonb_build_object('id',l.id,'name',name,'precision',precision,'address',address,'city',city,'region',region,'postal_code',postal_code,'country',country,'store_number',store_number,'latitude',latitude,'longitude',longitude),'aliases',aliases_json::jsonb,'place_ids',place_ids_json::jsonb,'transaction_pattern',transaction_pattern,'manual_override',manual_override,'attribution',p.attribution,'license',p.license,'url',p.url))::text AS data FROM location_records l JOIN source_provenance p ON p.id=l.source_provenance_id;
DO $$ DECLARE g record; BEGIN
 FOR g IN SELECT * FROM location_view_grants LOOP
  EXECUTE format('GRANT %s ON location_records_documents TO %s%s',g.privilege_type,
    CASE WHEN g.grantee='PUBLIC' THEN 'PUBLIC' ELSE quote_ident(g.grantee) END,
    CASE WHEN g.is_grantable='YES' THEN ' WITH GRANT OPTION' ELSE '' END);
 END LOOP;
END $$;
CREATE TRIGGER source_provenance_revision AFTER UPDATE ON source_provenance
FOR EACH ROW WHEN ((OLD.source,OLD.attribution,OLD.license,OLD.url)
 IS DISTINCT FROM (NEW.source,NEW.attribution,NEW.license,NEW.url))
EXECUTE FUNCTION advance_catalog_revision();
UPDATE ultrafinance_schema SET version=13;
