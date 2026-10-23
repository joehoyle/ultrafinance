WITH matched AS MATERIALIZED (
 SELECT i.external_id,s.merchant_key
 FROM unnest($1::text[]) i(external_id)
 CROSS JOIN LATERAL (
  -- Probe physical identity indexes directly, without converting numeric
  -- ownership to a public ID separately for every incoming place.
  SELECT m.merchant_key FROM source_records s
  JOIN merchants m ON m.id=s.merchant_id
  WHERE s.source='foursquare' AND s.external_id=i.external_id
  UNION ALL
  SELECT merchant_id FROM location_records
  WHERE source='foursquare' AND external_id=i.external_id
  UNION ALL
  SELECT merchant_id FROM foursquare_unlocated WHERE external_id=i.external_id
  OFFSET 0
 ) s
), owners AS (
 SELECT merchant_key,array_agg(external_id) AS external_ids
 FROM matched GROUP BY merchant_key
)
