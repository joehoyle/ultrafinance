#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export AWS_PAGER=""
AWS_PROFILE="${AWS_PROFILE:-$(tofu -chdir=infra output -raw aws_profile)}"
AWS_REGION="${AWS_REGION:-$(tofu -chdir=infra output -raw aws_region)}"
LAMBDA_FUNCTION_NAME="${LAMBDA_FUNCTION_NAME:-$(tofu -chdir=infra output -raw function_name)}"
export AWS_PROFILE AWS_REGION LAMBDA_FUNCTION_NAME
if [[ -z "$LAMBDA_FUNCTION_NAME" || "$LAMBDA_FUNCTION_NAME" == "null" ]]; then
  echo "Bootstrap the Lambda application with OpenTofu before deploying releases." >&2
  exit 1
fi
repository=$(tofu -chdir=infra output -raw repository_url)
registry=${repository%%/*}
repository_name=${repository#*/}
# Unique even for repeated builds of the same commit or uncommitted changes.
release="$(date -u +%Y%m%dT%H%M%SZ)-$(python3 -c 'import uuid; print(uuid.uuid4().hex[:12])')"
aws ecr get-login-password | docker login --username AWS --password-stdin "$registry"
docker buildx build --platform linux/arm64 --provenance=false \
  --tag "$repository:$release" --push .
digest=$(aws ecr describe-images --repository-name "$repository_name" \
  --image-ids "imageTag=$release" --query 'imageDetails[0].imageDigest' --output text)
python3 deploy/release.py "$repository@$digest"
