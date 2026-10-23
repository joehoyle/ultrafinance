-- Derived import cache only: deleting it is safe and forces reconciliation.
CREATE TABLE foursquare_import_fingerprints (
 external_id TEXT PRIMARY KEY,
 merchant_id BIGINT NOT NULL REFERENCES merchants(merchant_key) ON DELETE CASCADE,
 fingerprint BYTEA NOT NULL CHECK(octet_length(fingerprint)=32)
);
CREATE INDEX foursquare_import_fingerprints_merchant ON foursquare_import_fingerprints(merchant_id);

-- Invalidate once per changed statement, not once per place. This also handles
-- manual links, merges and writes outside the importer, including old/new owners.
CREATE FUNCTION invalidate_foursquare_import_fingerprints() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE changed TEXT; ids TEXT; has_changes BOOLEAN;
BEGIN
 IF TG_OP='TRUNCATE' THEN
  TRUNCATE foursquare_import_fingerprints;
  RETURN NULL;
 END IF;
 changed := CASE TG_OP WHEN 'UPDATE' THEN 'SELECT * FROM old_rows UNION ALL SELECT * FROM new_rows'
                      WHEN 'DELETE' THEN 'SELECT * FROM old_rows' ELSE 'SELECT * FROM new_rows' END;
 EXECUTE 'SELECT EXISTS(SELECT 1 FROM ('||changed||') c)' INTO has_changes;
 IF NOT has_changes THEN RETURN NULL; END IF;
 IF TG_ARGV[0]='owner' THEN
  ids := format('SELECT DISTINCT %s AS id FROM (%s) c',TG_ARGV[1],changed);
  EXECUTE 'DELETE FROM foursquare_import_fingerprints WHERE external_id=ANY(ARRAY(SELECT f.external_id FROM ('||ids||') owners CROSS JOIN LATERAL (SELECT external_id FROM foursquare_import_fingerprints WHERE merchant_id=owners.id OFFSET 0) f))';
 ELSE
  ids := format('SELECT c.external_id AS id FROM (%s) c',changed);
  IF TG_TABLE_NAME='foursquare_categories' THEN
   ids := ids||format(' UNION SELECT c.identity_external_id FROM (%s) c',changed);
  ELSE
   ids := ids||format(' UNION SELECT f.identity_external_id FROM foursquare_categories f JOIN (%s) c ON f.external_id=c.external_id',changed);
  END IF;
  EXECUTE 'DELETE FROM foursquare_import_fingerprints WHERE external_id=ANY(ARRAY('||ids||'))';
 END IF;
 RETURN NULL;
END $$;
DO $$ DECLARE spec RECORD; BEGIN
 FOR spec IN SELECT * FROM (VALUES
  ('merchants','owner','c.merchant_key'),
  ('manual_merchants','owner','merchant_key(c.id)'),
  ('source_records','owner','merchant_key(c.merchant_id)'),
  ('location_records','owner','c.merchant_id'),
  ('foursquare_unlocated','owner','c.merchant_id'),
  ('merchant_categories','owner','c.merchant_id'),
  ('manual_merchant_categories','owner','merchant_key(c.merchant_id)'),
  ('foursquare_categories','place',''),
  ('foursquare_place_categories','place','')
 ) v(table_name,kind,expression) LOOP
  EXECUTE format('CREATE TRIGGER fsq_import_cache_insert AFTER INSERT ON %I REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION invalidate_foursquare_import_fingerprints(%L,%L)',spec.table_name,spec.kind,spec.expression);
  EXECUTE format('CREATE TRIGGER fsq_import_cache_update AFTER UPDATE ON %I REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION invalidate_foursquare_import_fingerprints(%L,%L)',spec.table_name,spec.kind,spec.expression);
  EXECUTE format('CREATE TRIGGER fsq_import_cache_delete AFTER DELETE ON %I REFERENCING OLD TABLE AS old_rows FOR EACH STATEMENT EXECUTE FUNCTION invalidate_foursquare_import_fingerprints(%L,%L)',spec.table_name,spec.kind,spec.expression);
  EXECUTE format('CREATE TRIGGER fsq_import_cache_truncate AFTER TRUNCATE ON %I FOR EACH STATEMENT EXECUTE FUNCTION invalidate_foursquare_import_fingerprints()',spec.table_name);
 END LOOP;
END $$;
-- Runtime roles receive the same access as existing provider inputs.
DO $$ DECLARE g RECORD; who TEXT; BEGIN
 FOR g IN SELECT grantee,privilege_type,is_grantable FROM information_schema.role_table_grants
 WHERE table_schema=current_schema() AND table_name='source_records'
 AND privilege_type IN ('SELECT','INSERT','UPDATE','DELETE') LOOP
  who := CASE WHEN g.grantee='PUBLIC' THEN 'PUBLIC' ELSE quote_ident(g.grantee) END;
  EXECUTE 'GRANT '||g.privilege_type||' ON foursquare_import_fingerprints TO '||who||CASE WHEN g.is_grantable='YES' THEN ' WITH GRANT OPTION' ELSE '' END;
 END LOOP;
END $$;
UPDATE ultrafinance_schema SET version=30;
