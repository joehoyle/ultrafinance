# Deployment

When the user says "deploy", deploy the current workspace using the existing
`./deploy/deploy.sh` script. This project runs at https://ultrafinance.app on
AWS Lambda behind CloudFront, with ARM64 container images in ECR. Do not use
the Human Made internal tools deployment skill or provision another platform.

OpenTofu manages infrastructure in `infra/`. Routine application releases do
not require `tofu apply`. The deploy script reads the AWS profile, region,
function name and ECR repository from the existing OpenTofu outputs, builds and
pushes the Docker image and releases its immutable digest through the Rust
`infra deploy` command. `deploy/deploy.sh` wraps that command.
The Docker build runs workspace tests and Clippy. The release script checks the
published version before promoting the `live` alias and checks again afterwards,
with guarded rollback on failure.

Run `cargo test --locked -p ultrafinance-cli infra` before deploying.
After release, verify `https://ultrafinance.app/health`, `/docs`, and
`/openapi.json`, including the expected API paths. Report the deployed version
and verification results. Never print credentials, `.env`, full OpenTofu state,
or Lambda environment configuration. If AWS authentication is expired, report
the authentication error and ask the user to log in to the configured profile.

Use `./infra/tofu.sh` for explicitly requested infrastructure changes; see
`infra/README.md`. Preserve local state and configuration files.
