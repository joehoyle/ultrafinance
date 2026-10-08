#!/usr/bin/env bash
# Run OpenTofu with AWS CLI login credentials and the local Jev key.
set +x
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
profile=${AWS_PROFILE:-joehoyle}
if [[ ${1:-} == --profile ]]; then
  profile=${2:?Provide an AWS profile}
  shift 2
fi
if [[ $# == 0 ]]; then
  echo 'Usage: ./infra/tofu.sh [--profile PROFILE] COMMAND [ARGS...]' >&2
  exit 2
fi

# Explicit environment values take precedence over the ignored local .env.
if [[ ! ${TF_VAR_typesafe_api_key+x} ]]; then
  if [[ ! ${TYPESAFE_API_KEY+x} && -f "$root/.env" ]]; then
    # shellcheck disable=SC1091
    source "$root/.env"
    set +x
  fi
  if [[ ${TYPESAFE_API_KEY+x} ]]; then
    export TF_VAR_typesafe_api_key=$TYPESAFE_API_KEY
  fi
fi

# Capture credentials in memory. Never echo them or write credential files.
credentials=$(aws configure export-credentials --profile "$profile" --format process)
AWS_ACCESS_KEY_ID=$(jq -er '.AccessKeyId' <<< "$credentials")
AWS_SECRET_ACCESS_KEY=$(jq -er '.SecretAccessKey' <<< "$credentials")
AWS_SESSION_TOKEN=$(jq -r '.SessionToken // ""' <<< "$credentials")
export AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN
export AWS_EC2_METADATA_DISABLED=true
unset credentials AWS_PROFILE AWS_DEFAULT_PROFILE
exec tofu -chdir="$root/infra" "$@"
