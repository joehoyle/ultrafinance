-- Preserve explicit view privileges on the tables now queried directly.
-- Supporting tables need SELECT for the former joined projections.
DO $$
DECLARE permission record; dependency record;
BEGIN
 FOR permission IN
  SELECT c.relname AS view_name,a.grantee,a.privilege_type,a.is_grantable
  FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace,
  LATERAL aclexplode(COALESCE(c.relacl,acldefault('r',c.relowner))) a
  WHERE n.nspname='public' AND c.relkind='v'
   AND c.relname IN ('merchants_documents','manual_merchants_documents',
    'source_records_documents','location_records_documents','descriptor_resolution_documents')
   AND a.grantee<>c.relowner
 LOOP
  FOR dependency IN
   SELECT * FROM (VALUES
    ('merchants_documents','merchants',false),
    ('manual_merchants_documents','manual_merchants',false),
    ('source_records_documents','source_records',false),
    ('location_records_documents','location_records',false),
    ('location_records_documents','source_provenance',true),
    ('descriptor_resolution_documents','descriptor_resolutions',false),
    ('descriptor_resolution_documents','merchants',true)
   ) AS dependencies(view_name,table_name,supporting)
   WHERE view_name=permission.view_name
    AND (NOT supporting OR permission.privilege_type='SELECT')
  LOOP
   EXECUTE format('GRANT %s ON TABLE public.%I TO %s%s',
    permission.privilege_type,dependency.table_name,
    CASE WHEN permission.grantee=0 THEN 'PUBLIC'
     ELSE quote_ident(pg_get_userbyid(permission.grantee)) END,
    CASE WHEN permission.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END);
  END LOOP;
 END LOOP;
END $$;
DROP VIEW descriptor_resolution_documents;
DROP VIEW merchants_documents,manual_merchants_documents,source_records_documents,location_records_documents;
UPDATE ultrafinance_schema SET version=14;
