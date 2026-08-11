#!/usr/bin/env bash
#
# docker-run.sh - build, run and update Keystone in Docker.
#
#   First run : builds the image and starts the server.
#   Re-run    : rebuilds the image only when sources changed (Docker layer
#               cache), then recreates the container with the new image.
#               Your data lives in Docker volumes (storagedata, pgdata) — or in
#               the folder you chose with DATA_HOST_PATH in .env (e.g. ./data) —
#               so it survives every update.
#
# Storage on the host is controlled by DATA_HOST_PATH in .env:
#   - empty        -> Docker named volume "storagedata" (default)
#   - ./data       -> a real folder inside this project (files land in ./data)
#   - /some/abs/path -> bind mount to that exact host path
# The server itself always sees /mnt/keystone/data (STORAGE_LOCAL_PATHS).
#
# Usage:
#   ./docker-run.sh            run or update the server (default)
#   ./docker-run.sh logs       follow container logs
#   ./docker-run.sh status     show container status
#   ./docker-run.sh stop       stop the server (data kept)
#   ./docker-run.sh down       stop and remove the container (data kept)
#   ./docker-run.sh rebuild    force a full rebuild (no cache) and restart
#   ./docker-run.sh reset      remove container AND volumes (DESTROYS DATA)
#   ./docker-run.sh db-reset   delete ALL database data, re-run migrations (fresh schema)
#   ./docker-run.sh purge      remove container AND its images (data kept)
#   ./docker-run.sh help       show this help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

COMPOSE_FILE="docker/docker-compose.yml"
PROJECT="keystone"

help_text() {
  cat <<'EOF'
docker-run.sh - build, run and update Keystone in Docker.

  First run : builds the image and starts the server.
  Re-run    : rebuilds the image if sources changed (Docker layer cache),
              then recreates the container. Data persists in Docker volumes.

Usage:
  ./docker-run.sh            run or update the server (default)
  ./docker-run.sh logs       follow container logs
  ./docker-run.sh status     show container status
  ./docker-run.sh stop       stop the server (data kept)
  ./docker-run.sh down       stop and remove the container (data kept)
  ./docker-run.sh rebuild    force a full rebuild (no cache) and restart
  ./docker-run.sh reset      remove container AND volumes (DESTROYS DATA)
  ./docker-run.sh db-reset   delete ALL database data, re-run migrations (fresh schema)
  ./docker-run.sh purge      remove container AND its images (data kept)
  ./docker-run.sh help       show this help
EOF
}

# --- pick a compose binary ------------------------------------------------
if docker compose version >/dev/null 2>&1; then
  COMPOSE_BIN=(docker compose)
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE_BIN=(docker-compose)
else
  echo "error: neither 'docker compose' nor 'docker-compose' found" >&2
  echo "       install Docker: https://docs.docker.com/engine/install/" >&2
  exit 1
fi

compose() { "${COMPOSE_BIN[@]}" -p "$PROJECT" --env-file .env -f "$COMPOSE_FILE" "$@"; }

# DATA_HOST_PATH decides where files live on the host (outside the container).
# Anchoring relative values (like ./data) to the project root makes them behave
# the same no matter how compose resolves relative paths, so ./data always means
# "<this project>/data" — never docker/data.
# .env is only read by compose itself, so load the value here first.
if [[ -z "${DATA_HOST_PATH:-}" && -f .env ]]; then
  DATA_HOST_PATH="$(sed -n 's/^DATA_HOST_PATH=//p' .env | tail -n1)"
fi
if [[ -n "${DATA_HOST_PATH:-}" && "${DATA_HOST_PATH:0:1}" != "/" ]]; then
  export DATA_HOST_PATH="$(pwd)/${DATA_HOST_PATH}"
fi

# Same for SERVER_PORT: the published host port (default 3000). .env is only
# read by compose, so read it here to print the real URL after starting.
if [[ -z "${SERVER_PORT:-}" && -f .env ]]; then
  SERVER_PORT="$(sed -n 's/^SERVER_PORT=//p' .env | tail -n1)"
fi
SERVER_PORT="${SERVER_PORT:-3000}"

# --- first-run setup -------------------------------------------------------
if [ ! -f .env ]; then
  cp .env.example .env
  echo "[setup] created .env from .env.example"
  echo "[setup] edit .env (JWT_SECRET, storage, etc.) and re-run ./docker-run.sh"
  echo
fi

if grep -q '^JWT_SECRET=change-me' .env 2>/dev/null; then
  echo "[warn] JWT_SECRET is still the default value."
  echo "       generate one with: openssl rand -base64 32"
  echo
fi

