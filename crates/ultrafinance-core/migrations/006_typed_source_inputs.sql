-- Schema version 5: typed source identity/country inputs for import rebuilds.
-- Original merchant and raw documents remain intact for provenance and delta checks.
ALTER TABLE source_records
 ADD COLUMN input_id TEXT,
 ADD COLUMN input_name TEXT,
 ADD COLUMN input_website TEXT,
 ADD COLUMN input_logo_url TEXT,
 ADD COLUMN input_logo_source TEXT,
 ADD COLUMN input_markets TEXT[] NOT NULL DEFAULT '{}',
 ADD COLUMN input_aliases TEXT[] NOT NULL DEFAULT '{}',
 ADD COLUMN input_sources TEXT[] NOT NULL DEFAULT '{}',
 ADD COLUMN country_hints TEXT[] NOT NULL DEFAULT '{}',
 ADD COLUMN dataset_region TEXT;
UPDATE source_records SET
 input_id=merchant_json::jsonb->>'id',input_name=merchant_json::jsonb->>'name',
 input_website=merchant_json::jsonb->>'website',input_logo_url=merchant_json::jsonb->>'logo_url',input_logo_source=merchant_json::jsonb->>'logo_source',
 input_markets=ARRAY(SELECT jsonb_array_elements_text(COALESCE(merchant_json::jsonb->'markets','[]'))),
 input_aliases=ARRAY(SELECT jsonb_array_elements_text(COALESCE(merchant_json::jsonb->'aliases','[]'))),
 input_sources=ARRAY(SELECT jsonb_array_elements_text(COALESCE(merchant_json::jsonb->'sources','[]'))),
 country_hints=ARRAY(SELECT h #>> '{}' FROM jsonb_array_elements(CASE WHEN jsonb_typeof(raw_json::jsonb->'countryHints')='array' THEN raw_json::jsonb->'countryHints' ELSE '[]'::jsonb END) h WHERE jsonb_typeof(h)='string'),
 dataset_region=CASE WHEN source='open-enrichment' AND version IS NOT NULL THEN NULLIF(CASE WHEN version LIKE 'fnv1a64:%' THEN split_part(version,':',3) WHEN strpos(version,':')>0 THEN split_part(version,':',1) END,'') END;
ALTER TABLE source_records ALTER COLUMN input_id SET NOT NULL;
ALTER TABLE source_records ALTER COLUMN input_name SET NOT NULL;
UPDATE ultrafinance_schema SET version=5;
