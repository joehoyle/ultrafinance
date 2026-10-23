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
