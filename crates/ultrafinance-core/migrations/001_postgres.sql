CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE TABLE merchants (
    id TEXT PRIMARY KEY,
    country TEXT CHECK (country IS NULL OR country ~ '^[A-Z]{2}$'),
    data TEXT NOT NULL CHECK (data::jsonb IS NOT NULL)
);
CREATE TABLE manual_merchants (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL CHECK (data::jsonb IS NOT NULL)
);
CREATE TABLE source_records (
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    merchant_id TEXT NOT NULL REFERENCES merchants(id),
    data TEXT NOT NULL CHECK (data::jsonb IS NOT NULL),
    PRIMARY KEY (source, external_id)
);
CREATE INDEX source_records_merchant ON source_records(merchant_id);
CREATE TABLE aliases (
    merchant_id TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    normalized TEXT NOT NULL,
    PRIMARY KEY (merchant_id, normalized)
);
CREATE INDEX aliases_normalized ON aliases(normalized);
CREATE TABLE merchant_search (
    merchant_id TEXT PRIMARY KEY REFERENCES merchants(id) ON DELETE CASCADE,
    text TEXT NOT NULL,
    tokens TSVECTOR GENERATED ALWAYS AS (to_tsvector('simple', text)) STORED
);
CREATE INDEX merchant_search_tokens ON merchant_search USING GIN(tokens);
CREATE INDEX merchant_search_trigrams ON merchant_search USING GIN(text gin_trgm_ops);
CREATE TABLE ultrafinance_schema (version INTEGER PRIMARY KEY);
INSERT INTO ultrafinance_schema VALUES (1);
