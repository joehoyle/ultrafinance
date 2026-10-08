#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
case "${1:-up}" in
  up)
    docker compose up -d --wait postgres
    cargo run --locked -- --database-url "postgresql://ultrafinance@127.0.0.1:55432/ultrafinance?sslmode=disable" database init
    ;;
  down)
    docker compose stop postgres
    ;;
  test)
    docker compose up -d --wait postgres
    ULTRAFINANCE_TEST_DATABASE_URL="postgresql://ultrafinance@127.0.0.1:55432/ultrafinance?sslmode=disable" cargo test --locked --workspace
    ;;
  *)
    echo "Usage: dev/postgres.sh [up|down|test]" >&2
    exit 2
    ;;
esac
