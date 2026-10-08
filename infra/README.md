# AWS infrastructure

From the repository root:

```sh
cd infra
tofu init
tofu apply
```

`terraform.tfvars` contains the private configuration for this deployment,
including the AWS profile, account guard, image digest, domain, and Jev key.
OpenTofu loads it automatically. No wrapper or environment setup is required.
The root `.env` supplies the local Rust CLI/API; OpenTofu uses
`typesafe_api_key` in its own ignored variable file.

## AWS login

The local `joehoyle-tofu` profile delegates to the existing `joehoyle` AWS CLI
login using AWS's native [credential process](https://docs.aws.amazon.com/cli/latest/reference/configure/export-credentials.html).
On another machine, configure it once:

```sh
aws configure set credential_process \
  'aws configure export-credentials --profile joehoyle --format process' \
  --profile joehoyle-tofu
```

Set `aws_profile = "joehoyle-tofu"` in the private variable file. Other AWS
profiles supported directly by the provider can be selected through the same
input. Credentials are temporary and are never saved in the project.
Refresh your normal AWS login when it expires.

## Resources and releases

CloudFront serves the website and API through a public Lambda function URL.
Lambda runs an ARM64 Rust image from ECR and connects to PostgreSQL using
`database_url` in the private OpenTofu variables. Set `enable_aurora = true`
to provision Aurora PostgreSQL 17 Serverless v2: one private writer, encrypted
storage, seven-day backups, deletion protection, and an RDS-managed administrator
password in Secrets Manager. Two private subnets share one managed NAT gateway
for IPv4 HTTPS calls to Jev. This trades AZ-independent outbound availability
for lower idle cost; the single writer also has no standby replica.

Aurora defaults to 0–1 ACU with automatic pause. In Canada Central, at current USD on-demand rates
and 730 hours/month, 0.5 ACU costs $51.10/month; NAT costs $36.50/month plus
$3.65/month for its public IPv4 address. Add Aurora storage ($0.11/GB-month),
I/O ($0.22/million requests), Secrets Manager, data processing/transfer and
other existing application costs. A writer running continuously at 1 ACU
costs $102.20/month for database compute alone. Rates can change; see
[Aurora pricing](https://aws.amazon.com/rds/aurora/pricing/) and
[VPC pricing](https://aws.amazon.com/vpc/pricing/).

`aurora_min_acu = 0` enables auto-pause after five minutes without user
connections. The app sets PostgreSQL's server-side `idle_session_timeout` to
60 seconds and checks/reconnects before each database operation. Jobs already
started are never replayed. API startup connects lazily, so static pages and
health checks do not wake the database. Connections have a 35-second timeout;
catalog/enrichment requests allow 50/55 seconds, with 60-second Lambda and
CloudFront timeouts and 65-second browser timeouts. Resume typically takes
15 seconds, potentially 30 seconds or longer after prolonged inactivity;
unusually slow resumes can still exhaust this bounded request budget.
NAT, storage and secrets still accrue charges while database compute is paused.

The container requires PostgreSQL and never seeds or migrates a database on
startup. Initialize and migrate with the CLI before promoting the first
PostgreSQL release (see the root README). Use a read-only runtime role for
Lambda and a separate write role for imports. The URL is sensitive and, like
other Lambda secrets managed here, is stored in private OpenTofu state.

For cutover, first apply with `enable_aurora = true` and `database_url = null`.
This provisions the database/network without attaching Lambda to the VPC.
Use `database_endpoint`, `database_admin_secret_arn`,
`database_private_subnet_ids`, and `database_client_security_group_id` outputs
to run the CLI from an authorized migration host inside the VPC (or through
a secure tunnel). Back up and migrate the complete authoritative SQLite catalog,
verify counts/provenance, and create a separate read-only runtime role.
Set its TLS-enabled URL as `database_url` in the private variable file and apply
again to attach Lambda. Then release using the existing guarded deploy script.
OpenTofu preserves the existing `live` alias throughout this staged cutover;
database creation alone does not deploy the new application.
Use `./infra/tofu.sh` from the repository root for infrastructure commands.
Never print plans or state containing credentials. An old image rollback still serves its bundled
SQLite snapshot; it does not reflect subsequent PostgreSQL edits.

## On-demand CLI tasks

CLI tasks are included automatically whenever Aurora and an immutable application
image are configured. Apply with `./infra/tofu.sh apply`. This adds an ECS
cluster and ARM64 Fargate task definition, execution/task IAM roles, a private
S3 input bucket, a CloudWatch log group, and an empty Secrets Manager secret.
There is no ECS service or continuously running task. The tasks reuse the
existing database client security group, private subnets, and NAT gateway.

`python3 deploy/prod-cli.py --configure-database` prompts for the import-role
PostgreSQL URL and writes it directly to Secrets Manager, outside OpenTofu.
The task execution role can read only that secret and the application's ECR
repository; the task role can read only the staging bucket's `jobs/` objects.
The CLI container receives the URL through ECS secret injection. Lambda keeps
its independently configured runtime credentials.

See the root README for import/migration commands. The runner chooses the
immutable image behind Lambda's `live` alias at each invocation, registering
a temporary task definition revision when it differs from the infrastructure
image. `--image` can select a newer image during initial database cutover.
Completed runs deregister temporary revisions and remove their staged input.
Interrupted or uncertain runs preserve both; inspect their ECS status before
retrying. Input files expire after seven days, and logs after fourteen days.
Fargate compute is charged only while tasks run; S3, logs, and the secret have
their normal storage/request charges.

The operator's AWS profile needs `ecs:DescribeTaskDefinition`,
`ecs:RegisterTaskDefinition`, `ecs:DeregisterTaskDefinition`, `ecs:RunTask`,
and `ecs:DescribeTasks`, plus `iam:PassRole` for the two CLI roles. It also
needs staging-bucket upload/delete permissions, `logs:FilterLogEvents`,
`lambda:GetFunction`, and `secretsmanager:DescribeSecret` on the CLI secret.
Configuring the URL additionally requires `secretsmanager:PutSecretValue`.
Tasks do not inherit the operator's credentials. No provider credentials are
injected, so enrichment/evaluation jobs are not offered by this runner.

OpenTofu owns ECR, IAM, logging, Lambda runtime configuration, the function URL,
CloudFront, Route 53, and the ACM certificate in `us-east-1`. It preserves
unmanaged DNS records and protects the hosted zone against deletion.

With `image_uri = null`, the initial apply creates ECR and the GitHub OIDC role.
After pushing the first image, configure its immutable digest and apply again.
Keep `image_uri` set for an existing deployment: clearing it would plan deletion
of the application resources.

Routine releases use `./deploy/deploy.sh` from the repository root or GitHub
Actions. Release tooling owns the image and the version selected by `live`;
it tests new versions before promotion and supports guarded rollback.
After changing Lambda runtime settings with OpenTofu, run a release to promote
the newly published configuration to `live`.

## Private files

The active `terraform.tfstate` records the deployment and contains the Jev key.
Keep it private and preserve it. State, generated backups, variable files, plans,
provider locks, and `.terraform` are ignored. OpenTofu regenerates its cache and
lock file during `init`. No remote state backend is configured yet.

For a new installation, use the inputs declared in `variables.tf`. An existing
public hosted zone must be imported before applying the domain configuration:

```sh
tofu import 'aws_route53_zone.site[0]' YOUR_HOSTED_ZONE_ID
```

Validation:

```sh
tofu fmt -check -recursive
tofu validate
tofu test
```
