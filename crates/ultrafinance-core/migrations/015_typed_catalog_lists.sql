-- Lists are typed data, not serialized documents. Preserve order and duplicates
-- during migration; canonicalization remains the application's responsibility.
CREATE FUNCTION pg_temp.catalog_text_array(value TEXT) RETURNS TEXT[]
LANGUAGE SQL IMMUTABLE AS $$
    SELECT ARRAY(SELECT jsonb_array_elements_text(COALESCE(value,'[]')::jsonb))
$$;
DROP INDEX merchants_markets;
ALTER TABLE merchants
    ALTER COLUMN markets_json TYPE TEXT[] USING pg_temp.catalog_text_array(markets_json),
    ALTER COLUMN aliases_json TYPE TEXT[] USING pg_temp.catalog_text_array(aliases_json),
    ALTER COLUMN sources_json TYPE TEXT[] USING pg_temp.catalog_text_array(sources_json);
ALTER TABLE manual_merchants
    ALTER COLUMN markets_json TYPE TEXT[] USING pg_temp.catalog_text_array(markets_json),
    ALTER COLUMN aliases_json TYPE TEXT[] USING pg_temp.catalog_text_array(aliases_json),
    ALTER COLUMN sources_json TYPE TEXT[] USING pg_temp.catalog_text_array(sources_json);
ALTER TABLE location_records
    ALTER COLUMN aliases_json TYPE TEXT[] USING pg_temp.catalog_text_array(aliases_json),
    ALTER COLUMN place_ids_json TYPE JSONB USING COALESCE(place_ids_json,'{}')::jsonb;
ALTER TABLE merchants RENAME COLUMN markets_json TO markets;
ALTER TABLE merchants RENAME COLUMN aliases_json TO aliases;
ALTER TABLE merchants RENAME COLUMN sources_json TO sources;
ALTER TABLE manual_merchants RENAME COLUMN markets_json TO markets;
ALTER TABLE manual_merchants RENAME COLUMN aliases_json TO aliases;
ALTER TABLE manual_merchants RENAME COLUMN sources_json TO sources;
ALTER TABLE location_records RENAME COLUMN aliases_json TO aliases;
ALTER TABLE location_records RENAME COLUMN place_ids_json TO place_ids;
ALTER TABLE merchants
    ALTER COLUMN markets SET NOT NULL, ALTER COLUMN markets SET DEFAULT '{}',
    ALTER COLUMN aliases SET NOT NULL, ALTER COLUMN aliases SET DEFAULT '{}',
    ALTER COLUMN sources SET NOT NULL, ALTER COLUMN sources SET DEFAULT '{}';
ALTER TABLE manual_merchants
    ALTER COLUMN markets SET NOT NULL, ALTER COLUMN markets SET DEFAULT '{}',
    ALTER COLUMN aliases SET NOT NULL, ALTER COLUMN aliases SET DEFAULT '{}',
    ALTER COLUMN sources SET NOT NULL, ALTER COLUMN sources SET DEFAULT '{}';
ALTER TABLE location_records
    ALTER COLUMN aliases SET NOT NULL, ALTER COLUMN aliases SET DEFAULT '{}',
    ALTER COLUMN place_ids SET NOT NULL, ALTER COLUMN place_ids SET DEFAULT '{}';
CREATE INDEX merchants_markets ON merchants USING GIN(markets);
ANALYZE merchants,manual_merchants,location_records;
UPDATE ultrafinance_schema SET version=15;
