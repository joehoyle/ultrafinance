-- Keep upstream mappings intact for source refresh, relinking and audit.
-- Aggregate before serializing: a chain's branches must not be hydrated as
-- thousands of merchant source documents. Distinct credits and matching rules
-- remain separate; consolidation never establishes a new merchant identity.
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
), documents AS (
    SELECT s.merchant_id, s.source, s.external_id, d.data
    FROM selected s JOIN source_records_documents d USING (source, external_id)
    WHERE s.source <> 'foursquare'
    UNION ALL
    SELECT f.merchant_id, 'foursquare', f.external_id,
        jsonb_build_object(
            'source', 'foursquare', 'external_id', f.external_id,
            'merchant', jsonb_build_object(
                'id', f.merchant_id, 'name', m.name, 'website', f.website,
                'markets', f.markets, 'aliases', f.aliases, 'sources', f.sources),
            'attribution', f.attribution, 'license', f.license, 'url', f.url,
            'version', NULL,
            'raw', jsonb_strip_nulls(jsonb_build_object(
                'source_record_count', f.source_record_count,
                'countryHints', f.hints, 'negativeAliases', f.negatives,
                'transaction_text_regexp', f.transaction_pattern, 'parent_id', f.parent_id))
        )::text
    FROM foursquare f JOIN merchants m ON m.id = f.merchant_id
)
SELECT merchant_id, data FROM documents ORDER BY merchant_id, source, external_id, data;
