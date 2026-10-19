ALTER TABLE merchants DROP CONSTRAINT merchants_counterparty_type_check;
ALTER TABLE merchants ADD CONSTRAINT merchants_counterparty_type_check
    CHECK (counterparty_type IN ('merchant', 'financial_institution', 'payment_processor'));
ALTER TABLE manual_merchants DROP CONSTRAINT manual_merchants_counterparty_type_check;
ALTER TABLE manual_merchants ADD CONSTRAINT manual_merchants_counterparty_type_check
    CHECK (counterparty_type IN ('merchant', 'financial_institution', 'payment_processor'));
ALTER TABLE source_records DROP CONSTRAINT source_records_input_counterparty_type_check;
ALTER TABLE source_records ADD CONSTRAINT source_records_input_counterparty_type_check
    CHECK (input_counterparty_type IN ('merchant', 'financial_institution', 'payment_processor'));
UPDATE ultrafinance_schema SET version=21;
