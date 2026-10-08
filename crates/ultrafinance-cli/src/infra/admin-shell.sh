#!/bin/bash
# ECS injects the current RDS-managed credentials; no password enters a command or task definition.
set +x
set -e
: "${ULTRAFINANCE_ADMIN_USERNAME:?CLI administrator username missing; apply CLI infrastructure}"
: "${ULTRAFINANCE_ADMIN_PASSWORD:?CLI administrator password missing; apply CLI infrastructure}"
: "${ULTRAFINANCE_DATABASE_HOST:?CLI database host missing; apply CLI infrastructure}"
: "${ULTRAFINANCE_DATABASE_NAME:?CLI database name missing; apply CLI infrastructure}"

encode_component() {
    local LC_ALL=C
    local value="$1" encoded='' character hex ordinal index
    for ((index=0; index<${#value}; index++)); do
        character="${value:index:1}"
        case "$character" in
            [a-zA-Z0-9.~_-]) encoded+="$character" ;;
            *)
                # Older Bash versions treat UTF-8 bytes as signed; normalize to an octet.
                printf -v ordinal '%d' "'$character"
                printf -v hex '%%%02X' "$((ordinal & 255))"
                encoded+="$hex"
                ;;
        esac
    done
    printf -v "$2" '%s' "$encoded"
}
encode_component "$ULTRAFINANCE_ADMIN_USERNAME" encoded_username
encode_component "$ULTRAFINANCE_ADMIN_PASSWORD" encoded_password
encode_component "$ULTRAFINANCE_DATABASE_NAME" encoded_database
export ULTRAFINANCE_DATABASE_URL="postgresql://${encoded_username}:${encoded_password}@${ULTRAFINANCE_DATABASE_HOST}:5432/${encoded_database}?sslmode=require"
unset ULTRAFINANCE_ADMIN_USERNAME ULTRAFINANCE_ADMIN_PASSWORD encoded_username encoded_password encoded_database
unset -f encode_component
if (($#)); then
    exec "$@"
fi
exec /bin/bash --rcfile /etc/bash.bashrc -i
