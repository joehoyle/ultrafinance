-- Remove schema 30 cache dependencies before reconstructing an older schema.
DROP FUNCTION IF EXISTS invalidate_foursquare_import_fingerprints() CASCADE;
DROP TABLE IF EXISTS foursquare_import_fingerprints;
-- Test-only reconstruction of the text-array category schema, retaining evidence.
DO $$ BEGIN
 IF to_regclass('categories') IS NOT NULL THEN
  ALTER TABLE merchants ADD COLUMN category_ids TEXT[] NOT NULL DEFAULT '{}';
  ALTER TABLE manual_merchants ADD COLUMN category_ids TEXT[] NOT NULL DEFAULT '{}';
  ALTER TABLE source_records ADD COLUMN input_category_ids TEXT[] NOT NULL DEFAULT '{}';
  ALTER TABLE foursquare_categories ADD COLUMN category_ids TEXT[] NOT NULL DEFAULT '{}';
  UPDATE merchants SET category_ids=merchant_category_ids(id);
  UPDATE manual_merchants SET category_ids=merchant_category_ids(id,true);
  UPDATE source_records SET input_category_ids=source_category_ids(source,external_id);
  UPDATE foursquare_categories SET category_ids=foursquare_category_ids(external_id);
  DROP FUNCTION merchant_category_ids(text,boolean),source_category_ids(text,text),foursquare_category_ids(text),category_details(text[]);
  DROP TABLE merchant_categories,manual_merchant_categories,source_record_categories,foursquare_place_categories;
  DROP TABLE categories;
  DROP FUNCTION IF EXISTS advance_category_revision();
 END IF;
END $$;
-- Test-only reconstruction of schema 18, preserving data and public ownership.
DROP VIEW source_identities;
DROP TRIGGER location_merchant ON location_records;
DO $$ DECLARE t text; BEGIN
 FOR t IN SELECT unnest(ARRAY['aliases','merchant_search','merchant_identity_keys','location_records','foursquare_unlocated']) LOOP
  EXECUTE format('ALTER TABLE %I DROP CONSTRAINT %I',t,t||'_merchant_id_fkey');
  EXECUTE format('ALTER TABLE %I ALTER COLUMN merchant_id TYPE text USING merchant_public_id(merchant_id)',t);
  EXECUTE format('ALTER TABLE %I ADD CONSTRAINT %I FOREIGN KEY(merchant_id) REFERENCES merchants(id)%s',t,t||'_merchant_id_fkey',
   CASE WHEN t IN ('aliases','merchant_search','merchant_identity_keys') THEN ' ON DELETE CASCADE' ELSE '' END);
 END LOOP;
END $$;
CREATE OR REPLACE FUNCTION resolve_location_merchant() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.merchant_source IS NOT NULL THEN
  SELECT merchant_id INTO NEW.merchant_id FROM source_records WHERE source=NEW.merchant_source AND external_id=NEW.merchant_external_id;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER location_merchant BEFORE INSERT OR UPDATE OF merchant_id,merchant_source,merchant_external_id
ON location_records FOR EACH ROW EXECUTE FUNCTION resolve_location_merchant();
CREATE OR REPLACE FUNCTION relink_source_locations() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 UPDATE location_records SET merchant_id=NEW.merchant_id WHERE merchant_source=NEW.source AND merchant_external_id=NEW.external_id AND merchant_id IS DISTINCT FROM NEW.merchant_id;
 RETURN NEW;
END $$;
DROP FUNCTION merchant_keys(text[]),merchant_public_id(bigint),merchant_key(text);
ALTER TABLE merchants DROP COLUMN merchant_key;
CREATE VIEW source_identities AS
 SELECT source,external_id,merchant_id FROM source_records
 UNION ALL SELECT source,external_id,merchant_id FROM location_records WHERE source='foursquare'
 UNION ALL SELECT 'foursquare',external_id,merchant_id FROM foursquare_unlocated;
UPDATE ultrafinance_schema SET version=18;
