#!/bin/sh
# Run from the repository checkout.
set -eu
cd "$(dirname "$0")/.."
if [ -z "${RILL:-}" ]; then
    cargo build --locked
    RILL="$PWD/target/debug/rill"
fi
export RILL
python3 checks/reload.py
python3 checks/shared_listener.py
python3 checks/websocket.py
python3 checks/sites.py
python3 checks/edge_routing.py
python3 checks/migration.py
