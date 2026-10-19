-- Typed retrieval selects numeric merchant keys before applying its pool limits.
CREATE INDEX IF NOT EXISTS merchants_counterparty_type_key
    ON merchants(counterparty_type, merchant_key);
-- The new type column can lack statistics on existing catalogs. Without them,
-- ordered lists may scan the name index expecting far too many type matches.
ANALYZE merchants(counterparty_type);
UPDATE ultrafinance_schema SET version=22;
