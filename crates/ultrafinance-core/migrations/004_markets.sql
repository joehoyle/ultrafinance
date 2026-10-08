-- JSON country declarations are converted transactionally by migrate_markets.
ALTER TABLE merchants DROP COLUMN country;
UPDATE ultrafinance_schema SET version = 3;
