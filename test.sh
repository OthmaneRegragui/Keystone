#!/usr/bin/env bash
#
# test.sh - Run Keystone integration tests inside isolated Docker containers.
#
# Spins up a throwaway PostgreSQL container, runs `cargo test` against it, and
# tears everything down afterwards (unless told to keep the DB running).
#
# Usage:
#   ./test.sh                       run tests, tear down DB when done
#   ./test.sh --keep                run tests, leave DB running
#   ./test.sh --no-cleanup          same as --keep
#   ./test.sh up                    only start the test database
#   ./test.sh down                  stop and remove the test database
#   ./test.sh -- <extra cargo args> pass extra args to cargo test
#
# Environment overrides:
#   TEST_FILTER=api_auth            only run tests matching this name
#   CARGO_EXTRA_ARGS="..."          extra args for cargo test
#   DB_START_TIMEOUT=30             seconds to wait for DB health check

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

COMPOSE_FILE="docker/docker-compose.test.yml"
PROJECT="keystone-test"
DB_PORT=5434
DB_USER="test"
DB_PASS="test"
DB_NAME="postgres"
DB_START_TIMEOUT="${DB_START_TIMEOUT:-30}"

# ── Colours (disabled when stdout is not a terminal) ────────────────────────
if [[ -t 1 ]]; then
  RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
  BLUE='\033[0;34m'; BOLD='\033[1m'; RESET='\033[0m'
else
  RED=''; GREEN=''; YELLOW=''; BLUE=''; BOLD=''; RESET=''
fi

info()  { echo -e "${BLUE}>>${RESET} $*"; }
ok()    { echo -e "${GREEN}✓${RESET} $*"; }
warn()  { echo -e "${YELLOW}!${RESET} $*" >&2; }
die()   { echo -e "${RED}error:${RESET} $*" >&2; exit 1; }

# ── Docker compose detection ────────────────────────────────────────────────
if docker compose version >/dev/null 2>&1; then
  COMPOSE=(docker compose)
elif command -v docker-compose >/dev/null 2>&1; then
  COMPOSE=(docker-compose)
else
  die "neither 'docker compose' nor 'docker-compose' found"
fi

compose() { "${COMPOSE[@]}" -p "$PROJECT" -f "$COMPOSE_FILE" "$@"; }

# ── Argument parsing ────────────────────────────────────────────────────────
ACTION="run"            # default: start db, run tests, tear down
CLEANUP=true            # remove containers after tests?
EXTRA_CARGO_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    up|start)        ACTION="up";   shift ;;
    down|stop|rm)    ACTION="down"; shift ;;
    --keep|--no-cleanup|--no-clean)
                     CLEANUP=false; shift ;;
    --cleanup|--clean)
                     CLEANUP=true;  shift ;;
    --)              shift; EXTRA_CARGO_ARGS+=("$@"); break ;;
    -h|--help)       ACTION="help"; shift ;;
    -*)              EXTRA_CARGO_ARGS+=("$1"); shift ;;
    *)               EXTRA_CARGO_ARGS+=("$1"); shift ;;
  esac
done

# ── Help ────────────────────────────────────────────────────────────────────
if [[ "$ACTION" == "help" ]]; then
  cat <<'EOF'
test.sh - Run Keystone integration tests in an isolated Docker container.

Usage:
  ./test.sh                       Run tests, tear down DB when done
  ./test.sh --keep                Run tests, leave DB running for inspection
  ./test.sh --no-cleanup          Same as --keep
  ./test.sh up                    Only start the test database
  ./test.sh down                  Stop and remove the test database
  ./test.sh -- <extra args>       Pass extra args to cargo test

Flags:
  --keep, --no-cleanup    Leave the test database running after tests
  --cleanup               Remove the test database after tests (default)
  -h, --help              Show this help

Environment:
  TEST_FILTER=pattern     Only run tests matching this name
  DB_START_TIMEOUT=30     Seconds to wait for DB to become healthy

Examples:
  ./test.sh                              # full run + cleanup
  ./test.sh --keep                       # run, keep DB up
  ./test.sh down                         # shut down test DB
  ./test.sh -- --test api_file_ops       # run specific test file
  TEST_FILTER=login ./test.sh            # only tests matching "login"
EOF
  exit 0
