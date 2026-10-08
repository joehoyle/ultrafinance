#!/usr/bin/env bash
set -euo pipefail
exec tofu -chdir="$(cd "$(dirname "$0")" && pwd)" "$@"
