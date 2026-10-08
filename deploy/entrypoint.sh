#!/bin/sh
set -eu
# Lambda's image filesystem is read-only. Each instance gets its own working copy.
cp /opt/ultrafinance/merchants.sqlite "$ULTRAFINANCE_DB"
exec /usr/local/bin/ultrafinance-api
