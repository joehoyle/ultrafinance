-- Version 9 repairs databases whose version-8 views still emitted the retired
-- market_evidence field. Keep merchant coverage, identities and source inputs.
-- Preserve explicit view privileges when recreating the document projections.
CREATE TEMP TABLE document_view_grants ON COMMIT DROP AS
SELECT c.relname AS view_name, a.grantee, a.privilege_type, a.is_grantable
FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace,
LATERAL aclexplode(COALESCE(c.relacl,acldefault('r',c.relowner))) a
WHERE n.nspname='public'
 AND c.relname IN ('merchants_documents','manual_merchants_documents','source_records_documents','descriptor_resolution_documents')
 AND a.grantee<>c.relowner;
DROP VIEW descriptor_resolution_documents;
DROP VIEW merchants_documents, manual_merchants_documents, source_records_documents;
ALTER TABLE merchants DROP COLUMN IF EXISTS market_evidence_json;
ALTER TABLE manual_merchants DROP COLUMN IF EXISTS market_evidence_json;
DROP TABLE IF EXISTS source_market_inputs, merchant_market_evidence;
CREATE VIEW merchants_documents AS SELECT *, (jsonb_build_object('id',id,'name',name,'website',website,'logo_url',logo_url,'logo_source',logo_source,'markets',markets_json::jsonb,'aliases',aliases_json::jsonb,'sources',sources_json::jsonb))::text AS data FROM merchants;
CREATE VIEW manual_merchants_documents AS SELECT *, (jsonb_build_object('id',id,'name',name,'website',website,'logo_url',logo_url,'logo_source',logo_source,'markets',markets_json::jsonb,'aliases',aliases_json::jsonb,'sources',sources_json::jsonb))::text AS data FROM manual_merchants;
CREATE VIEW source_records_documents AS SELECT s.*, jsonb_build_object(
 'source',source,'external_id',external_id,
 'merchant',jsonb_build_object(
  'id',input_id,'name',input_name,'website',input_website,
  'logo_url',input_logo_url,'logo_source',input_logo_source,
  'markets',input_markets,'aliases',input_aliases,'sources',input_sources
 ),
 'raw',jsonb_strip_nulls(jsonb_build_object(
  'countryHints',CASE WHEN cardinality(country_hints)>0 THEN country_hints END,
  'negativeAliases',CASE WHEN cardinality(negative_aliases)>0 THEN negative_aliases END,
  'transaction_text_regexp',transaction_pattern,'parent_id',parent_id
 )),
 'version',version,'attribution',attribution,'license',license,'url',url
)::text AS data FROM source_records s;
CREATE VIEW descriptor_resolution_documents AS SELECT r.id, (jsonb_build_object('id',r.id,'context',jsonb_build_object('description',r.description,'country',r.country,'amount',r.amount,'currency',r.currency,'location',CASE WHEN r.location_present THEN jsonb_build_object('address',r.location_address,'city',r.location_city,'region',r.location_region,'postal_code',r.location_postal_code,'country',r.location_country,'store_number',r.location_store_number) ELSE NULL END,'extra',r.extra_json::jsonb),'merchant',m.data::jsonb,'provenance',r.provenance_json::jsonb,'verified',r.verified,'evidence',r.review_evidence))::text AS data FROM descriptor_resolutions r JOIN merchants_documents m ON m.id=r.merchant_id;
DO $$
DECLARE permission RECORD;
BEGIN
 FOR permission IN SELECT * FROM document_view_grants LOOP
  EXECUTE format('GRANT %s ON TABLE public.%I TO %s%s',
   permission.privilege_type, permission.view_name,
   CASE WHEN permission.grantee=0 THEN 'PUBLIC'
    ELSE quote_ident(pg_get_userbyid(permission.grantee)) END,
   CASE WHEN permission.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END);
 END LOOP;
END $$;
UPDATE ultrafinance_schema SET version=9;
