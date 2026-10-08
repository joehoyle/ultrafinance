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
PostgreSQL release (see the root README). Use a shared non-administrator application role for
Lambda and the CLI, with catalog read/write permissions. The URL is sensitive and, like
other Lambda secrets managed here, is stored in private OpenTofu state.

For cutover, first apply with `enable_aurora = true` and `database_url = null`.
This provisions the database/network without attaching Lambda to the VPC.
Use `database_endpoint`, `database_admin_secret_arn`,
`database_private_subnet_ids`, and `database_client_security_group_id` outputs
to run the CLI from an authorized migration host inside the VPC (or through
a secure tunnel). Back up and migrate the complete authoritative SQLite catalog,
verify counts/provenance, and create a non-administrator application role.
Set its TLS-enabled URL as `database_url` in the private variable file and apply
again to attach Lambda. Then release using the existing guarded deploy script.
OpenTofu preserves the existing `live` alias throughout this staged cutover;
database creation alone does not deploy the new application.
Use `./infra/tofu.sh` from the repository root for infrastructure commands.
Never print plans or state containing credentials. An old image rollback still serves its bundled
SQLite snapshot; it does not reflect subsequent PostgreSQL edits.

## On-demand CLI shell

CLI resources are included automatically whenever Aurora and an immutable
application image are configured. Apply with `./infra/tofu.sh apply`. This adds
an ECS cluster and ARM64 Fargate task definition, execution/task IAM roles,
a CloudWatch log group, and a Secrets Manager secret populated from `database_url`. There is no ECS
service or continuously running task. Tasks use the existing database client
security group, private subnets, and NAT gateway.

Run `python3 deploy/prod-cli.py` in a terminal to launch the existing application
image and open interactive Bash through ECS Exec, with an `ultrafinance` prompt
showing the working directory. Install the AWS CLI and Session Manager
plugin locally first. The container runs a bounded sleep process with init
process support while the Exec session is active. The runner waits for the
ExecuteCommandAgent to be ready and stops the task when the session ends or
fails. A task also exits after one hour, bounding costs after a lost connection.
During an active session, Ctrl-C is handled by ECS Exec so it can cancel remote
commands. Use `exit` to close the shell and stop the task; Ctrl-C during startup
still cancels the launch and cleans up any known task.
Run `cargo run -- infra cli-cleanup` to stop leftover CLI tasks and wait for
them to stop. This also closes any active production CLI shells.

Set `database_url` in the private infrastructure variables to the shared
TLS-enabled application URL. OpenTofu configures Lambda's environment variable
and populates the Secrets Manager URL injected into shell tasks. Both use the
same non-administrator login with catalog read/write permissions; schema
administration remains separate. The URL is sensitive and stored in private
OpenTofu state. The execution role can read only this secret and the application
ECR repository. The task role has the four `ssmmessages` channel permissions
required for ECS Exec.

The runner selects the immutable image behind Lambda's `live` alias and
registers a temporary task definition revision for the session. Use
`cargo run -- infra cli --latest` (or `python3 deploy/prod-cli.py --latest`) to
select the highest published Lambda version, including one awaiting a database
migration before promotion. Mutable `$LATEST` is excluded. `--image` selects a
specific immutable digest and cannot be combined with `--latest`. The runner stops
known tasks and deregisters their temporary revisions on exit. If launching
has an uncertain outcome, inspect ECS tasks using the printed session ID;
the task's one-hour lifetime still applies. The filesystem is ephemeral.

The operator's AWS profile needs `ecs:DescribeTaskDefinition`,
`ecs:RegisterTaskDefinition`, `ecs:DeregisterTaskDefinition`, `ecs:RunTask`,
`ecs:DescribeTasks`, `ecs:ExecuteCommand`, and `ecs:StopTask`, plus
`iam:PassRole` for the two CLI roles and `lambda:GetFunction`. Tasks do not
inherit the operator's credentials. Fargate compute is charged while the task runs; logs and the
secret have their normal charges.

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
