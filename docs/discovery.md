# Merchant interpretation and discovery

The enrichment pipeline first checks remembered descriptor resolutions, retrieves
catalog candidates using the original description and competing interpretations,
and asks Jev to evaluate the evidence. If the result is unresolved, an optional
discovery service can provide business listings for one further evaluation.

## Preview interpretation

```sh
cargo run -- interpret 'SQ *JULIUS CAFE BROMONT 00482'
```

This identifies a Square processor hint and an unverified terminal numeric token.
It preserves both `JULIUS CAFE BROMONT` as a name and `JULIUS CAFE` with possible
location `BROMONT`, plus the original description. Trailing one- and two-word
spans are possible localities, not confirmed geographic facts. These hypotheses
are copied from the descriptor; Jev evaluates them alongside candidates and the
original transaction. Retrieval compares each hypothesis with catalog names and
aliases. For an equal name, it also checks the proposed locality against the
merchant's stored outlet cities, respecting the transaction country. It attaches
the matching name and supporting outlet source, identifier and attribution to the
candidate supplied to Jev. No matching outlet leaves the locality unconfirmed;
missing coverage is not contradictory evidence.

A complete-name match ranks above a name/locality split supported by an outlet.
Both candidates remain available for evaluation, and this support never grants
an automatic trusted match. Hypotheses never become trusted aliases or location
records. This mechanism uses the existing merchant and outlet tables; it requires
no discovery service or additional business database.

The rules intentionally do not attempt arbitrary date/amount removal, expand
unknown abbreviations, or generate new business names. All original text remains
available to the evaluator. Additional bank formats can be added with examples.

## Configure discovery

Discovery is disabled by default. No external search or place-data subscription
is bundled. Supply an adapter endpoint backed by the business-data/search service
you choose; it must return the structured contract below.

Set `ULTRAFINANCE_DISCOVERY_URL` to an HTTPS endpoint. Optionally set
`ULTRAFINANCE_DISCOVERY_API_KEY` for bearer authentication. Do not embed credentials
in the URL. Jev credentials are also required to evaluate discovered candidates.
Changing the environment requires restarting the application.

The endpoint receives a JSON POST with `description`, `country`, `location`,
`interpretation` and `limit: 5`. It does not receive amount, currency, date or
`extra`. The interpretation contains the original descriptor, processor hint,
unverified tokens, and name/location hypotheses.

Return at most five places, with a total response of at most 64 KiB:

```json
{
  "places": [
    {
      "source": "chosen-business-directory",
      "external_id": "persistent-place-id",
      "name": "Example Cafe",
      "country": "CA",
      "website": "https://example.com",
      "evidence_url": "https://example.com/locations",
      "evidence": "Business listing with its published name and address",
      "attribution": "Directory publisher",
      "license": "License that permits retaining this record"
    }
  ]
}
```

Source and external ID must be stable across requests. Identity, evidence,
attribution and license are required. URLs must be HTTP(S), country must be two
uppercase letters, and individual text fields are bounded. Upstream terms must
permit this use; the adapter records its supplied license rather than granting
rights. Do not manufacture descriptors, addresses or licensing claims.

Listings remain untrusted evidence. Jev can choose none. A selected listing is
imported with its source identity and evidence, then returned with the stable
local merchant ID. Rejected or malformed listings do not add merchants.
Locations mentioned by listings remain evidence; this does not create a verified
outlet record. Retrieval failures and provider failures remain errors.

Discovery has a five-second HTTP timeout, no redirects, and one fallback per
transaction. A batch permits discovery for its first four unresolved transactions
concurrently. Remaining unresolved entries retain their result and history records
`discovery_skipped: batch_budget`; submit smaller batches for further discovery.
No discovery runs on already matched transactions. Search-only evaluation uses
the catalog and remembered mappings, without external discovery or model calls.

## Remember and verify resolutions

Model-supported matches are remembered as **unverified candidates**, with merchant
identity and source provenance. Repeats still require model evaluation. Existing
verified exact aliases keep their fast path and are not replaced by model mappings.

The mapping context includes the normalized complete description, country,
currency, amount, supplied location and all `extra`. Date is omitted. Differences
in those context fields prevent reuse. This deliberately does not generalize a
descriptor across accounts, amounts, localities or changing reference numbers.

```sh
cargo run -- resolutions list
cargo run -- resolutions confirm DESCRIPTOR_ID \
  --evidence 'Receipt checked against the official business website'
cargo run -- resolutions revoke DESCRIPTOR_ID
```

Confirmation is an explicit review action, not a claim that a supplied note was
automatically verified. Check the merchant and context before confirming. A
confirmed mapping can resolve that context without Jev. No transaction descriptor
is added to the merchant's global aliases. Conflicting identities, missing review
evidence and attempts to downgrade verified mappings are rejected. Revocation
removes the mapping while preserving the merchant and original enrichment history.
Mappings have no automatic expiry. Catalog redirects from merchant deduplication
are followed when a mapping is retrieved.

## Database rollout

PostgreSQL requires schema version 4: run
`database init` with a schema-owner connection before running this version. The
mapping fields are columns with a merchant foreign key; variable extra context
and provenance remain JSON. Grant the runtime role the required table and view
permissions. See [database schema](database-schema.md) for the complete schema,
migration and release requirements.

Canonical catalog fingerprints include mappings and verification state. Mapping data is private transaction
context and should be handled alongside private enrichment history.
