#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo run --locked -- infra deploy "$@"
