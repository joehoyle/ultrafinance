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
CREATE VIEW location_records_documents AS SELECT l.*, (jsonb_build_object('source',l.source,'external_id',external_id,'merchant',CASE WHEN merchant_source IS NOT NULL THEN jsonb_build_object('source',merchant_source,'external_id',merchant_external_id) ELSE jsonb_build_object('merchant_id',merchant_id) END,'location',jsonb_build_object('id',l.id,'name',name,'precision',precision,'address',address,'city',city,'region',region,'postal_code',postal_code,'country',country,'store_number',store_number,'latitude',latitude,'longitude',longitude),'aliases',aliases_json::jsonb,'place_ids',place_ids_json::jsonb,'transaction_pattern',transaction_pattern,'manual_override',manual_override,'attribution',p.attribution,'license',p.license,'url',p.url))::text AS data FROM location_records l JOIN source_provenance p ON p.id=l.source_provenance_id;
UPDATE ultrafinance_schema SET version=13;
DROP VIEW location_records_documents;
ALTER TABLE location_records ADD COLUMN attribution TEXT,ADD COLUMN license TEXT,ADD COLUMN url TEXT;
UPDATE location_records l SET attribution=p.attribution,license=p.license,url=p.url FROM source_provenance p WHERE p.id=l.source_provenance_id;
DROP TRIGGER location_merchant ON location_records;
DROP TRIGGER source_location_merchant ON source_records;
DROP FUNCTION resolve_location_merchant();
DROP FUNCTION relink_source_locations();
ALTER TABLE location_records DROP CONSTRAINT location_records_source_reference;
ALTER TABLE location_records ALTER COLUMN merchant_id DROP NOT NULL;
UPDATE location_records SET merchant_id=NULL WHERE merchant_source IS NOT NULL;
ALTER TABLE location_records ADD CONSTRAINT location_records_check CHECK
 ((merchant_id IS NOT NULL AND merchant_source IS NULL AND merchant_external_id IS NULL)
 OR (merchant_id IS NULL AND merchant_source IS NOT NULL AND merchant_external_id IS NOT NULL));
ALTER TABLE location_records DROP COLUMN source_provenance_id;
DROP TABLE source_provenance;
CREATE VIEW location_records_documents AS SELECT *, (jsonb_build_object('source',source,'external_id',external_id,'merchant',CASE WHEN merchant_id IS NOT NULL THEN jsonb_build_object('merchant_id',merchant_id) ELSE jsonb_build_object('source',merchant_source,'external_id',merchant_external_id) END,'location',jsonb_build_object('id',id,'name',name,'precision',precision,'address',address,'city',city,'region',region,'postal_code',postal_code,'country',country,'store_number',store_number,'latitude',latitude,'longitude',longitude),'aliases',aliases_json::jsonb,'place_ids',place_ids_json::jsonb,'transaction_pattern',transaction_pattern,'manual_override',manual_override,'attribution',attribution,'license',license,'url',url))::text AS data FROM location_records;
UPDATE ultrafinance_schema SET version=12;
