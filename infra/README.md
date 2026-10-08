# AWS infrastructure

CloudFront serves the website and API through a public Lambda function URL.
Lambda runs the ARM64 Rust image stored in ECR. SQLite is a read-only catalog
bundled into the image and copied into each instance's `/tmp` directory.

OpenTofu owns ECR, IAM, logging, Lambda runtime configuration, the function URL,
CloudFront, Route 53, and the ACM certificate. Release tooling owns the Lambda
image and the version selected by the `live` alias.

## Local configuration

Install OpenTofu, AWS CLI, and jq. Authenticate your AWS profile before running
commands. The shell helper bridges AWS CLI login credentials to the provider;
credentials stay in process memory.

For a new checkout, copy `terraform.tfvars.example` to `terraform.tfvars` and
replace the account ID with your own. Keep the existing variable file when
working with an already deployed installation.

Set `TYPESAFE_API_KEY` in the ignored root `.env` file. The helper passes it as
the sensitive OpenTofu input `typesafe_api_key`. An explicit
`TF_VAR_typesafe_api_key` takes precedence, followed by an exported
`TYPESAFE_API_KEY`, then `.env`.

From the repository root:

```sh
./infra/tofu.sh init
./infra/tofu.sh plan
./infra/tofu.sh apply
./infra/tofu.sh output -raw site_url
```

Use `./infra/tofu.sh --profile PROFILE plan` to select another AWS profile.
AWS profile and region default to `joehoyle` and `ca-central-1`.

## Bootstrap and releases

With `image_uri = null`, the first apply creates ECR and the GitHub OIDC role.
Build and push an image using the Dockerfile, then set `image_uri` to its immutable
ECR digest and apply again to create the application. Keep this value configured:
clearing it would plan deletion of the application resources.

Routine image releases use `./deploy/deploy.sh` or the GitHub Actions workflow.
They test the new Lambda version before moving `live`, with guarded rollback.
A runtime configuration apply publishes a version but does not promote it;
run a release afterwards to make the configuration live.

GitHub uses OIDC restricted to the configured repository's `main` branch.
Configure repository variables `AWS_DEPLOY_ROLE_ARN`, `ECR_REPOSITORY`,
`AWS_REGION`, and `LAMBDA_FUNCTION_NAME` using the corresponding OpenTofu outputs.
The CI role cannot change IAM or Lambda environment configuration.

## Domain

Set `domain_name` to the existing public Route 53 zone's name, then import that
zone before applying:

```sh
./infra/tofu.sh import 'aws_route53_zone.site[0]' YOUR_HOSTED_ZONE_ID
./infra/tofu.sh plan
./infra/tofu.sh apply
```

The configuration protects the zone against deletion, preserves unmanaged
records, and adds apex A/AAAA aliases to CloudFront. ACM DNS validation uses a
certificate in `us-east-1`. Lambda stays in the configured application region.

## Private local files

`terraform.tfstate` and its backup are the records of the live deployment.
They contain the provider key. Keep them private and backed up; never delete
state as part of a directory cleanup. `terraform.tfvars`, `.env`, saved plans,
the provider lock file, and the generated `.terraform` directory are ignored.
The provider version is pinned in `versions.tf`. Run `init` again after removing
the provider cache. No remote state backend is configured yet.

Check infrastructure configuration with:

```sh
tofu -chdir=infra fmt -check -recursive
tofu -chdir=infra validate
tofu -chdir=infra test
shellcheck infra/tofu.sh
```