fi

# ── Subcommand: down ────────────────────────────────────────────────────────
if [[ "$ACTION" == "down" ]]; then
  info "Stopping test containers..."
  compose down -v 2>/dev/null || true
  ok "Test containers removed."
  exit 0
fi

# ── Preflight ───────────────────────────────────────────────────────────────
docker info >/dev/null 2>&1 || die "Docker daemon is not running"

if ! command -v cargo >/dev/null 2>&1; then
  # Try common locations
  if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
  else
    die "cargo not found. Install Rust: https://rustup.rs"
  fi
fi

# ── Cleanup trap ────────────────────────────────────────────────────────────
cleanup() {
  if [[ "$CLEANUP" == "true" ]]; then
    info "Tearing down test containers..."
    compose down -v 2>/dev/null || true
    ok "Test containers removed."
  else
    warn "Test database left running on port $DB_PORT"
    warn "Stop it with: ./test.sh down"
  fi
}

# ── Start test database ─────────────────────────────────────────────────────
info "Starting test PostgreSQL container..."
compose up -d

# Wait for healthy
info "Waiting for database to be ready (timeout: ${DB_START_TIMEOUT}s)..."
elapsed=0
while [[ $elapsed -lt $DB_START_TIMEOUT ]]; do
  # Check the container health status
  health=$(docker inspect --format='{{.State.Health.Status}}' "keystone-test-postgres" 2>/dev/null || echo "starting")
  if [[ "$health" == "healthy" ]]; then
    ok "Database is ready."
    break
  fi
  sleep 1
  elapsed=$((elapsed + 1))
done

if [[ $elapsed -ge $DB_START_TIMEOUT ]]; then
  warn "Database did not become healthy in ${DB_START_TIMEOUT}s, trying anyway..."
fi

# Give postgres a moment to finish accepting connections
sleep 1

# ── Subcommand: up (just start the DB) ─────────────────────────────────────
if [[ "$ACTION" == "up" ]]; then
  ok "Test database is running on localhost:$DB_PORT"
  echo ""
  echo "  Connect with:"
  echo "    psql postgres://test:test@localhost:$DB_PORT/postgres"
  echo ""
  echo "  Run tests with:"
  echo "    TEST_DATABASE_BASE_URL=postgres://test:test@localhost:$DB_PORT/postgres \\"
  echo "      RUST_TEST_THREADS=1 APP_ENV=test cargo test"
  echo ""
  echo "  Tear down with:"
  echo "    ./test.sh down"
  exit 0
fi

# ── Run tests ───────────────────────────────────────────────────────────────
trap cleanup EXIT

TEST_BASE_URL="postgres://${DB_USER}:${DB_PASS}@localhost:${DB_PORT}/${DB_NAME}"

echo ""
echo -e "${BOLD}═══════════════════════════════════════════════${RESET}"
echo -e "${BOLD}  Running integration tests${RESET}"
echo -e "${BOLD}  DB: localhost:$DB_PORT (${DB_NAME})${RESET}"
if [[ "$CLEANUP" == "true" ]]; then
  echo -e "${BOLD}  Cleanup: will tear down after tests${RESET}"
else
  echo -e "${BOLD}  Cleanup: disabled (use ./test.sh down to remove)${RESET}"
fi
echo -e "${BOLD}═══════════════════════════════════════════════${RESET}"
echo ""

# TEST_DATABASE_BASE_URL tells helpers.rs to derive per-binary database names
# (e.g. keystone_test_api_auth_tests) from this base. Each test binary gets
# its own isolated database, created on demand via the maintenance connection
# to the `postgres` database in the same cluster.
#
# IMPORTANT: Do NOT set TEST_DATABASE_URL — that skips per-binary DB creation
# and forces all binaries to share one database, which breaks table truncation.
TEST_EXIT=0
TEST_DATABASE_BASE_URL="$TEST_BASE_URL" \
RUST_TEST_THREADS=1 \
APP_ENV=test \
  cargo test "${EXTRA_CARGO_ARGS[@]}" || TEST_EXIT=$?

echo ""
if [[ $TEST_EXIT -eq 0 ]]; then
  ok "All tests passed."
else
  warn "Some tests failed (exit code: $TEST_EXIT)."
fi

exit $TEST_EXIT
