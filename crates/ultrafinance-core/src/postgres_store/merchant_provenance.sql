SELECT s.*,p.attribution,p.license,p.url
FROM source_records s JOIN source_provenance p ON p.id=s.source_provenance_id
WHERE s.merchant_id=ANY($1)
ORDER BY s.merchant_id,s.source,s.external_id;
