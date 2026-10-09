-- Merge reports are returned to callers; only identity redirects are persisted.
DROP TABLE IF EXISTS merchant_merge_runs, location_merge_runs;
UPDATE ultrafinance_schema SET version=10;
