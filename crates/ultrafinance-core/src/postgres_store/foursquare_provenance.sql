-- Resolve public IDs once, then use the numeric owner indexes directly. Reading
-- source_identities and joining locations again repeats merchant/key lookups for
-- every outlet of every candidate in a large chain.
WITH owners AS MATERIALIZED (
    SELECT id,merchant_key FROM merchants WHERE id=ANY($1)
), refs AS (
    SELECT m.id AS merchant_id,l.external_id,p.attribution,p.license,p.url
    FROM owners m CROSS JOIN LATERAL (
        SELECT external_id,source_provenance_id FROM location_records
        WHERE merchant_id=m.merchant_key AND source='foursquare' OFFSET 0
    ) l LEFT JOIN source_provenance p ON p.id=l.source_provenance_id
    UNION ALL
    SELECT m.id,u.external_id,NULL,NULL,NULL
    FROM owners m CROSS JOIN LATERAL (
        SELECT external_id FROM foursquare_unlocated WHERE merchant_id=m.merchant_key OFFSET 0
    ) u
    UNION ALL
    SELECT m.id,s.external_id,p.attribution,p.license,p.url
    FROM owners m CROSS JOIN LATERAL (
        SELECT external_id FROM source_records WHERE merchant_id=m.id AND source='foursquare' OFFSET 0
    ) s
    LEFT JOIN location_records l ON l.source='foursquare' AND l.external_id=s.external_id
    LEFT JOIN source_provenance p ON p.id=l.source_provenance_id
)
SELECT merchant_id,min(external_id) AS external_id,count(*) AS n,
    COALESCE(attribution,$2) AS attribution,COALESCE(license,$3) AS license,COALESCE(url,$4) AS url
FROM refs GROUP BY merchant_id,attribution,license,url;
