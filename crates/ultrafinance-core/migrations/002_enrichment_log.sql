CREATE TABLE enrichment_log (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('started', 'matched', 'unresolved', 'error')),
    merchant_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    finished_at TIMESTAMPTZ,
    data TEXT NOT NULL CHECK (data::jsonb IS NOT NULL)
);
CREATE INDEX enrichment_log_created ON enrichment_log(created_at DESC, id);
CREATE INDEX enrichment_log_status ON enrichment_log(status, created_at DESC);
CREATE INDEX enrichment_log_merchant ON enrichment_log(merchant_id, created_at DESC);
UPDATE ultrafinance_schema SET version = 2;
