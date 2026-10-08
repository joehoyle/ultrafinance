#!/usr/bin/env bash
set -euo pipefail
infra_directory="$(cd "$(dirname "$0")" && pwd)"
# Credential-process commands also need a region when refreshing native AWS login.
export AWS_REGION="${AWS_REGION:-$(tofu -chdir="$infra_directory" output -raw aws_region 2>/dev/null || echo ca-central-1)}"
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-$AWS_REGION}"
exec tofu -chdir="$infra_directory" "$@"
