-- Import reconciliation looks up accepted business hosts without a name key.
-- The existing (rule_name, rule_host) index cannot serve that lookup.
CREATE INDEX IF NOT EXISTS merchant_identity_rule_host ON merchant_identity_keys(rule_host, merchant_id)
    WHERE rule_host IS NOT NULL AND rule_host <> '';
UPDATE ultrafinance_schema SET version=12;
