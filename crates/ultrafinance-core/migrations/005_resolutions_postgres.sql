CREATE TABLE descriptor_resolutions (
 id TEXT PRIMARY KEY,
 merchant_id TEXT NOT NULL REFERENCES merchants(id),
 description TEXT NOT NULL,
 country TEXT,
 amount TEXT,
 currency TEXT,
 location_present BOOLEAN NOT NULL,
 location_address TEXT,
 location_city TEXT,
 location_region TEXT,
 location_postal_code TEXT,
 location_country TEXT,
 location_store_number TEXT,

 extra_json TEXT NOT NULL,
 provenance_json TEXT NOT NULL,
 verified BOOLEAN NOT NULL DEFAULT false,
 review_evidence TEXT,
 created_at TIMESTAMPTZ NOT NULL DEFAULT (clock_timestamp()),
 updated_at TIMESTAMPTZ NOT NULL DEFAULT (clock_timestamp()),
 CHECK (NOT verified OR (review_evidence IS NOT NULL AND length(trim(review_evidence)) BETWEEN 1 AND 4096))
);
CREATE INDEX descriptor_resolutions_merchant ON descriptor_resolutions(merchant_id,verified);
CREATE INDEX descriptor_resolutions_description ON descriptor_resolutions(description,country);
CREATE VIEW descriptor_resolution_documents AS SELECT r.id, (jsonb_build_object('id',r.id,'context',jsonb_build_object('description',r.description,'country',r.country,'amount',r.amount,'currency',r.currency,'location',CASE WHEN r.location_present THEN jsonb_build_object('address',r.location_address,'city',r.location_city,'region',r.location_region,'postal_code',r.location_postal_code,'country',r.location_country,'store_number',r.location_store_number) ELSE NULL END,'extra',r.extra_json::jsonb),'merchant',m.data::jsonb,'provenance',r.provenance_json::jsonb,'verified',r.verified,'evidence',r.review_evidence))::text AS data FROM descriptor_resolutions r JOIN merchants_documents m ON m.id=r.merchant_id;
