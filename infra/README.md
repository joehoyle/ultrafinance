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

`foursquare_token` is an optional sensitive Places Portal token in that private
variable file. OpenTofu sets `ULTRAFINANCE_FOURSQUARE_TOKEN` alongside Lambda's
other API keys. Optional `openai_api_key` enables explicit research and opt-in
enrichment research fallback; `research_model` selects its model. The OpenAI,
Foursquare and Jev keys are stored in Secrets Manager
for injection into new CLI tasks; existing tasks retain their environment.
Token values remain
private in configuration/state and must never be printed in plans or logs.

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

The current production private configuration overrides `aurora_max_acu` to 32
for large imports, retaining `aurora_min_acu = 0` and five-minute auto-pause.
The ceiling permits scaling under demand; it does not reserve 32 ACUs or
guarantee import throughput. Database compute is billed at actual capacity.
Reduce the production override after import work if a lower spending ceiling
is preferred. This database capacity change does not require an image release.

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
PostgreSQL release (see the root README). Use a non-administrator application
role for Lambda, with catalog read/write permissions. CLI shells use the RDS
administrator for migrations and catalog work. The application URL is sensitive
and, like other Lambda secrets managed here, is stored in private OpenTofu state.

Schema changes in later releases also require an administrator migration.
`ultrafinance database check` checks compatibility without changing the schema.
Follow the [schema upgrade cutover](../deploy/SCHEMA_UPGRADES.md) for incompatible
versions: take a recovery snapshot, migrate during a maintenance window with the
candidate's CLI image, and promote that same immutable image. CI cannot perform
this administrator operation; repeated 503 checks cannot fix a schema mismatch.

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
a CloudWatch log group, and access to the RDS-managed administrator secret.
There is no ECS service or continuously running task. Tasks use the existing database client
security group, private subnets, and NAT gateway.

Run `cargo run -- infra cli` in a terminal to launch the existing application
image and open interactive Bash through ECS Exec, with an `ultrafinance` prompt
showing the working directory. The image includes `tmux` and uses HTTPS APT
sources for additional package installs. The default temporary mode stops the
task when its CLI connection closes, even if tmux was started manually.
Install the AWS CLI and Session Manager
plugin locally first. The container runs a bounded sleep process with init
process support while the Exec session is active. The runner waits for the
ExecuteCommandAgent to be ready and stops the task when the session ends or
fails. A task also exits after 24 hours, bounding costs after a lost connection.
During an active session, Ctrl-C is handled by ECS Exec so it can cancel remote
commands. Use `exit` to close the shell and stop the task; Ctrl-C during startup
still cancels the launch and cleans up any known task.
Run `cargo run -- infra cli-cleanup` to stop leftover CLI tasks and wait for
them to stop. This also closes any active production CLI shells.

For long-running work, use a persistent tmux session:

```sh
cargo run -- infra cli --persistent
# Optional: override the persistent mode's default 24-hour lifetime (1–168 hours).
cargo run -- infra cli --persistent --lifetime-hours 72
# List active session IDs, statuses, and start times:
cargo run -- infra cli-list
# Use the 32-character session ID printed at launch:
cargo run -- infra cli-attach SESSION_ID
cargo run -- infra cli-stop SESSION_ID
```

`cli-list` lists active tasks from the configured CLI task family, newest first,
including tasks still starting. It shows the session ID, ECS status, start time
(or creation time while starting), and task ID. It lists temporary and persistent
sessions; `cli-attach` and `cli-stop` require a persistent session.

Persistent mode starts or attaches to the `ultrafinance` tmux session automatically.
Mouse mode is enabled in the image: use the scroll wheel to browse pane history
and click to select panes. Press `q` to leave tmux's scroll/copy mode. The local CLI
restores terminal settings and disables mouse reporting when ECS Exec returns,
including after connection errors. If an older CLI leaves mouse coordinates
appearing at your local prompt, press Ctrl-C and run:

```sh
printf '\033[?1000l\033[?1002l\033[?1003l\033[?1006l'
```

Detach with Ctrl-B then D. Detaching, closing the last shell, or losing the
connection leaves the task and its scratch disk running. Reconnecting reaches
the same task, files, and surviving tmux windows; it never launches another task
or resets the lifetime. The launcher prints the session ID, reconnect/stop commands,
and maximum lifetime before launch. `cli-stop` stops that session and waits for
termination; the scratch disk is configured for deletion when the task stops.
`cli-cleanup` stops all CLI tasks, including persistent ones. Task and disk charges
continue while a persistent session runs, including while disconnected.

To save files between tasks, finish any imports/downloads in every tmux window,
run `sync` in the remote shell, then detach and use:

```sh
cargo run -- infra cli-stop SESSION_ID --save-data
# Use the snapshot ID printed by the save command:
cargo run -- infra cli --persistent --snapshot snap-0123456789abcdef0
```

`--save-data` finds that session's attached `/scratch` disk, creates an encrypted
EBS snapshot, prints its ID and progress, and waits up to six hours for completion
before requesting a stop. Snapshot errors, interrupted waits, or timeouts do not
send a stop request; the task's existing expiry still applies. An interrupted
snapshot may finish independently, so check its printed ID before retrying.
To continue waiting on an existing snapshot without creating another one:

```sh
cargo run -- infra cli-stop SESSION_ID --save-data --snapshot SNAPSHOT_ID --wait-hours 6
```

