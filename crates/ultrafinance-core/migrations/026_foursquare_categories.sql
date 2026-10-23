-- Reimport prepared Foursquare data to populate existing category assignments.
ALTER TABLE merchants ADD COLUMN IF NOT EXISTS category_ids TEXT[] NOT NULL DEFAULT '{}';
ALTER TABLE manual_merchants ADD COLUMN IF NOT EXISTS category_ids TEXT[] NOT NULL DEFAULT '{}';
ALTER TABLE source_records ADD COLUMN IF NOT EXISTS input_category_ids TEXT[] NOT NULL DEFAULT '{}';
UPDATE ultrafinance_schema SET version=26;
UPDATE catalog_revision SET revision=revision+1;
