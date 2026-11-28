#!/usr/bin/env bash
# Run from the release checkout. Produces an ARM64 archive and a sibling SHA-256 file.
set -euo pipefail
if [ "$#" -ne 2 ] || [ "$1" != arm64 ]; then
    echo "usage: $0 arm64 OUTPUT_DIRECTORY" >&2
    exit 2
fi
target=aarch64-apple-darwin
mkdir -p -- "$2"
out=$(cd -- "$2" && pwd)
version=$(cargo metadata --locked --no-deps --format-version 1 |
    python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "japka"))')
if [ "${RELEASE_TAG-v$version}" != "v$version" ]; then
    echo "Release tag does not match Cargo version $version" >&2
    exit 1
fi

export MACOSX_DEPLOYMENT_TARGET=15.0
cargo build --locked --release --target "$target" --target-dir target
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT INT TERM HUP
mkdir -- "$stage/japka"
cp -R -- "target/$target/release/japka" README.md LICENSE public notices "$stage/japka/"
host_arch=$(uname -m)
if [ "$host_arch" = arm64 ]; then
    (cd -- "$stage/japka" && env -i PATH="$PATH" ./japka config test)
else
    echo "Skipping native preflight on $host_arch host for ARM64 binary" >&2
fi
archive="japka-$version-macos-arm64.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$out/$archive" -C "$stage" japka
(cd -- "$out" && shasum -a 256 "$archive" > "$archive.sha256")
