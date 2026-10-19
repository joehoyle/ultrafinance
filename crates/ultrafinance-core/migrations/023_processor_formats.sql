-- Researched descriptor formats are catalog data, separate from identity aliases.
CREATE TABLE IF NOT EXISTS processor_formats (
    id TEXT PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 128),
    processor_id TEXT NOT NULL REFERENCES merchants(id) ON DELETE CASCADE,
    pattern TEXT NOT NULL CHECK (length(pattern) BETWEEN 1 AND 4096),
    sources TEXT[] NOT NULL CHECK (cardinality(sources) > 0),
    positive_examples JSONB NOT NULL CHECK (jsonb_typeof(positive_examples)='array'),
    negative_examples TEXT[] NOT NULL,
    reviewed_on DATE NOT NULL,
    scope TEXT NOT NULL CHECK (length(trim(scope)) > 0),
    notes TEXT NOT NULL DEFAULT '',
    enabled BOOLEAN NOT NULL DEFAULT true
);
CREATE INDEX IF NOT EXISTS processor_formats_processor ON processor_formats(processor_id);
UPDATE ultrafinance_schema SET version=23;
