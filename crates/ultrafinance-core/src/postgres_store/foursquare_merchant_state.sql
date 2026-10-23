SELECT m.*,o.external_ids,
 ARRAY(SELECT c.external_id FROM merchant_categories a
       JOIN categories c ON c.id=a.category_id
       WHERE a.merchant_id=m.merchant_key ORDER BY c.external_id) AS category_ids,
 EXISTS(SELECT 1 FROM manual_merchants WHERE id=m.id) AS is_manual,
 (SELECT count(*)=1 FROM (
  -- The outer LIMIT short-circuits once a second identity is found. Numeric
  -- owner indexes avoid joining merchants again through source_identities.
  SELECT 1 FROM source_records WHERE merchant_id=m.id
  UNION ALL
  SELECT 1 FROM location_records WHERE merchant_id=m.merchant_key AND source='foursquare'
  UNION ALL
  SELECT 1 FROM foursquare_unlocated WHERE merchant_id=m.merchant_key
  LIMIT 2
 ) refs) AS replaceable
FROM owners o
CROSS JOIN LATERAL (
 SELECT * FROM merchants WHERE merchant_key=o.merchant_key OFFSET 0
) m
