# Starter outlet catalog

`open-enrichment-au.json` is a small source-backed catalog of Apple Canberra
(R483) and Apple Sydney (R238). Addresses, coordinates, external record IDs, and
Apple/Google place IDs come from Open Enrichment's Australian CSV, inspected on
2026-10-08:

https://github.com/steveharrison/openenrichment/blob/main/src/public/data/au/merchants.csv

The CSV data is CC0-1.0. These are source-consistency examples, not independently
verified current store addresses. The transaction patterns deliberately require
the store identifier, with word boundaries, rather than a generic brand match.
No logos are included.

`merchants.example.json` supplies their parent brand for a fresh database. Its
source/external identity is the same as the Open Enrichment global Apple record,
so an existing import keeps its local merchant ID. Importing this small parent
fixture refreshes that source record's aliases; for an existing catalog, use the
full global dataset instead, or link the source reference to a verified merchant.

```sh
cargo run -- merchants import data/locations/merchants.example.json --source open-enrichment
cargo run -- locations import data/locations/open-enrichment-au.json
cargo run -- locations eval evals/location-smoke.json
```

Outlet imports resolve merchant references, retain attribution/place IDs, assign
stable `loc_…` service IDs, and refresh by `(source, external_id)`. Externally
linked merchants are followed at read time. A record with `manual_override: true`
protects its corrections from later source refreshes; another explicit manual
record can update it. Export records with `locations list MERCHANT_ID`, edit the
JSON, and import the edited array.

`locations eval` is offline. Its optional merchant reference supplies the known
merchant to isolate location matching; it does not measure merchant resolution.
Field accuracy, exact outlet accuracy, and unresolved accuracy are reported
separately. Unlabeled fields do not count as correct. The smoke suite includes
positive geography/outlet examples and ambiguous/billing descriptions; its
source-consistency scores do not establish accuracy on unseen real transactions.
