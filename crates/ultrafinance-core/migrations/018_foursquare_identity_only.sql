-- Foursquare keeps current outlets and import identity, not replayable source inputs.
LOCK TABLE merchants,source_records,location_records IN ACCESS EXCLUSIVE MODE;
DO $$ BEGIN
 IF EXISTS(SELECT 1 FROM source_records s JOIN location_records l
  ON (l.source,l.external_id)=(s.source,s.external_id)
  WHERE s.source='foursquare' AND s.merchant_id<>l.merchant_id) THEN
  RAISE EXCEPTION 'conflicting Foursquare source/outlet ownership; resolve before migrating';
 END IF;
END $$;
CREATE TABLE foursquare_unlocated (
 external_id TEXT PRIMARY KEY,
 merchant_id TEXT NOT NULL REFERENCES merchants(id)
);
CREATE INDEX foursquare_unlocated_merchant ON foursquare_unlocated(merchant_id);
INSERT INTO foursquare_unlocated
SELECT s.external_id,s.merchant_id FROM source_records s
WHERE s.source='foursquare' AND NOT EXISTS (
 SELECT 1 FROM location_records l WHERE l.source=s.source AND l.external_id=s.external_id
);
-- Keep canonical ownership; stop depending on source evidence rows.
UPDATE location_records SET merchant_source=NULL,merchant_external_id=NULL
WHERE merchant_source='foursquare';
DELETE FROM source_records WHERE source='foursquare';
ALTER TABLE source_records ADD CONSTRAINT source_records_no_foursquare CHECK(source<>'foursquare');
CREATE VIEW source_identities AS
 SELECT source,external_id,merchant_id FROM source_records
 UNION ALL SELECT source,external_id,merchant_id FROM location_records WHERE source='foursquare'
 UNION ALL SELECT 'foursquare',external_id,merchant_id FROM foursquare_unlocated;
CREATE TRIGGER foursquare_unlocated_revision AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE
ON foursquare_unlocated FOR EACH STATEMENT EXECUTE FUNCTION advance_catalog_revision();
-- The new relations need the same runtime access as source records.
DO $$ DECLARE g record; who text; BEGIN
 FOR g IN SELECT grantee,privilege_type,is_grantable FROM information_schema.role_table_grants
 WHERE table_schema=current_schema() AND table_name='source_records'
 AND privilege_type IN ('SELECT','INSERT','UPDATE','DELETE') LOOP
  who := CASE WHEN g.grantee='PUBLIC' THEN 'PUBLIC' ELSE quote_ident(g.grantee) END;
  EXECUTE 'GRANT '||g.privilege_type||' ON foursquare_unlocated TO '||who||CASE WHEN g.is_grantable='YES' THEN ' WITH GRANT OPTION' ELSE '' END;
  EXECUTE 'GRANT SELECT ON source_identities TO '||who;
 END LOOP;
END $$;
UPDATE ultrafinance_schema SET version=18;
