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
Lambda runs an ARM64 Rust image from ECR. SQLite is a read-only catalog bundled
into the image and copied into each instance's `/tmp` directory.

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
