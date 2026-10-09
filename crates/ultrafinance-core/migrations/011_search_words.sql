-- Fuzzy-match distinct lexemes, then use the existing full-text index to
-- retrieve current merchant IDs. This avoids scanning merchant-sized posting
-- lists for every trigram in every descriptor hypothesis.
CREATE TABLE IF NOT EXISTS merchant_search_words (word TEXT PRIMARY KEY);
INSERT INTO merchant_search_words(word)
SELECT DISTINCT word FROM merchant_search
CROSS JOIN LATERAL unnest(tsvector_to_array(tokens)) words(word)
WHERE char_length(word) >= 3
ON CONFLICT(word) DO NOTHING;
CREATE INDEX IF NOT EXISTS merchant_search_words_trigrams
    ON merchant_search_words USING GIN(word gin_trgm_ops);

-- Statement-level transition tables keep bulk imports bulk operations.
CREATE OR REPLACE FUNCTION remember_merchant_search_words() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO merchant_search_words(word)
    SELECT DISTINCT word FROM new_search_rows
    CROSS JOIN LATERAL unnest(tsvector_to_array(tokens)) words(word)
    WHERE char_length(word) >= 3
    ON CONFLICT(word) DO NOTHING;
    RETURN NULL;
END;
$$;
DROP TRIGGER IF EXISTS merchant_search_words_insert ON merchant_search;
CREATE TRIGGER merchant_search_words_insert AFTER INSERT ON merchant_search
    REFERENCING NEW TABLE AS new_search_rows FOR EACH STATEMENT
    EXECUTE FUNCTION remember_merchant_search_words();
DROP TRIGGER IF EXISTS merchant_search_words_update ON merchant_search;
CREATE TRIGGER merchant_search_words_update AFTER UPDATE ON merchant_search
    REFERENCING NEW TABLE AS new_search_rows FOR EACH STATEMENT
    EXECUTE FUNCTION remember_merchant_search_words();
-- Obsolete vocabulary is harmless: merchant IDs always come from current
-- merchant_search rows, never the dictionary. No merchant evidence is retained.
ANALYZE merchant_search_words;
UPDATE ultrafinance_schema SET version=11;
