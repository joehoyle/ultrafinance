`foursquare-categories.csv` is the Places OS / Pro / Premium taxonomy linked
from https://docs.foursquare.com/data-products/docs/categories, retrieved
2026-10-20 (1,279 categories). Use this taxonomy, not the separate
Personalization APIs / Movement SDK taxonomy.

Published CSV:
https://static.observableusercontent.com/files/8bf043336b438a770178f4e9fbacfef6f52fc49c106a199295b42389b920bff193b885db1082f575be2b5853cd75119800b6ce6751961091ce659a4617e56585

The source has category ID, name, and full label. Ancestor IDs are resolved
from label prefixes; IDs themselves do not encode ancestry. Replace the CSV
with a new official snapshot and run the hierarchy tests to update it.


Schema 29 seeds this snapshot into the `categories` database table. Database
names and `parent_id` links are authoritative for API reads; application imports
never overwrite them. Update the table with an administrator migration when the
taxonomy changes, matching entries by `external_id` and preserving existing
integer `id` values. The CSV also supports portable, database-free resolution.
Replacing the CSV alone does not update an initialized database.
