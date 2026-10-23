# Website logos

Logo discovery is an administrative CLI job. API requests only read already
stored associations; the homepage continues to use its release snapshot.
Images are immutable static files. Locally the API serves them from `./logos`;
production serves them from S3 through CloudFront without invoking Lambda or
waking Aurora.

## Data model

The optional `ultrafinance_logos` PostgreSQL schema has two tables:

- `website_logos`: normalized website URL, display asset ID, discovered
  source URL, kind (`logo`, `touch_icon`, `manifest_icon`, `favicon`), checked time.
- `assets`: content SHA-256, origin-relative asset path, detected MIME type, actual width/height,
  byte length. Identical bytes reuse one asset across websites.

Website normalization preserves scheme, path, and query. Different storefronts
on a shared host stay separate. Redirect destinations are recorded as image
provenance, never used to merge merchant identities or website associations.
Merchant-provided/imported `logo_url` wins; website associations only fill a
missing logo in catalog retrieval and enrichment. They do not rewrite imports.

The logo tables are additive and installed by `database init` alongside the
merchant schema. They do not change merchant schema compatibility or get created
during API startup. Older releases ignore these tables. Before installation,
lookup behaves as if no discovered logos exist. Rerun `database init` when
upgrading from original/display storage to remove the old original reference
column. Existing display associations are preserved. Installation copies
SELECT/USAGE grants for existing catalog readers; if a new runtime DB role is
created later, rerun `database init` as the administrator to grant access.

## Setup and operation

Use the usual database environment configuration. The regular setup command
initializes or upgrades the merchant database and installs the logo tables in
the same transaction:

```sh
ultrafinance database init
ultrafinance logos list
ultrafinance logos list --limit 25 --offset 25
ultrafinance logos list --json
ultrafinance logos get https://example.com
ultrafinance logos get https://example.com --json
```

`logos list` shows saved associations in website URL order, with image types,
dimensions, display URLs, and check times. `logos get` shows the full metadata
for one website. Both commands support `--json` and make no website requests.
List JSON contains `logos`, `total`, `limit`, and `offset`; a missing get result
is JSON `null`.

Production infrastructure definitions add a private S3 bucket, signed CloudFront
origin at `/logos/*`, and write permission for the existing administrative CLI
role. Apply through the project's normal reviewed OpenTofu workflow. The deployed
CLI task receives the bucket through an environment variable.
The runtime image includes AWS CLI, which uses the operator/task's normal AWS
credentials. No vendor account or logo API key is required.

```sh
# In the configured production CLI shell, after infrastructure and schema setup:
ultrafinance logos discover https://example.com https://another.example
ultrafinance logos discover https://example.com --refresh
# Explicit reviewed official logo, bypassing HTML discovery:
ultrafinance logos discover https://example.com --refresh \
  --image-url https://example.com/brand/logo.svg
```

Locally, no AWS configuration is needed:

```sh
ultrafinance logos discover https://example.com
cargo run -p ultrafinance-api
```

### Populate a market

Select merchants by known operating country, using a two-letter country code:

```sh
# Count Canadian merchant websites still needing discovery (no writes or requests):
ultrafinance logos discover --market CA --dry-run
# Try a small batch:
ultrafinance logos discover --market CA --limit 25 --workers 4
# Populate all remaining eligible Canadian websites:
ultrafinance logos discover --market CA --workers 8
```

Selection skips merchants without websites or with imported/manual logos,
normalizes and deduplicates website URLs across branches, and skips websites
with saved associations. It uses operating-market data, not the website's TLD
or a merchant's headquarters. `CA` includes merchants operating in Canada,
including international brands. Province/city filters are not supported.
`--limit` counts unique pending websites, in URL order. `--dry-run` reports the
full remaining count and how many would run after applying `--limit`, without
fetching images or changing data. A completed market reports zero remaining. `--refresh` includes saved
associations to explicitly rediscover them; manual logos still take precedence.

Discovery uses four concurrent website jobs by default. Set `--workers N`
(`--threads` is an alias) to choose 1–64; `--workers 1` runs sequentially.
Workers overlap network requests across websites, while each website's candidate
requests remain sequential. Image processing, local/S3 publishing, and saving
finish within the same worker slot, so a full market never creates a task for
every website at once. These are asynchronous jobs, not a count of OS threads.
Market runs show one updating progress bar on stderr, with completed/total,
percentage, saved/failed counts, worker count, and elapsed time. Selection shows
a spinner while the market is scanned. Market runs suppress per-website JSON
and errors; a final failure count still causes a nonzero exit. Redirected logs
receive periodic summaries instead of terminal control codes. Explicit website
runs retain JSON lines on stdout and individual errors on stderr.
Rerunning continues with websites that have no saved association; failures are retried, while successes are reused. This is best-effort
coverage: not every website provides a usable logo, and merchants without a
website need separate research. A full market may take a long time; start with
a limited batch. The bulk commands use the same local directory/S3 settings as
single-website discovery.

