#!/bin/bash
# shellcheck disable=SC2154,SC2016
# Variables job, restore_jobs and transfer_jobs come from the generated header.
# Downloaded bootstrap and archive were generated locally, with per-file SHA-256.
# Keep diagnostics private: PostgreSQL and curl errors may contain sensitive data.
stage=preflight
export PGHOST="${ULTRAFINANCE_DATABASE_HOST:?}"
export PGUSER="${ULTRAFINANCE_ADMIN_USERNAME:?}"
export PGPASSWORD="${ULTRAFINANCE_ADMIN_PASSWORD:?}"
export PGDATABASE="${ULTRAFINANCE_DATABASE_NAME:?}"
export PGSSLMODE=verify-full
export PGSSLROOTCERT=/etc/ssl/certs/ca-certificates.crt
export PGCONNECT_TIMEOUT=35
export PGOPTIONS='-c statement_timeout=0 -c lock_timeout=30000 -c idle_session_timeout=0'
unset PGSERVICE
original=$PGDATABASE
staging="uf_upload_$job"
old="uf_old_$job"
production_blocked=0
staging_created=0
reporter=
step() { stage=$1; printf 'UF_UPLOAD_STAGE:%s\n' "$stage"; }
fail() { printf 'UF_UPLOAD_ERROR:%s\n' "$1"; exit 1; }
psql_safe() { psql --no-password -Xq -v ON_ERROR_STOP=1 "$@"; }
cleanup() {
    result=$?
    trap - EXIT
    set +e
    if [[ -n $reporter ]]; then kill "$reporter" 2>/dev/null; wait "$reporter" 2>/dev/null; fi
    if ((production_blocked)); then
        # If either cutover transaction failed, reconnect the original name.
        # Transactional renames ensure this is always either the old or new DB.
        psql_safe --dbname=postgres -v database="$original" > /dev/null 2>&1 <<'SQL'
SELECT format('ALTER DATABASE %I ALLOW_CONNECTIONS true',datname)
FROM pg_database WHERE datname=:'database'
\gexec
SQL
    fi
    if ((staging_created)); then
        psql_safe --dbname=postgres -v staging="$staging" > /dev/null 2>&1 <<'SQL'
SELECT format('DROP DATABASE %I WITH (FORCE)',datname)
FROM pg_database WHERE datname=:'staging'
\gexec
SQL
    fi
    if ((result != 0)); then
        printf 'UF_UPLOAD_FAILED_STAGE:%s\n' "$stage"
    fi
    exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP
# Redirect all tool diagnostics to scratch, never the ECS console. The launcher
# prints only the fixed stage labels and requires a final success marker.
exec 2>diagnostics.log
step preflight
[[ $(pg_restore --version) == 'pg_restore (PostgreSQL) 17.'* ]]
# The selected candidate may require a newer schema than production currently
# has. Check connectivity here; validate the restored staging database below.
psql_safe -At -c 'SELECT 1' > /dev/null || fail database-connectivity
# Limit archive workers to connection headroom; don't oversubscribe small Aurora.
headroom=$(psql_safe -At -c "SELECT greatest(0,current_setting('max_connections')::int-(SELECT count(*) FROM pg_stat_activity)::int-16)") || fail connection-headroom
if ((restore_jobs == 0)); then
    restore_jobs=$(( $(nproc) * 2 ))
    ((restore_jobs <= 32)) || restore_jobs=32
    ((restore_jobs <= headroom)) || restore_jobs=$headroom
fi
((restore_jobs > 0 && restore_jobs <= headroom)) || fail restore-capacity
step download
mkdir -p verified
python3 progress.py download &
reporter=$!
# xargs waits for all workers and fails if any download/checksum fails.
xargs -d '\n' -P "$transfer_jobs" -I '{}' /bin/bash -c '
    IFS=$'"'"'\t'"'"' read -r name hash size url <<< "$1"
    curl -fsS --retry 3 "$url" -o "dump/$name" &&
    printf "%s  %s\n" "$hash" "dump/$name" | sha256sum -c --status &&
    touch "verified/$name"
' download '{}' < downloads > /dev/null
kill "$reporter" 2>/dev/null || true
wait "$reporter" 2>/dev/null || true
reporter=
python3 - <<'PY_PROGRESS'
from pathlib import Path
from progress import download_position
position, total = download_position(Path("downloads"), Path("dump"), Path("verified"))
print(f"UF_UPLOAD_PROGRESS:download:{position}:{total}:bytes", flush=True)
PY_PROGRESS
pg_restore --list dump > toc.list

# Preserve the existing production roles and grants, rather than importing local
# ACLs or assuming that the Lambda application connects as the administrator.
psql_safe -At -v staging="$staging" > database.sql <<'SQL'
SELECT format('CREATE DATABASE %I TEMPLATE template0 OWNER %I ENCODING %L LC_COLLATE %L LC_CTYPE %L;',
 :'staging',pg_get_userbyid(datdba),pg_encoding_to_char(encoding),datcollate,datctype)
FROM pg_database WHERE datname=current_database() AND datlocprovider='c';
SQL
[[ -s database.sql ]]
psql_safe -At -v staging="$staging" > database-access.sql <<'SQL'
SELECT format('REVOKE ALL ON DATABASE %I FROM PUBLIC;',:'staging');
SELECT format('GRANT %s ON DATABASE %I TO %s%s;',a.privilege_type,:'staging',
 CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END,
 CASE WHEN a.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END)
FROM pg_database d CROSS JOIN LATERAL aclexplode(coalesce(d.datacl,acldefault('d',d.datdba))) a
WHERE d.datname=current_database();
SELECT format('ALTER DATABASE %I CONNECTION LIMIT %s;',:'staging',datconnlimit)
FROM pg_database WHERE datname=current_database();
SELECT CASE WHEN s.setrole=0
 THEN format('ALTER DATABASE %I SET %I TO %L;',:'staging',split_part(c,'=',1),substr(c,strpos(c,'=')+1))
 ELSE format('ALTER ROLE %I IN DATABASE %I SET %I TO %L;',pg_get_userbyid(s.setrole),:'staging',split_part(c,'=',1),substr(c,strpos(c,'=')+1)) END
FROM pg_db_role_setting s CROSS JOIN LATERAL unnest(s.setconfig) c
WHERE s.setdatabase=(SELECT oid FROM pg_database WHERE datname=current_database());
SQL
psql_safe -At > object-access.sql <<'SQL'
SET search_path=pg_catalog;
SELECT format('REVOKE ALL ON SCHEMA %I FROM PUBLIC; REVOKE ALL ON ALL TABLES IN SCHEMA %I FROM PUBLIC; REVOKE ALL ON ALL SEQUENCES IN SCHEMA %I FROM PUBLIC; REVOKE ALL ON ALL FUNCTIONS IN SCHEMA %I FROM PUBLIC;',nspname,nspname,nspname,nspname)
FROM pg_namespace WHERE nspname !~ '^pg_' AND nspname <> 'information_schema';
SELECT format('GRANT %s ON SCHEMA %I TO %s%s;',a.privilege_type,n.nspname,
 CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END,
 CASE WHEN a.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END)
FROM pg_namespace n CROSS JOIN LATERAL aclexplode(coalesce(n.nspacl,acldefault('n',n.nspowner))) a
WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema';
SELECT format('GRANT %s ON %s %I.%I TO %s%s;',a.privilege_type,
 CASE WHEN c.relkind='S' THEN 'SEQUENCE' ELSE 'TABLE' END,n.nspname,c.relname,
 CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END,
 CASE WHEN a.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END)
FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
CROSS JOIN LATERAL aclexplode(coalesce(c.relacl,acldefault(CASE WHEN c.relkind='S' THEN 'S'::"char" ELSE 'r'::"char" END,c.relowner))) a
WHERE c.relkind IN ('r','p','v','m','S','f') AND n.nspname !~ '^pg_' AND n.nspname <> 'information_schema';
SELECT format('GRANT %s ON %s %s TO %s%s;',a.privilege_type,
 CASE WHEN p.prokind='p' THEN 'PROCEDURE' ELSE 'FUNCTION' END,p.oid::regprocedure,
 CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END,
 CASE WHEN a.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END)
FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace
CROSS JOIN LATERAL aclexplode(coalesce(p.proacl,acldefault('f',p.proowner))) a
WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema';
SELECT format('GRANT %s (%I) ON TABLE %I.%I TO %s%s;',a.privilege_type,col.attname,n.nspname,c.relname,
 CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END,
 CASE WHEN a.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END)
FROM pg_attribute col JOIN pg_class c ON c.oid=col.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace
CROSS JOIN LATERAL aclexplode(col.attacl) a
WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema';
SELECT format('REVOKE ALL ON TYPE %I.%I FROM PUBLIC;',n.nspname,t.typname)
FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace
WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' AND t.typtype IN ('d','e','r','m');
SELECT format('GRANT %s ON TYPE %I.%I TO %s%s;',a.privilege_type,n.nspname,t.typname,
 CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END,
 CASE WHEN a.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END)
FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace
CROSS JOIN LATERAL aclexplode(coalesce(t.typacl,acldefault('T',t.typowner))) a
WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' AND t.typtype IN ('d','e','r','m');
SELECT format('ALTER DEFAULT PRIVILEGES FOR ROLE %I %s REVOKE ALL ON %s FROM PUBLIC;',pg_get_userbyid(d.defaclrole),
 CASE WHEN d.defaclnamespace=0 THEN '' ELSE format('IN SCHEMA %I',n.nspname) END,
 CASE d.defaclobjtype WHEN 'r' THEN 'TABLES' WHEN 'S' THEN 'SEQUENCES' WHEN 'f' THEN 'FUNCTIONS' WHEN 'T' THEN 'TYPES' WHEN 'n' THEN 'SCHEMAS' END)
FROM pg_default_acl d LEFT JOIN pg_namespace n ON n.oid=d.defaclnamespace;
SELECT format('ALTER DEFAULT PRIVILEGES FOR ROLE %I %s GRANT %s ON %s TO %s%s;',pg_get_userbyid(d.defaclrole),
 CASE WHEN d.defaclnamespace=0 THEN '' ELSE format('IN SCHEMA %I',n.nspname) END,a.privilege_type,
 CASE d.defaclobjtype WHEN 'r' THEN 'TABLES' WHEN 'S' THEN 'SEQUENCES' WHEN 'f' THEN 'FUNCTIONS' WHEN 'T' THEN 'TYPES' WHEN 'n' THEN 'SCHEMAS' END,
 CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END,
 CASE WHEN a.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END)
FROM pg_default_acl d LEFT JOIN pg_namespace n ON n.oid=d.defaclnamespace
CROSS JOIN LATERAL aclexplode(d.defaclacl) a;
SQL
# Creation is separate from restore so indexes are built after COPY finishes.
psql_safe --dbname=postgres -f database.sql > /dev/null
staging_created=1
export PGDATABASE=$staging
step restore
: > restore.log
python3 progress.py restore &
reporter=$!
LC_ALL=C pg_restore --no-password --dbname="$staging" --jobs="$restore_jobs" \
    --no-owner --no-privileges --exit-on-error --verbose dump > /dev/null 2>restore.log
kill "$reporter" 2>/dev/null || true
wait "$reporter" 2>/dev/null || true
reporter=
python3 - <<'PY_PROGRESS'
from pathlib import Path
from progress import restore_position
_, total = restore_position(Path("restore.log"), Path("toc.list"))
print(f"UF_UPLOAD_PROGRESS:restore:{total}:{total}:items", flush=True)
PY_PROGRESS
step permissions
psql_safe -f object-access.sql > /dev/null
psql_safe -f database-access.sql > /dev/null
step analyze
vacuumdb --no-password --dbname="$staging" --analyze-only --jobs="$restore_jobs" > /dev/null
step validate
ULTRAFINANCE_DATABASE_NAME="$staging" /bin/bash admin-shell.sh ultrafinance database check > /dev/null
# PostgreSQL restored and validated indexes, constraints and sequence values.
# Stop new connections before terminating old ones; otherwise Lambda can race a
# rename by reconnecting. No application credentials or Lambda config are read.
step cutover
production_blocked=1
psql_safe --dbname=postgres -v database="$original" -v staging="$staging" -v old="$old" > /dev/null <<'SQL'
SELECT pg_advisory_lock(8463227049951709550);
BEGIN;
ALTER DATABASE :"database" ALLOW_CONNECTIONS false;
ALTER DATABASE :"staging" ALLOW_CONNECTIONS false;
COMMIT;
SELECT pg_terminate_backend(pid,10000) FROM pg_stat_activity
WHERE datname IN (:'database',:'staging') AND pid<>pg_backend_pid();
BEGIN;
ALTER DATABASE :"database" RENAME TO :"old";
ALTER DATABASE :"staging" RENAME TO :"database";
ALTER DATABASE :"database" ALLOW_CONNECTIONS true;
COMMIT;
SQL
production_blocked=0
staging_created=0
step delete-old
psql_safe --dbname=postgres -v old="$old" > /dev/null <<'SQL'
DROP DATABASE :"old" WITH (FORCE);
SQL
step complete