# --- commands --------------------------------------------------------------
cmd_up() {
  docker info >/dev/null 2>&1 || {
    echo "error: Docker daemon is not running" >&2
    exit 1
  }
  echo ">> building (cached) and starting Keystone..."
  # Builds the image when needed (only changed layers are rebuilt) and
  # recreates the container if the image changed. Volumes keep your data.
  compose up -d --build
  echo
  compose ps
  echo
  echo "============================================"
  echo "  Keystone is running: http://localhost:$SERVER_PORT"
  echo "  Run ./docker-run.sh again to update it."
  echo "  Logs: ./docker-run.sh logs"
  echo "============================================"
}

cmd_logs()   { compose logs -f; }
cmd_status() { compose ps; }
cmd_stop()   { compose stop; }
cmd_down()   { compose down; }

cmd_rebuild() {
  echo ">> full rebuild (no cache) - this can take a while..."
  compose build --no-cache
  compose up -d
  cmd_status
}

cmd_reset() {
  echo "This will remove the container AND the storage + database volumes"
  echo "(the storagedata volume, or the DATA_HOST_PATH folder if you set one,"
  echo "plus pgdata)."
  echo "ALL Keystone data will be destroyed."
  read -r -p "Type 'yes' to continue: " ans
  [ "$ans" = "yes" ] || { echo "aborted."; exit 1; }
  compose down -v
  echo "Removed. Next run starts fresh."
}

cmd_db_reset() {
  # Make sure the postgres container is running so we can wipe the database.
  if ! docker ps --format '{{.Names}}' | grep -qx 'keystone-postgres'; then
    echo "error: the 'keystone-postgres' container is not running."
    echo "       start the stack first: ./docker-run.sh" >&2
    exit 1
  fi

  # Read POSTGRES_USER/POSTGRES_DB from .env unless already exported.
  PGUSER="${POSTGRES_USER:-$(grep -E '^POSTGRES_USER=' .env | tail -1 | cut -d= -f2)}"
  PGUSER="${PGUSER:-keystone}"
  PGDATABASE="${POSTGRES_DB:-$(grep -E '^POSTGRES_DB=' .env | tail -1 | cut -d= -f2)}"
  PGDATABASE="${PGDATABASE:-keystone}"

  echo "This will DELETE ALL DATA in the '$PGDATABASE' database:"
  echo "users, files, folders, api keys, buckets, groups and admin settings."
  echo "Uploaded files on disk (the storagedata volume, or your"
  echo "DATA_HOST_PATH folder if you set one) are kept."
  read -r -p "Type 'yes' to continue: " ans
  [ "$ans" = "yes" ] || { echo "aborted."; exit 1; }

  # Stop the server first so it cannot write while the DB is wiped.
  compose stop server

  echo ">> dropping schema in database '$PGDATABASE'..."
  if ! compose exec -T postgres psql -v ON_ERROR_STOP=1 -U "$PGUSER" -d "$PGDATABASE" \
      -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"; then
    echo "error: failed to reset the database" >&2
    compose start server >/dev/null 2>&1 || true
    exit 1
  fi

  echo ">> starting server - migrations rebuild the schema from scratch..."
  compose up -d server
  echo
  compose ps
  echo
  echo "============================================"
  echo "  Database reset. Keystone restarted: http://localhost:$SERVER_PORT"
  echo "  Go to /setup to create your first admin user."
  echo "============================================"
}

cmd_purge() {
  echo "This removes the Keystone container and its Docker images."
  echo "Your data (storagedata volume, or your DATA_HOST_PATH folder if you set"
  echo "one, plus pgdata) is kept."
  echo "The next run will rebuild the image from scratch (takes minutes)."
  read -r -p "Type 'yes' to continue: " ans
  [ "$ans" = "yes" ] || { echo "aborted."; exit 1; }
  compose down --rmi all
  # Remove dangling intermediate images left behind by the multi-stage build
  docker image prune -f
  echo "Container and images removed. Run ./docker-run.sh to rebuild."
}

# --- dispatch --------------------------------------------------------------
case "${1:-run}" in
  run|start|up|update) cmd_up ;;
  logs)                cmd_logs ;;
  status|ps)           cmd_status ;;
  stop)                cmd_stop ;;
  down)                cmd_down ;;
  rebuild)             cmd_rebuild ;;
  reset)               cmd_reset ;;
  db-reset|reset-db)   cmd_db_reset ;;
  purge)               cmd_purge ;;
  help|-h|--help)      help_text ;;
  *)
    echo "unknown command: $1" >&2
    echo >&2
    help_text >&2
    exit 1
    ;;
esac