### Local storage

Run discovery and the API from the project root. Discovery defaults to `./logos`, with
files at `./logos/assets/<sha256>.<extension>`. The API serves those files at
`http://127.0.0.1:3000/logos/assets/<sha256>.<extension>` without database access.
The directory is ignored by Git and Docker builds. Only resized WebP display
images are stored; downloaded originals are discarded after conversion.

Use `--directory PATH` to change the CLI directory and set
`ULTRAFINANCE_LOGO_DIRECTORY=PATH` for the API. The existing
`ULTRAFINANCE_LOGO_BUCKET` setting or `--bucket BUCKET` selects S3 storage.
No public base URL configuration is needed for either backend: both serve the
same `/logos/assets/<sha256>.<extension>` paths. The database stores these paths
without a scheme, host, or port, so catalogs can move between environments.
Browsers use their current origin; API clients should resolve paths against the
API origin (for example, `new URL(logo_url, apiBaseUrl)`), including when the API
uses HTTP or runs behind a proxy. Imported/manual absolute logo URLs retain
their existing behavior. Legacy managed absolute URLs are returned as origin-relative paths on reads,
without rediscovery. External logo URLs keep their original origins.
Rediscovery with `--refresh` also upgrades the stored managed asset paths.

URLs can be supplied as a batch; output is one JSON association per
success. Failures are reported individually and cause a nonzero exit after the
batch. Repeated runs reuse successful associations without website requests.
`--refresh` explicitly checks again. A failed refresh preserves the old record.
Missing icons are not cached as successful logos. The display upload
must succeed before the association is saved; abandoned uploads can be cleaned
up later by comparing keys against the assets table.

When upgrading existing storage, the schema change preserves display associations
and leaves old files and asset metadata for cleanup. Delete only original assets
that are unused by every `website_logos.display_asset_id` and by explicit merchant
logo URLs; an original can share its content hash with a display asset. Check
references before deleting local files or S3 objects.

## Discovery and output

Discovery considers Organization/LocalBusiness/Corporation/Brand JSON-LD logo
URLs, Apple touch icons, web manifest icons, and favicons, in that order. An
explicit `--image-url` is available for reviewed exceptions. Within a source
category, actual dimensions determine preference, with no advantage beyond
256 pixels. If page loading fails, HTML is not valid UTF-8, or declared images
are unusable, discovery tries conventional paths: `/apple-touch-icon.png`,
`/apple-touch-icon-precomposed.png`, `/favicon.ico`, and `/favicon.png`.
Fallbacks use the final page origin after a successful redirect, otherwise the
original website origin. They retain the same public-network checks, image
validation, and overall two-minute deadline. Page loading is capped at 20 seconds,
and the primary discovery phase at 75 seconds to leave time for fallbacks.
An explicit `--image-url` continues to fetch only that image. It fetches at most 24 image candidates, two manifests, and six hops
per download, with a two-minute overall discovery deadline. Only standard
HTTP(S) ports and public IP destinations are accepted; every redirect is checked.
Direct requests pin validated DNS addresses. When an outbound proxy is configured,
its destination policy must also enforce public-network access and prevent DNS
rebinding. This CLI is an administrative tool, not a public URL-fetching API.

Original bytes are discarded after conversion. Type is detected from bytes, not URL suffixes
or response headers. Supported inputs are PNG, JPEG, WebP, GIF, ICO, and SVG;
GIF output uses the first frame. Display output is lossless WebP, at most 256px
on its longest edge, preserving aspect ratio and transparency. Small raster
icons stay small; vectors render at the target size. SVG external/data image
references and system font access are disabled during rasterization; text-only
or image-based SVGs may therefore need an explicit raster alternative. Only raster display URLs are returned as merchant logos.

Download limits are 2 MiB HTML, 256 KiB manifests, and 8 MiB images. Raster
processing bounds dimensions to 8192px and decoder allocation to 64 MiB. CPU image
processing is performed by the administrative CLI. No on-demand resizing,
tracking URL, paid provider, or database lookup is involved in serving an image.

Keeping files indefinitely avoids missing images and repeated upstream work.
This is persistent catalog data, rather than a per-page network cache. Website
associations and check times support later scheduled refresh/backfill jobs.
Check the source's reuse terms before bulk importing a third-party collection.
