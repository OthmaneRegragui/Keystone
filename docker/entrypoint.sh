#!/bin/sh
set -eu

# The storage volume (the "storagedata" named volume, or a DATA_HOST_PATH bind
# mount) may be owned by root or another user — e.g. created by an older image,
# populated as root, or set up on the host. Mounting hides the image-time chown
# in the Dockerfile, so fix ownership at startup: this makes the storage
# root(s) writable by the `keystone` app user, then drops privileges and execs
# the server. The server process itself never runs as root.
#
# STORAGE_LOCAL_PATHS is the path INSIDE the container (comma-separated list
# allowed) and defaults to /mnt/keystone/data in docker-compose.
STORAGE_PATHS="${STORAGE_LOCAL_PATHS:-/mnt/keystone/data}"
IFS=','
for path in $STORAGE_PATHS; do
    [ -n "$path" ] || continue
    chown -R keystone:keystone "$path"
done
unset IFS

exec setpriv --reuid=keystone --regid=keystone --init-groups "$@"
