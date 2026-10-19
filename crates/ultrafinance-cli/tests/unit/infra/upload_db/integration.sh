#!/bin/bash
# Isolated real PostgreSQL 17 test; no AWS calls and no host database writes.
set -euo pipefail
root=$(cd "$(dirname "$0")/../../../../../.." && pwd)
container="uf-upload-test-$(date +%s)-$$"
fixtures=$(mktemp -d)
cleanup() {
    result=$?
    if ((result != 0)); then docker logs --tail 80 "$container" >&2 || true; fi
    docker rm -f "$container" >/dev/null 2>&1 || true
    rm -rf "$fixtures"
}
trap cleanup EXIT
# An init process reaps orphaned test clients instead of PostgreSQL PID 1.
docker run --init -d --name "$container" -e POSTGRES_HOST_AUTH_METHOD=trust postgres:17 >/dev/null
for ((attempt=0; attempt<60; attempt++)); do
    if docker exec "$container" pg_isready -U postgres >/dev/null 2>&1; then break; fi
    sleep 1
done
docker exec "$container" bash -c "apt-get update -qq && apt-get install -y -qq --no-install-recommends python3 >/dev/null"
# All fixtures live in this disposable server, including the production stand-in.
docker exec -i "$container" psql -U postgres -v ON_ERROR_STOP=1 >/dev/null <<'SQL'
CREATE ROLE app LOGIN;
CREATE ROLE reporter LOGIN;
CREATE DATABASE source TEMPLATE template0 LC_COLLATE 'C' LC_CTYPE 'C';
CREATE DATABASE production TEMPLATE template0 LC_COLLATE 'C' LC_CTYPE 'C';
\c source
CREATE EXTENSION pg_trgm;
CREATE TYPE private_code AS ENUM ('secret');
CREATE TABLE ultrafinance_schema(version int);
INSERT INTO ultrafinance_schema VALUES (20);
CREATE TABLE merchants(id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, name text NOT NULL);
INSERT INTO merchants(name) SELECT 'new-'||g FROM generate_series(1,25000) g;
CREATE FUNCTION private_name() RETURNS text LANGUAGE sql AS $$SELECT 'private'::text$$;
\c production
CREATE EXTENSION pg_trgm;
CREATE TYPE private_code AS ENUM ('secret');
CREATE TABLE ultrafinance_schema(version int);
INSERT INTO ultrafinance_schema VALUES (19);
CREATE TABLE merchants(id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, name text NOT NULL);
INSERT INTO merchants(name) VALUES ('old');
CREATE FUNCTION private_name() RETURNS text LANGUAGE sql AS $$SELECT 'private'::text$$;
REVOKE ALL ON FUNCTION private_name() FROM PUBLIC;
REVOKE ALL ON TYPE private_code FROM PUBLIC;
GRANT USAGE ON TYPE private_code TO app;
GRANT SELECT(name) ON merchants TO reporter;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO reporter;
GRANT USAGE ON SCHEMA public TO app;
GRANT SELECT,INSERT ON merchants TO app;
GRANT USAGE ON SEQUENCE merchants_id_seq TO app;
ALTER ROLE app IN DATABASE production SET statement_timeout='12s';
SQL
docker exec "$container" mkdir -p /fixture /scratch /shims
# Transport is stubbed; archive COPY, index build, ACL handling, ANALYZE and
# database cutover all use real PostgreSQL processes.
docker exec "$container" pg_dump -U postgres -d source -Fd -j 4 --compress="${UF_TEST_COMPRESSION:-zstd:1}" --no-owner --no-privileges -f /fixture/dump
cat > "$fixtures/curl" <<'SH'
#!/bin/bash
set -euo pipefail
while (($#)); do
    case "$1" in
        -o) destination=$2; shift 2;;
        --retry) shift 2;;
        -*) shift;;
        *) source=$1; shift;;
    esac
done
cp "/fixture/dump/$source" "$destination"
if [[ -f /fixture/corrupt-download ]]; then printf corrupt >> "$destination"; fi
SH
for program in psql pg_restore vacuumdb; do
    printf '#!/bin/bash\nexport PGSSLMODE=disable\nexec /usr/lib/postgresql/17/bin/%s "$@"\n' "$program" > "$fixtures/$program"
