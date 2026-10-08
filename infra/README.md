# AWS hosting

CloudFront (AWS HTTPS hostname) → Lambda function URL → ARM64 Lambda container.
The existing Axum server runs through [AWS Lambda Web Adapter](https://github.com/aws/aws-lambda-web-adapter).
The website, `/health`, and `/v1/enrich` are public through both CloudFront and
the direct function URL. The function URL uses `NONE` authentication. No client
token or application request throttle is configured.

The region defaults to `ca-central-1` and AWS profile to `joehoyle`. Both are
configurable. A custom domain and remote state backend can be added later.
API responses are never cached. CloudFront forwards requests while replacing
Host for the Lambda function URL. Origin signing is disabled, so standard JSON
POSTs work unchanged.

## SQLite for now

The API currently reads merchant records and does not persist request results or
new discoveries. Keep the authoritative SQLite database locally and package a
catalog snapshot with each release. The Docker build imports JSON using the CLI,
creating SQLite and its FTS5 indexes on Linux. At startup the database is copied
from the image into `/tmp`, because Lambda's image filesystem is read-only.
Each Lambda instance has an independent copy. Changes to `/tmp` are disposable
and are not shared across instances.

The default `deploy/catalog.json` is empty, so an initial deployment returns
`unresolved` until a real catalog is included. Export an existing local database:

```sh
python3 deploy/export-catalog.py data/ultrafinance.sqlite data/merchants.deploy.json
```

Use `--build-arg MERCHANT_CATALOG=data/merchants.deploy.json` when building below.
You can also supply an existing verified JSON catalog. The selected catalog is
intentionally included in the image; credentials, infrastructure state and live
SQLite files are excluded from the Docker context. Only publish merchant data
that belongs in this service.

If the API starts writing shared data, revisit storage before deploying those
writes. A managed Postgres database would require replacing the SQLite store and
its FTS queries. EFS is not configured: SQLite locking over network filesystems
needs care, and this read-only workload does not need a shared filesystem.

## Prepare without AWS access

From the repository root:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
tofu -chdir=infra init -backend=false
tofu -chdir=infra validate
docker buildx build --platform linux/arm64 --provenance=false --load \
  -t ultrafinance:lambda .
```

Local container check (adapter activates on Lambda):

```sh
docker run --rm -p 8080:8080 --read-only --tmpfs /tmp ultrafinance:lambda
curl --fail http://localhost:8080/health
```

## Deploy after login

Install OpenTofu, AWS CLI and Docker with Buildx. Authenticate with `joehoyle`
using whatever login method that profile is configured for, then verify the account:

```sh
aws sts get-caller-identity --profile joehoyle
cp infra/terraform.tfvars.example infra/terraform.tfvars
tofu -chdir=infra init
```

The example configuration uses `aws_use_cli_credentials = true` for the
`joehoyle` profile's AWS CLI login chain. `deploy/tofu.py` exports short-lived
credentials into the OpenTofu process environment without writing them to disk.
The provider checks account `123456789012` before operating. If using a profile
that the provider supports directly, set `aws_use_cli_credentials = false` and
use plain `tofu -chdir=infra` commands instead.

The first apply creates ECR and, if `github_repository` is configured, the GitHub
OIDC deployment role. It creates no application while `image_uri` is null. This avoids
requiring a container image before its repository exists:

```sh
python3 deploy/tofu.py plan
python3 deploy/tofu.py apply
```

Build and push a uniquely tagged image (repository tags are immutable):

```sh
repository=$(tofu -chdir=infra output -raw repository_url)
registry=${repository%%/*}
release=$(date -u +%Y%m%dT%H%M%SZ)
aws ecr get-login-password --profile joehoyle --region ca-central-1 \
  | docker login --username AWS --password-stdin "$registry"
docker buildx build --platform linux/arm64 --provenance=false \
  --build-arg MERCHANT_CATALOG=deploy/catalog.json \
  --tag "$repository:$release" --push .
aws ecr describe-images --profile joehoyle --region ca-central-1 \
  --repository-name ultrafinance --image-ids "imageTag=$release" \
  --query 'imageDetails[0].imageDigest' --output text
```

If you changed region or name, use those values in the commands. Save
`image_uri = "<repository_url>@<returned sha256 digest>"` in `infra/terraform.tfvars`.
This digest bootstraps the first published version and `live` alias. Routine
releases use the deployment command below.
Keep this setting after deployment: removing it would plan deletion of the app resources.

Supply your provider key without putting it in source files:

```sh
read -r -s TF_VAR_typesafe_api_key
export TF_VAR_typesafe_api_key
python3 deploy/tofu.py plan
python3 deploy/tofu.py apply
tofu -chdir=infra output -raw site_url
```

An unset provider key permits exact merchant matches and empty-catalog requests;
fuzzy evaluation requires a valid TypeSafe key. The key is a sensitive OpenTofu
input, stored in local state and Lambda environment configuration. Keep state
private and backed up; do not commit it or saved plan files. Before sharing this
infrastructure among operators, configure an encrypted remote backend with locking.
Future applies must supply the provider key to keep it configured.

Verify the resulting site:

```sh
site=$(tofu -chdir=infra output -raw site_url)
curl --fail "$site/health"
curl --fail "$site/v1/enrich" \
  -H 'Content-Type: application/json' \
  -d '{"description":"LS","country":"CA"}'
```

CloudWatch retains application logs for 14 days. Lambda has 1024 MiB memory and a
30-second timeout; the app's enrichment deadline is 25 seconds. Lambda runs outside
a VPC, avoiding NAT infrastructure for outbound provider calls.


## Routine releases

After the initial application apply, run this from the repository root:

```sh
./deploy/deploy.sh
# Or export the current SQLite catalog and include it in the next release:
python3 deploy/export-catalog.py data/ultrafinance.sqlite data/merchants.deploy.json
MERCHANT_CATALOG=data/merchants.deploy.json ./deploy/deploy.sh
```

The command reads profile, region, ECR repository and function name from local
OpenTofu outputs. It builds and tests the ARM64 image, pushes a unique immutable
tag, updates unpublished Lambda code, waits for readiness, and publishes a version
(or reuses an unchanged published snapshot).
It invokes `/health`, `/`, and an invalid enrichment request on that specific
version before promoting `live`. Invalid input exercises the API without a paid
provider call. Failed candidate checks leave `live` unchanged. Failed checks after
promotion attempt to restore the previous version, using a revision guard so they
cannot overwrite another deployment. The old release continues serving while the
new one builds and is checked. Alias promotion does not prewarm all future Lambda
instances, so cold starts can still occur.

The function URL and its public permissions target `live`; CloudFront's origin
stays stable. OpenTofu ignores changes to the function image and the alias's
version/routing configuration because release tooling owns those values. It still
owns runtime configuration, IAM, and endpoints. A runtime configuration apply can
publish another version but does not promote it; run a release afterwards to make
that configuration live. Keep the bootstrap `image_uri` set: clearing it would
still plan deletion of application resources.

The Dockerfile uses cargo-chef to cache compiled dependencies separately from app
source and includes Rust tests and Clippy in the image build. Local builds reuse
Docker layers; Actions stores intermediate layers in its build cache. Catalog
changes do not invalidate dependency compilation.

## GitHub Actions and OIDC

The example variables restrict the deployment role to `joehoyle/ultrafinance` on
`main`. They include the repository's verified numeric IDs so the trust policy
also supports GitHub's immutable OIDC subject format. Existing name-based subjects
remain limited to that exact repository and branch. No wildcard repository or
branch trust is granted. If the AWS account already has a GitHub OIDC provider,
set `github_oidc_provider_arn` to reuse it instead of creating a duplicate.

After applying infrastructure, add these **repository variables**, not secrets,
in GitHub Settings → Secrets and variables → Actions → Variables:

| Variable | OpenTofu output |
| --- | --- |
| `AWS_DEPLOY_ROLE_ARN` | `deploy_role_arn` |
| `ECR_REPOSITORY` | `repository_url` |
| `AWS_REGION` | `aws_region` (default `ca-central-1`) |
| `LAMBDA_FUNCTION_NAME` | `function_name` (default `ultrafinance`) |

For example, after the application and CI role exist:

```sh
gh variable set AWS_DEPLOY_ROLE_ARN --repo joehoyle/ultrafinance --body "$(tofu -chdir=infra output -raw deploy_role_arn)"
gh variable set ECR_REPOSITORY --repo joehoyle/ultrafinance --body "$(tofu -chdir=infra output -raw repository_url)"
gh variable set AWS_REGION --repo joehoyle/ultrafinance --body "$(tofu -chdir=infra output -raw aws_region)"
gh variable set LAMBDA_FUNCTION_NAME --repo joehoyle/ultrafinance --body "$(tofu -chdir=infra output -raw function_name)"
```

The workflow runs on relevant pushes to `main` or a manual dispatch from `main`.
It skips deployment until `AWS_DEPLOY_ROLE_ARN` and `ECR_REPOSITORY` are configured.
It uses a native ARM64 runner, pinned actions, persistent Docker build caching,
and OIDC credentials. It has no AWS access key or access to infrastructure state.
The role can push to this ECR repository, publish versions of this function,
invoke candidate versions, and promote/roll back `live`. It cannot update Lambda
environment configuration or IAM. Deployments are serialized with
`cancel-in-progress: false`; revision guards also detect local/CI races.

CI uses the committed `deploy/catalog.json` by default. Local ignored catalogs
are not available on GitHub runners. Commit the intended deployable merchant
catalog there if CI releases should include it.

The release output and Actions summary record the previous version. For a manual
rollback, replace `PREVIOUS_VERSION` with that version number and use your profile:

```sh
aws lambda update-alias --profile joehoyle --region ca-central-1 \
  --function-name ultrafinance --name live --function-version PREVIOUS_VERSION
```

Local checks without AWS access:

```sh
python3 -m unittest discover -s deploy -p 'test_*.py'
shellcheck deploy/*.sh
actionlint .github/workflows/deploy.yml
tofu -chdir=infra test
```

Infrastructure tests use a mocked AWS provider; release tests exercise candidate
failure, alias conflicts and guarded rollback without contacting AWS. Actual AWS
version publication, IAM/OIDC authentication and HTTP routing still need a first
live deployment after login.
