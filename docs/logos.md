# Website logos

Logo discovery is an administrative CLI job. API requests only read already
stored associations; the homepage continues to use its release snapshot.
Images are immutable static files, served from S3 through CloudFront without
invoking Lambda or waking Aurora.

## Data model

The optional `ultrafinance_logos` PostgreSQL schema has two tables:

- `website_logos`: normalized website URL, original/display asset IDs, discovered
  source URL, kind (`logo`, `touch_icon`, `manifest_icon`, `favicon`), checked time.
- `assets`: content SHA-256, public URL, detected MIME type, actual width/height,
  byte length. Identical bytes reuse one asset across websites.

Website normalization preserves scheme, path, and query. Different storefronts
on a shared host stay separate. Redirect destinations are recorded as image
provenance, never used to merge merchant identities or website associations.
Merchant-provided/imported `logo_url` wins; website associations only fill a
missing logo in catalog retrieval and enrichment. They do not rewrite imports.

The logo tables are additive and installed by `database init` alongside the
merchant schema. They do not change merchant schema compatibility or get created
during API startup. Older releases ignore these tables. Before installation,
lookup behaves as if no discovered logos exist. Installation copies
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
CLI task receives the bucket and HTTPS base URL through environment variables.
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

Outside that shell, pass `--bucket BUCKET --public-base-url https://ultrafinance.app`.
For local preparation, replace `--bucket` with `--directory ./logo-assets` and
set an HTTPS public origin that serves that directory. Local files alone do not
make those URLs public: configure static HTTPS hosting before using them in a
shared catalog. Never expose the directory as an executable document origin;
serve SVG originals as attachments with a restrictive CSP, as production does.

URLs can be supplied as a sequential batch; output is one JSON association per
success. Failures are reported individually and cause a nonzero exit after the
batch. Repeated runs reuse successful associations without website requests.
`--refresh` explicitly checks again. A failed refresh preserves the old record.
Missing icons are not cached as successful logos. Original and display uploads
must succeed before the association is saved; abandoned uploads can be cleaned
up later by comparing keys against the assets table.

## Discovery and output

Discovery considers Organization/LocalBusiness/Corporation/Brand JSON-LD logo
URLs, Apple touch icons, web manifest icons, and favicons, in that order. An
explicit `--image-url` is available for reviewed exceptions. Within a source
category, actual dimensions determine preference, with no advantage beyond
256 pixels. It fetches at most 24 image candidates, two manifests, and six hops
per download, with a two-minute overall discovery deadline. Only standard
HTTP(S) ports and public IP destinations are accepted; every redirect is checked.
Direct requests pin validated DNS addresses. When an outbound proxy is configured,
its destination policy must also enforce public-network access and prevent DNS
rebinding. This CLI is an administrative tool, not a public URL-fetching API.

The original bytes are retained. Type is detected from bytes, not URL suffixes
or response headers. Supported inputs are PNG, JPEG, WebP, GIF, ICO, and SVG;
GIF output uses the first frame. Display output is lossless WebP, at most 256px
on its longest edge, preserving aspect ratio and transparency. Small raster
icons stay small; vectors render at the target size. SVG external/data image
references and system font access are disabled during rasterization; text-only
or image-based SVGs may therefore need an explicit raster alternative. Originals
are not sanitized SVG documents; production marks SVG uploads as attachments
and sends `nosniff` and a sandbox CSP. Only raster display URLs are returned as
merchant logos.

Download limits are 2 MiB HTML, 256 KiB manifests, and 8 MiB images. Raster
processing bounds dimensions to 8192px and decoder allocation to 64 MiB. CPU image
processing is performed by the administrative CLI. No on-demand resizing,
tracking URL, paid provider, or database lookup is involved in serving an image.

Keeping files indefinitely avoids missing images and repeated upstream work.
This is persistent catalog data, rather than a per-page network cache. Website
associations and check times support later scheduled refresh/backfill jobs.
Check the source's reuse terms before bulk importing a third-party collection.
