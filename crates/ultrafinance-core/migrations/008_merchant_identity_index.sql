-- Keys are computed by the shared Rust normalizer, including Unicode and URL rules.
CREATE TABLE merchant_identity_keys (
    merchant_id TEXT PRIMARY KEY REFERENCES merchants(id) ON DELETE CASCADE,
    normalized_name TEXT NOT NULL,
    website_host TEXT,
    rule_name TEXT,
    rule_host TEXT
);
CREATE INDEX merchant_identity_rule ON merchant_identity_keys(rule_name,rule_host,merchant_id) WHERE rule_name IS NOT NULL;
CREATE INDEX merchant_identity_name ON merchant_identity_keys(normalized_name);
CREATE INDEX merchant_identity_host ON merchant_identity_keys(website_host) WHERE website_host IS NOT NULL;
CREATE INDEX merchant_identity_trigrams ON merchant_identity_keys USING GIN(normalized_name gin_trgm_ops);
CREATE TABLE catalog_revision (singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK(singleton), revision BIGINT NOT NULL);
INSERT INTO catalog_revision VALUES(TRUE,0);
CREATE FUNCTION advance_catalog_revision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE catalog_revision SET revision=revision+1;
    RETURN NULL;
END $$;
CREATE TRIGGER merchants_revision AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON merchants FOR EACH STATEMENT EXECUTE FUNCTION advance_catalog_revision();
CREATE TRIGGER sources_revision AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON source_records FOR EACH STATEMENT EXECUTE FUNCTION advance_catalog_revision();
CREATE TRIGGER manual_revision AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON manual_merchants FOR EACH STATEMENT EXECUTE FUNCTION advance_catalog_revision();
CREATE TRIGGER locations_revision AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON location_records FOR EACH STATEMENT EXECUTE FUNCTION advance_catalog_revision();
CREATE TRIGGER redirects_revision AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON merchant_redirects FOR EACH STATEMENT EXECUTE FUNCTION advance_catalog_revision();
UPDATE ultrafinance_schema SET version=7;
