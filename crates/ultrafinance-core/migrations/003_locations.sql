CREATE TABLE IF NOT EXISTS location_records (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    merchant_id TEXT REFERENCES merchants(id),
    merchant_source TEXT,
    merchant_external_id TEXT,
    data TEXT NOT NULL CHECK (data::jsonb IS NOT NULL),
    UNIQUE(source, external_id),
    FOREIGN KEY(merchant_source, merchant_external_id) REFERENCES source_records(source, external_id),
    CHECK ((merchant_id IS NOT NULL AND merchant_source IS NULL AND merchant_external_id IS NULL)
        OR (merchant_id IS NULL AND merchant_source IS NOT NULL AND merchant_external_id IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS location_records_merchant ON location_records(merchant_id);
CREATE INDEX IF NOT EXISTS location_records_source_merchant ON location_records(merchant_source, merchant_external_id);
-- This additive catalog extension keeps the version-2 merchant/log contract.
-- Existing application versions can keep running and release rollback remains safe.