done
cat > "$fixtures/ultrafinance" <<'SH'
#!/bin/bash
set -euo pipefail
if [[ $ULTRAFINANCE_DATABASE_NAME == uf_upload_* && -f /fixture/fail-validation ]]; then exit 1; fi
[[ $(psql -U postgres -d "$ULTRAFINANCE_DATABASE_NAME" -Atc 'SELECT version FROM ultrafinance_schema') == 20 ]]
SH
chmod +x "$fixtures/"*
docker cp "$fixtures/." "$container:/shims/"
docker cp "$root/crates/ultrafinance-cli/src/infra/admin-shell.sh" "$container:/fixture/admin-shell.sh"
docker cp "$root/crates/ultrafinance-cli/src/infra/upload-progress.py" "$container:/fixture/progress.py"
docker cp "$root/crates/ultrafinance-cli/src/infra/upload-db.sh" "$container:/fixture/body.sh"
# Generate the same nonsecret header that the Rust launcher produces.
docker exec -i "$container" bash > /dev/null <<'SH'
set -euo pipefail
cat > /fixture/run.sh <<'HEADER'
#!/bin/bash
set -Eeuo pipefail
umask 077
job=integration
restore_jobs=4
transfer_jobs=4
mkdir -p /scratch/"$job"/dump
cd /scratch/"$job"
cp /fixture/admin-shell.sh .
cp /fixture/progress.py .
: > downloads
for file in /fixture/dump/*; do
    name=${file##*/}
    hash=$(sha256sum "$file" | cut -d ' ' -f 1)
    size=$(stat -c %s "$file")
    printf '%s\t%s\t%s\t%s\n' "$name" "$hash" "$size" "$name" >> downloads
done
HEADER
cat /fixture/body.sh >> /fixture/run.sh
SH
run_upload() {
    docker exec -e PATH=/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
        -e ULTRAFINANCE_DATABASE_HOST=127.0.0.1 -e ULTRAFINANCE_DATABASE_NAME=production \
        -e ULTRAFINANCE_ADMIN_USERNAME=postgres -e ULTRAFINANCE_ADMIN_PASSWORD=unused \
        "$container" bash /fixture/run.sh
}
# A checksum failure must not create a database or modify production.
docker exec "$container" touch /fixture/corrupt-download
if run_upload; then echo 'Expected checksum failure' >&2; exit 1; fi
[[ $(docker exec "$container" psql -U postgres -d production -Atc 'SELECT name FROM merchants') == old ]]
docker exec "$container" rm /fixture/corrupt-download
# A validation failure must leave production's contents and connectivity intact.
docker exec "$container" touch /fixture/fail-validation
if run_upload; then echo 'Expected validation failure' >&2; exit 1; fi
[[ $(docker exec "$container" psql -U postgres -d production -Atc 'SELECT name FROM merchants') == old ]]
[[ $(docker exec "$container" psql -U postgres -Atc "SELECT count(*) FROM pg_database WHERE datname='uf_upload_integration'") == 0 ]]
docker exec "$container" rm /fixture/fail-validation
# A name collision forces failure after connections were disabled. The EXIT
# handler must reconnect the original database and discard staging.
docker exec "$container" createdb -U postgres uf_old_integration
if run_upload; then echo 'Expected cutover failure' >&2; exit 1; fi
[[ $(docker exec "$container" psql -U postgres -d production -Atc 'SELECT name FROM merchants') == old ]]
docker exec "$container" dropdb -U postgres uf_old_integration
# Hold an application connection open to verify termination before rename.
docker exec "$container" bash -c 'psql -U app -d production -c "SELECT pg_sleep(300)" >/tmp/app-session.log 2>&1 &'
# Exercise a successful replacement and deletion of the original DB.
if ! run_upload; then
    docker exec "$container" cat /scratch/integration/diagnostics.log >&2
    exit 1
fi
[[ $(docker exec "$container" psql -U postgres -d production -Atc 'SELECT count(*) FROM merchants') == 25000 ]]
[[ $(docker exec "$container" psql -U postgres -Atc "SELECT count(*) FROM pg_database WHERE datname IN ('uf_upload_integration','uf_old_integration')") == 0 ]]
[[ $(docker exec "$container" psql -U app -d production -Atc "INSERT INTO merchants(name) VALUES ('app-write') RETURNING id") == $'25001\nINSERT 0 1' ]]
[[ $(docker exec "$container" psql -U app -d production -Atc 'SHOW statement_timeout') == 12s ]]
[[ $(docker exec "$container" psql -U postgres -d production -Atc "SELECT has_function_privilege('app','private_name()','EXECUTE')") == f ]]

[[ $(docker exec "$container" psql -U postgres -d production -Atc "SELECT has_column_privilege('reporter','merchants','name','SELECT') AND NOT has_column_privilege('reporter','merchants','id','SELECT')") == t ]]
[[ $(docker exec "$container" psql -U postgres -d production -Atc "SELECT has_type_privilege('app','private_code','USAGE') AND NOT has_type_privilege('reporter','private_code','USAGE')") == t ]]
docker exec "$container" psql -U postgres -d production -c 'CREATE TABLE future_table(id int)' >/dev/null
[[ $(docker exec "$container" psql -U postgres -d production -Atc "SELECT has_table_privilege('reporter','future_table','SELECT')") == t ]]
echo 'PostgreSQL 17 integration passed: failure isolation, parallel restore, grants, sequences, role settings, connection termination and cutover.'
