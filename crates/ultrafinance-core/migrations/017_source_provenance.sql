-- Share immutable attribution variants with locations. Keep source identity,
-- version and all matching inputs on their original rows.
LOCK TABLE source_records,source_provenance IN SHARE ROW EXCLUSIVE MODE;
INSERT INTO source_provenance(source,attribution,license,url)
SELECT DISTINCT source,attribution,license,url FROM source_records
ON CONFLICT(source,attribution,license,url) DO NOTHING;
ALTER TABLE source_records ADD COLUMN source_provenance_id BIGINT;
UPDATE source_records s SET source_provenance_id=p.id FROM source_provenance p
WHERE (s.source,s.attribution,s.license,s.url)=(p.source,p.attribution,p.license,p.url);
ALTER TABLE source_records ALTER COLUMN source_provenance_id SET NOT NULL;
ALTER TABLE source_records ADD CONSTRAINT source_records_provenance_fk
 FOREIGN KEY(source,source_provenance_id) REFERENCES source_provenance(source,id);
ALTER TABLE source_records DROP COLUMN attribution,DROP COLUMN license,DROP COLUMN url;
ANALYZE source_records,source_provenance;
UPDATE ultrafinance_schema SET version=17;
-- Existing source readers/writers also need the shared metadata relation.
-- Preserve grantability when the previous source privilege was delegable.
DO $$ DECLARE g record; who text; delegation text; BEGIN
 FOR g IN SELECT grantee,privilege_type,is_grantable FROM information_schema.role_table_grants
          WHERE table_schema=current_schema() AND table_name='source_records'
            AND privilege_type IN ('SELECT','INSERT','UPDATE') LOOP
  who := CASE WHEN g.grantee='PUBLIC' THEN 'PUBLIC' ELSE quote_ident(g.grantee) END;
  delegation := CASE WHEN g.is_grantable='YES' THEN ' WITH GRANT OPTION' ELSE '' END;
  EXECUTE 'GRANT SELECT ON source_provenance TO ' || who || delegation;
  IF g.privilege_type IN ('INSERT','UPDATE') THEN
   EXECUTE 'GRANT INSERT,UPDATE ON source_provenance TO ' || who || delegation;
   EXECUTE format('GRANT USAGE ON SEQUENCE %s TO %s%s',pg_get_serial_sequence('source_provenance','id'),who,delegation);
  END IF;
 END LOOP;
END $$;
