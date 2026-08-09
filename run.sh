#!/usr/bin/env bash
set -euo pipefail

# APP_ENV values:
#   test         - PostgreSQL (keystone_test), fast parallel tests, needs a local Postgres
#   development  - PostgreSQL (keystone), local storage, needs a local Postgres
#   production   - requires KEYSTONE__DATABASE__URL (PostgreSQL), proper JWT secret, real storage

export APP_ENV="${APP_ENV:-development}"
export SERVER_HOST="${SERVER_HOST:-0.0.0.0}"
export SERVER_PORT="${SERVER_PORT:-3000}"

# Derive database from POSTGRES_* (same as the app) if not explicitly set.
# KEYSTONE__DATABASE__URL is what the app reads (config crate); DATABASE_URL is
# also exported for the sqlx CLI (Makefile migrate target).
PG_USER="${POSTGRES_USER:-keystone}"
PG_PASSWORD="${POSTGRES_PASSWORD:-keystone}"
PG_DB="${POSTGRES_DB:-keystone}"
PG_PORT="${POSTGRES_PORT:-5432}"
if [ -z "${KEYSTONE__DATABASE__URL:-}" ]; then
    case "$APP_ENV" in
        test)
            KEYSTONE__DATABASE__URL="postgres://$PG_USER:$PG_PASSWORD@localhost:$PG_PORT/keystone_test"
            echo "[db] test mode: postgres://$PG_USER:****@localhost:$PG_PORT/keystone_test"
            ;;
        development)
            KEYSTONE__DATABASE__URL="postgres://$PG_USER:$PG_PASSWORD@localhost:$PG_PORT/$PG_DB"
            echo "[db] development: postgres://$PG_USER:****@localhost:$PG_PORT/$PG_DB"
            ;;
        production)
            echo "[error] production requires KEYSTONE__DATABASE__URL to be set"
            echo "  export KEYSTONE__DATABASE__URL=postgres://user:pass@host:5432/keystone"
            exit 1
            ;;
        *)
            echo "[error] unknown APP_ENV='$APP_ENV'"
            echo "  valid values: test, development, production"
            exit 1
            ;;
    esac
    export KEYSTONE__DATABASE__URL
fi
export DATABASE_URL="$KEYSTONE__DATABASE__URL"

# JWT secret check — mirrors the in-app production guard in src/main.rs.
# Reads .env (the app loads it via dotenvy too) so the check is not bypassed
# when JWT_SECRET only lives in .env.
JWT_SECRET_VAL="${JWT_SECRET:-$(grep -E '^JWT_SECRET=' .env 2>/dev/null | tail -n1 | cut -d= -f2-)}"
if [ "$APP_ENV" = "production" ] && { [ -z "$JWT_SECRET_VAL" ] \
    || printf '%s' "$JWT_SECRET_VAL" | grep -q 'change-me' \
    || [ "${#JWT_SECRET_VAL}" -lt 32 ]; }; then
    echo "[error] JWT_SECRET must be set to a random value of at least 32 bytes for production"
    echo "  generate one with: openssl rand -base64 32"
    exit 1
fi

ENCRYPTION_TOKEN_VAL="${ENCRYPTION_TOKEN:-$(grep -E '^ENCRYPTION_TOKEN=' .env 2>/dev/null | tail -n1 | cut -d= -f2-)}"
if [ "$APP_ENV" = "production" ] && { [ -z "$ENCRYPTION_TOKEN_VAL" ] \
    || printf '%s' "$ENCRYPTION_TOKEN_VAL" | grep -q 'change-me' \
    || [ "${#ENCRYPTION_TOKEN_VAL}" -lt 16 ]; }; then
    echo "[error] ENCRYPTION_TOKEN must be set to a random value of at least 16 bytes for production"
    echo "  generate one with: openssl rand -base64 32"
    exit 1
fi

echo "============================================"
echo "  Keystone"
echo "============================================"
echo "  Mode:        $APP_ENV"
echo "  Listening:   http://$SERVER_HOST:$SERVER_PORT"
echo "  Database:    ${KEYSTONE__DATABASE__URL%%@*}@****"
echo "  Storage:     ${STORAGE_DEFAULT_BACKEND:-local}"
echo "============================================"
echo ""

exec cargo run --bin keystone
