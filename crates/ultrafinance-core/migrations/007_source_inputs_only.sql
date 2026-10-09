-- Retain source identities and matching inputs, not downloadable source blobs.
DROP VIEW source_records_documents;
ALTER TABLE source_records ADD COLUMN negative_aliases TEXT[] NOT NULL DEFAULT '{}';
UPDATE source_records SET negative_aliases=ARRAY(
 SELECT v #>> '{}' FROM jsonb_array_elements(
  CASE WHEN jsonb_typeof(raw_json::jsonb->'negativeAliases')='array'
   THEN raw_json::jsonb->'negativeAliases' ELSE '[]'::jsonb END
 ) v WHERE jsonb_typeof(v)='string'
) WHERE strpos(raw_json,'"negativeAliases"')>0;
ALTER TABLE source_records DROP COLUMN merchant_json, DROP COLUMN raw_json;
-- Historical merge audits may also contain copied upstream payloads. Keep
-- their identity history and matching fields without the downloadable blobs.
UPDATE merchant_merge_runs SET data=jsonb_set(data::jsonb,'{before,sources}',
 COALESCE((SELECT jsonb_agg(jsonb_set(item,'{1,raw}',jsonb_strip_nulls(jsonb_build_object(
  'countryHints',item#>'{1,raw,countryHints}',
  'negativeAliases',item#>'{1,raw,negativeAliases}',
  'transaction_text_regexp',item#>'{1,raw,transaction_text_regexp}',
  'parent_id',item#>'{1,raw,parent_id}'
 )))) FROM jsonb_array_elements(data::jsonb#>'{before,sources}') item),'[]'::jsonb)
)::text WHERE jsonb_typeof(data::jsonb#>'{before,sources}')='array';
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
UPDATE ultrafinance_schema SET version=6;
