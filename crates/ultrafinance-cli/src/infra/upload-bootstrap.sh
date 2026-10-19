set +x
set -euo pipefail
umask 077
printf 'UF_UPLOAD_STAGE:bootstrap\n'
fail() { printf 'UF_UPLOAD_ERROR:%s\n' "$1"; exit 1; }
for tool in curl python3 pg_restore psql vacuumdb sha256sum; do
    command -v "$tool" >/dev/null 2>&1 || fail "missing-$tool"
done
[[ $(pg_restore --version) == 'pg_restore (PostgreSQL) 17.'* ]] || fail postgres-version
curl -fsS --retry 3 "$script_url" -o "$script_path" 2>/dev/null || fail bootstrap-download
printf '%s  %s\n' "$script_hash" "$script_path" | sha256sum -c --status || fail bootstrap-checksum
/bin/bash "$script_path" || fail remote-script
printf '\n%s\n' "$success_marker"
