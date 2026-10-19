ALTER TABLE merchants ADD COLUMN IF NOT EXISTS counterparty_type TEXT NOT NULL DEFAULT 'merchant'
    CHECK (counterparty_type IN ('merchant', 'financial_institution'));
ALTER TABLE manual_merchants ADD COLUMN IF NOT EXISTS counterparty_type TEXT NOT NULL DEFAULT 'merchant'
    CHECK (counterparty_type IN ('merchant', 'financial_institution'));
ALTER TABLE source_records ADD COLUMN IF NOT EXISTS input_counterparty_type TEXT NOT NULL DEFAULT 'merchant'
    CHECK (input_counterparty_type IN ('merchant', 'financial_institution'));
UPDATE ultrafinance_schema SET version=20;
