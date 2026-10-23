-- Keep each provider's assignment so a category can disappear from one place
-- while remaining assigned to other places belonging to the same merchant.
-- Ownership follows source_identities; relinks and merges require no duplicate
-- ownership column or foreign key to update here.
CREATE TABLE IF NOT EXISTS foursquare_categories (
 external_id TEXT PRIMARY KEY,
 identity_external_id TEXT NOT NULL,
 category_ids TEXT[] NOT NULL
);
CREATE INDEX IF NOT EXISTS foursquare_categories_identity ON foursquare_categories(identity_external_id);
-- No canonical categories can be unambiguously distributed back to places.
-- Reimport the export to populate the assignments without inventing evidence.
DO $$ DECLARE g record; who text; BEGIN
 FOR g IN SELECT grantee,privilege_type,is_grantable FROM information_schema.role_table_grants
 WHERE table_schema=current_schema() AND table_name='source_records'
 AND privilege_type IN ('SELECT','INSERT','UPDATE','DELETE') LOOP
  who := CASE WHEN g.grantee='PUBLIC' THEN 'PUBLIC' ELSE quote_ident(g.grantee) END;
  EXECUTE 'GRANT '||g.privilege_type||' ON foursquare_categories TO '||who||CASE WHEN g.is_grantable='YES' THEN ' WITH GRANT OPTION' ELSE '' END;
 END LOOP;
END $$;
UPDATE ultrafinance_schema SET version=27;
