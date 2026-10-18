-- Optional subsystem: intentionally independent of the merchant schema version.
-- Old application releases ignore these additive tables. Installed by `database init`.
CREATE SCHEMA IF NOT EXISTS ultrafinance_logos;
CREATE TABLE IF NOT EXISTS ultrafinance_logos.assets (
    id text PRIMARY KEY CHECK (id ~ '^[a-f0-9]{64}$'),
    url text NOT NULL,
    mime_type text NOT NULL CHECK (mime_type LIKE 'image/%'),
    width integer NOT NULL CHECK (width > 0),
    height integer NOT NULL CHECK (height > 0),
    byte_length bigint NOT NULL CHECK (byte_length > 0)
);
CREATE TABLE IF NOT EXISTS ultrafinance_logos.website_logos (
    website_url text PRIMARY KEY,
    original_asset_id text NOT NULL REFERENCES ultrafinance_logos.assets(id),
    display_asset_id text NOT NULL REFERENCES ultrafinance_logos.assets(id),
    source_url text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('logo', 'touch_icon', 'manifest_icon', 'favicon')),
    checked_at timestamptz NOT NULL DEFAULT now()
);
-- Match existing explicit catalog readers without granting public access.
DO $$
DECLARE reader record; role_name text;
BEGIN
    FOR reader IN SELECT DISTINCT grantee FROM information_schema.table_privileges
        WHERE table_schema='public' AND table_name='merchants' AND privilege_type='SELECT'
    LOOP
        role_name := CASE WHEN reader.grantee='PUBLIC' THEN 'PUBLIC' ELSE quote_ident(reader.grantee) END;
        EXECUTE 'GRANT USAGE ON SCHEMA ultrafinance_logos TO ' || role_name;
        EXECUTE 'GRANT SELECT ON ALL TABLES IN SCHEMA ultrafinance_logos TO ' || role_name;
    END LOOP;
END $$;
