-- Keep compiled formats fresh across connections and every database writer.
DROP TRIGGER IF EXISTS processor_formats_revision ON processor_formats;
CREATE TRIGGER processor_formats_revision AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE
ON processor_formats FOR EACH STATEMENT EXECUTE FUNCTION advance_catalog_revision();
UPDATE ultrafinance_schema SET version=25;
