# Offline place reference data

This generated GeoNames directory recognizes geographic clues in descriptors.
It is independent of merchant outlet tables: loading it requires no database,
creates no merchant records, and provides no evidence that a merchant operates
in a place. Ambiguous names retain all matching GeoNames IDs.

The bundled snapshot is derived from [GeoNames](https://www.geonames.org/),
licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
Changes include selecting populated places from cities500, restricting alternate
names, normalizing aliases, and mapping administrative regions to ISO codes.
Subdivision mappings derive from pycountry / Debian iso-codes under LGPL 2.1;
see ISO-LICENSE.txt. Source URLs, input hashes, policies and the output hash are
recorded in manifest.json. No population ranking resolves ambiguous names.

Inspect the current snapshot and browse places:

```sh
cargo run --locked -- locations gazetteer
cargo run --locked -- locations gazetteer list --country CA --region ON --limit 50
cargo run --locked -- locations gazetteer Springfield --country US
cargo run --locked -- locations gazetteer lookup List --country DE
```

Raw inputs are kept outside Git in the ignored data/sources directory. Verify
this snapshot from its preserved inputs without downloading or changing output:

```sh
python3 scripts/generate_gazetteer.py --input-dir data/sources/geonames/2026-10-09 --verify
```

To refresh, choose a new snapshot directory and download the public inputs:

```sh
python3 scripts/generate_gazetteer.py --input-dir data/sources/geonames/NEW-SNAPSHOT --download
```

Review the generated manifest and dataset diff, then run core and CLI tests.
Refreshing reference data does not import or modify merchant locations.
