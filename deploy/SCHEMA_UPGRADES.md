# Database schema upgrade releases

Routine CI deploys do not have database administrator access and do not run
migrations. The API connects lazily, so `/health`, `/docs`, and the homepage
can succeed even when the image cannot use the database. A catalog response
with `database_schema_mismatch` is a permanent release error, not Aurora resume.
The release tool stops immediately and leaves an unpromoted candidate off `live`.

Schema 25 adds a revision trigger on `processor_formats`. This invalidates
compiled rule and processor caches after updates from any database writer,
including direct SQL and other connections. Apply `database init` with the new
binary before serving schema-25 images. Update schema-24 clients together.

Schema 24 removes synthetic positive/negative examples from runtime processor
format records. They now live in repository test fixtures. Apply `database init`
with the new binary before serving schema-24 images. The migration preserves all
rule patterns, processor links, research provenance and enabled states. Schema-23
clients reject the upgraded database, so update active clients together.

Schema 23 adds the processor-format catalog. Apply `database init` with the new
binary before serving schema-23 images, then explicitly import the reviewed
format data; migration does not invent or auto-enable processor formats.
See [format validation and import](../docs/processor-formats.md). Schema-22
binaries reject an upgraded database, so update active clients together.

Schema 22 adds `merchants_counterparty_type_key` on
`merchants(counterparty_type, merchant_key)` for typed catalog retrieval. Existing
databases require `ultrafinance database init` with the new binary before serving
schema-22 images. The index builds transactionally under the catalog writer lock
and blocks writes to `merchants` during construction; allow time for a full-table
index build on large catalogs. The migration removes the statement timeout for
this build only within its transaction. Initial loads defer the index until data
has been loaded. The migration also analyzes `counterparty_type` so ordered type
listings can choose the new index instead of filtering the full name index.
Databases that applied an earlier schema-22 migration without that analysis can
refresh it with `ANALYZE merchants(counterparty_type);`.
Existing schema-21 binaries reject the upgraded schema.

## The schema 14 → 19 cutover

The failure in run 39064834793 is confirmed by the version 37 Lambda logs:
production has schema 14, while commit `8c084ab` requires schema 19. The live
alias remained on version 34. These are observations from that run, not values
to assume for future releases.

Migrations 15–19 change list column types/names, normalize source provenance,
replace Foursquare source evidence with canonical identity/outlet links, and
convert internal merchant references to numeric keys. Public merchant IDs stay
stable. The migrations execute in one transaction under the catalog write lock,
but rewrite large tables and take exclusive locks. Production has approximately
1.79 million merchants, so allow a maintenance window rather than assuming an
instant upgrade.

Old schema-14 images cannot serve the upgraded database. **An application alias
rollback does not roll back the database.** If the upgrade commits and the new
release cannot be made healthy, recovery requires restoring the pre-upgrade
database snapshot and its connection configuration, together with the old image.

## Review and execute a cutover

1. Build and push the candidate image using the normal release pipeline. Record
   its immutable ECR digest and published Lambda version. For this incident,
   the already-built schema-19 candidate is Lambda 37, image
   `920726014496.dkr.ecr.ca-central-1.amazonaws.com/ultrafinance@sha256:3cb1eb879cd48d85ad4d8cfe25937b283b7d40d98203549218116f2716e99f8c`.
   Use a reviewed image that includes the exact migrations being applied.
2. Record the current live version and its immutable image, verify no imports or
   administrator jobs are active, and agree on the maintenance window. Keep
   CI deployment idle during the cutover; do not run parallel releases.
3. Create a manual Aurora cluster snapshot of `ultrafinance-postgres` using the
   configured AWS profile/region. Wait until it is `available` and record its
   identifier. Verify the restore procedure and old image are available before
   applying any migration. Automated backup retention alone is not a substitute
   for this recovery point.
4. Open the existing production administrator CLI using the **candidate** image,
   not the default live image:

   ```sh
   cargo run --locked -- infra cli --image '<immutable-candidate-ECR-digest>'
   ```

   Newly built CLIs support `ultrafinance database check`: it connects read-only,
   reports compatibility, and never applies migrations. On schema 14 it should
   fail with the required version. Older candidate 37 predates that command;
   its mismatch is already established by the Lambda logs.
5. In that administrator shell, run:

   ```sh
   ultrafinance database init
   ultrafinance merchants stats
   ```

   `database init` applies migrations atomically. If it fails, stop and inspect
   the failure; do not promote the image. Once successful, run `database check`
   when supported by the candidate CLI and verify merchant counts, known public
   IDs, and source identities. Close the temporary shell after verification.
6. Release the **same** immutable image with the existing deployment wrapper:

   ```sh
   ./deploy/deploy.sh --image '<immutable-candidate-ECR-digest>'
   ```

   This reuses the built image without trying to capture the homepage example
   through the old, now-incompatible live API. Candidate checks must pass before
   promotion; post-promotion checks retain their guarded rollback behavior.
   If rollback selects an old schema-14 image, the service still needs the
   database restore described above. Do not repeatedly retry the old image.
7. Verify `/health`, `/docs`, `/openapi.json`, the expected API paths, catalog
   browsing, and a real enrichment request on `https://ultrafinance.app`. Record
   the deployed Lambda version. Resume routine CI releases after the new schema
   and live image are confirmed healthy.

Do not print database credentials, Lambda environment configuration, secret
values, or OpenTofu state. The administrator shell receives credentials from
the existing ECS Secrets Manager configuration.
