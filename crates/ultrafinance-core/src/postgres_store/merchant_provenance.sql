-- Aggregate a chain's matching inputs before hydration. Upstream identities
-- remain separately stored for refresh and relinking; no document serialization.
WITH selected AS (
    SELECT * FROM source_records WHERE merchant_id = ANY($1)
), foursquare AS (
    SELECT merchant_id,
        COALESCE(min(external_id) FILTER (WHERE external_id LIKE 'brand:%'), min(external_id)) AS external_id,
        attribution, license, url, transaction_pattern, parent_id,
        count(*) AS source_record_count,
        min(input_website) AS website,
        jsonb_path_query_array(jsonb_agg(DISTINCT input_markets), '$[*][*]') AS markets,
        jsonb_path_query_array(jsonb_agg(DISTINCT input_aliases), '$[*][*]') AS aliases,
        jsonb_path_query_array(jsonb_agg(DISTINCT input_sources), '$[*][*]') AS sources,
        jsonb_path_query_array(jsonb_agg(DISTINCT country_hints), '$[*][*]') AS hints,
        jsonb_path_query_array(jsonb_agg(DISTINCT negative_aliases), '$[*][*]') AS negatives
    FROM selected WHERE source = 'foursquare'
    GROUP BY merchant_id, attribution, license, url, transaction_pattern, parent_id
), contributions AS (
    SELECT merchant_id,source,external_id,input_id,input_name,input_website,
        input_logo_url,input_logo_source,input_markets,input_aliases,input_sources,
        country_hints,negative_aliases,transaction_pattern,parent_id,version,
        attribution,license,url,NULL::bigint AS source_record_count
    FROM selected WHERE source <> 'foursquare'
    UNION ALL
    SELECT f.merchant_id,'foursquare',f.external_id,f.merchant_id,m.name,f.website,
        NULL,NULL,
        ARRAY(SELECT jsonb_array_elements_text(f.markets)),
        ARRAY(SELECT jsonb_array_elements_text(f.aliases)),
        ARRAY(SELECT jsonb_array_elements_text(f.sources)),
        ARRAY(SELECT jsonb_array_elements_text(f.hints)),
        ARRAY(SELECT jsonb_array_elements_text(f.negatives)),
        f.transaction_pattern,f.parent_id,NULL,
        f.attribution,f.license,f.url,f.source_record_count
    FROM foursquare f JOIN merchants m ON m.id = f.merchant_id
)
SELECT * FROM contributions
ORDER BY merchant_id,source,external_id,attribution,license,url,transaction_pattern,parent_id;