The CLI checks that the snapshot came from this session's scratch disk before
stopping; `--wait-hours` accepts 1–24 hours and defaults to six. Reusing a snapshot
preserves only the files captured at its original creation time.

Snapshots capture disk files at creation time, not running processes or tmux
windows; writes made afterwards are not saved. Without finishing writes and
running `sync` first, an attached-disk snapshot is only crash-consistent.

Restoring creates a new disk from a completed snapshot in the configured AWS
region, with at least the snapshot's original volume size, and uses the selected
image (live by default). Finish saving well before the original task expires.
Snapshots remain after task deletion and incur storage charges until explicitly
deleted, for example with `aws ec2 delete-snapshot --snapshot-id SNAPSHOT_ID`
using the configured profile and region. Neither `cli-stop` nor `cli-cleanup`
deletes snapshots. Saving uses the local AWS profile's `ec2:CreateSnapshot`,
`ec2:CreateTags`, and `ec2:DescribeSnapshots` permissions; it does not grant these
to the container. Restoring also requires the ECS volume role to have access to
the snapshot's encryption key (the current default EBS key is supported).

Startup failures still clean up known tasks. Once ready, even failure of the first
ECS Exec connection preserves the persistent task for a reconnect. The image must
include tmux. Expiry or a task failure still ends its processes and deletes scratch
files; persistent mode protects against client disconnection, not task failure.

Set `database_url` in the private infrastructure variables to the TLS-enabled
application URL used by Lambda. CLI tasks instead receive the administrator
username and password directly from the RDS-managed secret using ECS JSON-key
secret injection. The execution role can read the administrator and shared API-key secrets and
the application ECR repository; the task role retains only the ECS Exec channel
permissions. Password rotation is picked up when a new task starts.

The launcher builds the administrator connection URL inside the remote shell
with percent-encoded credentials and `sslmode=require`, without printing the
password or placing it in command arguments or task definitions. This works
with existing Bash-enabled application images. Reopen the shell after applying
CLI infrastructure changes; existing tasks retain their original credentials.

The runner selects the immutable image behind Lambda's `live` alias and
registers a temporary task definition revision for the session. Use
`cargo run -- infra cli --latest` to
select the highest published Lambda version, including one awaiting a database
migration before promotion. Mutable `$LATEST` is excluded. `--image` selects a
specific immutable digest and cannot be combined with `--latest`. The runner stops
known temporary tasks and deregisters temporary revisions on exit. Persistent
tasks remain running even after their temporary revision is deregistered; ECS
[continues existing tasks using inactive revisions](https://docs.aws.amazon.com/AmazonECS/latest/APIReference/API_DeregisterTaskDefinition.html). If launching
has an uncertain outcome, inspect ECS tasks using the printed session ID;
the task's configured lifetime still applies. Each session has 8 vCPUs, 32 GiB RAM, and a fresh encrypted 1 TiB gp3 EBS scratch disk mounted at `/scratch`. The shell starts there, so relative download and bundle paths use that disk. `TMPDIR=/scratch/tmp` also places download and sorting temporary files on it. `cli_scratch_gib` and `cli_lifetime_hours` can override the disk size and temporary-mode lifetime in the private variable file; persistent mode uses `--lifetime-hours` (default 24). The disk is deleted when the task stops; committed database imports persist. There are no disk or task compute charges between sessions. Size is conservative headroom, not a measured guarantee for future global exports. Check `df -h /scratch` during preparation.

The operator's AWS profile needs `ecs:DescribeTaskDefinition`,
`ecs:RegisterTaskDefinition`, `ecs:DeregisterTaskDefinition`, `ecs:RunTask`,
`ecs:DescribeTasks`, `ecs:ListTasks`, `ecs:ExecuteCommand`, and `ecs:StopTask`, plus
`iam:PassRole` for the execution, task, and ECS volume infrastructure roles, `lambda:GetFunction`, and
`lambda:ListVersionsByFunction` for `--latest`. Tasks do not
inherit the operator's credentials. Fargate compute is charged while the task runs; logs and the
secret have their normal charges.

OpenTofu owns ECR, IAM, logging, Lambda runtime configuration, the function URL,
CloudFront, Route 53, and the ACM certificate in `us-east-1`. It preserves
unmanaged DNS records and protects the hosted zone against deletion.

With `image_uri = null`, the initial apply creates ECR and the GitHub OIDC role.
After pushing the first image, configure its immutable digest and apply again.
Keep `image_uri` set for an existing deployment: clearing it would plan deletion
of the application resources.

Routine releases use `cargo run -- infra deploy` from the repository root or GitHub
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

## Import production into local PostgreSQL

Run `docker compose up -d --wait postgres`, then `cargo run -- infra import-db` from the workspace. This copies the full production database into the
default local database, including private enrichment history, overwriting local
copies of production tables. The command ignores application database URL overrides.

Install PostgreSQL client tools (`pg_dump` 17+, matching `pg_restore`, `psql`), AWS
CLI, and Session Manager plugin. Alongside the existing CLI runner permissions,
the operator needs `secretsmanager:GetSecretValue` for the RDS administrator
secret and `ssm:StartSession` for the ECS target and
`AWS-StartPortForwardingSessionToRemoteHost` document. The existing task role
already permits SSM channels. No infrastructure change or application release is
needed. A temporary ECS task provides the tunnel to private Aurora. Production
is only read; the local restore strips ownership/grants and uses one transaction.
Temporary dumps, the tunnel, and the ECS task are cleaned up when the command ends.
