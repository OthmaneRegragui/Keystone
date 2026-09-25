#!/usr/bin/env bash
#
# auto-update.sh - host-side helper for the admin "Update & Restart" button.
#
# The Keystone server runs inside Docker and cannot touch the host's git or
# Docker daemon. When an admin clicks "Update & Restart" in the admin panel,
# the server drops a small marker file into a folder that is bind-mounted from
# the host. This script watches that folder and, when it sees a marker, pulls
# the latest code and runs ./docker-run.sh to rebuild and restart the stack.
#
# Setup (one time):
#   1. Make sure .env contains an UPDATE_REQUEST_DIR pointing at a host folder
#      that is mounted into the server container, e.g.:
#          UPDATE_REQUEST_DIR=./update-requests
#      docker-run.sh mounts that folder into the container automatically.
#   2. Start this watcher in the background (it is not started by docker-run.sh
#      so you stay in control of when updates happen):
#          nohup ./docker/auto-update.sh > /dev/null 2>&1 &
#
# The watcher is safe to run indefinitely: it does nothing until a marker file
# appears, and each marker is processed once.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$SCRIPT_DIR"

POLL_INTERVAL=5
WATCH_DIR=""

# Same .env loading convention as docker-run.sh.
if [ -f .env ]; then
  WATCH_DIR="$(sed -n 's/^UPDATE_REQUEST_DIR=//p' .env | tail -n1 | tr -d '"'"'"' \r')"
fi

if [ -z "$WATCH_DIR" ]; then
  echo "error: UPDATE_REQUEST_DIR is not set in .env" >&2
  exit 1
fi

# Relative paths are relative to the project root, matching docker-run.sh.
case "$WATCH_DIR" in
  /*) ;;
  *) WATCH_DIR="$PWD/$WATCH_DIR" ;;
esac

mkdir -p "$WATCH_DIR"

echo ">> Keystone auto-update watcher started (watching $WATCH_DIR)"

while true; do
  REQUEST="$(find "$WATCH_DIR" -maxdepth 1 -name 'restart-*.request' -print -quit 2>/dev/null || true)"
  if [ -n "$REQUEST" ]; then
    echo ">> Update requested ($(date)) - pulling and rebuilding Keystone..."
    rm -f "$REQUEST"
    if git pull --ff-only && ./docker-run.sh; then
      echo ">> Keystone updated and restarted successfully."
    else
      echo ">> Keystone update failed. Check the output above." >&2
    fi
  fi
  sleep "$POLL_INTERVAL"
done
