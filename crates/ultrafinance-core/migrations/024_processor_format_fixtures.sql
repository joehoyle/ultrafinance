-- Synthetic regex examples belong in repository fixtures, not runtime records.
ALTER TABLE processor_formats DROP COLUMN IF EXISTS positive_examples, DROP COLUMN IF EXISTS negative_examples;
UPDATE ultrafinance_schema SET version=24;
