-- Fuzzy retrieval uses merchant_search_words plus merchant_search.tokens.
-- Identity comparisons use rule keys, website_host, or normalized-name trigrams;
-- no query uses equality/range ordering on normalized_name.
DROP INDEX IF EXISTS merchant_search_trigrams;
DROP INDEX IF EXISTS merchant_identity_name;
UPDATE ultrafinance_schema SET version=16;
