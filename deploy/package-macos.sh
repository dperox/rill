#!/usr/bin/env bash
# Run from the release checkout. Produces an archive and a sibling SHA-256 file.
set -euo pipefail
case "${1:-}" in
    arm64) target=aarch64-apple-darwin ;;
    amd64) target=x86_64-apple-darwin ;;
    *) echo "usage: $0 arm64|amd64 OUTPUT_DIRECTORY" >&2; exit 2 ;;
esac
mkdir -p -- "${2:?output directory required}"
out=$(cd -- "$2" && pwd)
version=$(cargo metadata --locked --no-deps --format-version 1 |
    python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "rill"))')
if [ "${RELEASE_TAG-v$version}" != "v$version" ]; then
    echo "Release tag does not match Cargo version $version" >&2
    exit 1
fi

export MACOSX_DEPLOYMENT_TARGET=15.0
cargo build --locked --release --target "$target" --target-dir target
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT INT TERM HUP
mkdir -- "$stage/rill"
cp -R -- "target/$target/release/rill" README.md LICENSE public notices "$stage/rill/"
host_arch=$(uname -m)
if { [ "$1" = "arm64" ] && [ "$host_arch" = "arm64" ]; } || \
   { [ "$1" = "amd64" ] && [ "$host_arch" = "x86_64" ]; }; then
    (cd -- "$stage/rill" && env -i PATH="$PATH" ./rill config test)
else
    echo "Skipping native preflight on $host_arch host for $1 binary" >&2
fi
archive="rill-$version-macos-$1.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$out/$archive" -C "$stage" rill
(cd -- "$out" && shasum -a 256 "$archive" > "$archive.sha256")
