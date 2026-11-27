#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
if [[ $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    echo 'This development package requires an Apple Silicon Mac.' >&2
    exit 1
fi
# Matches package names, so an installed build identifies uncommitted changes.
PLAIT_BUILD_REVISION=$(git rev-parse --short=12 HEAD)
[[ -z $(git status --porcelain) ]] || PLAIT_BUILD_REVISION+='-dirty'
export PLAIT_BUILD_REVISION
# Tagged builds know their release version, so installed apps can compare updates.
if [[ -n ${PLAIT_RELEASE_TAG:-} ]]; then
    [[ $PLAIT_RELEASE_TAG =~ ^v([0-9]+\.[0-9]+\.[0-9]+(-preview\.[1-9][0-9]*)?)$ ]] || {
        echo "Unrecognized release tag: $PLAIT_RELEASE_TAG" >&2
        exit 1
    }
    export PLAIT_RELEASE_VERSION=${BASH_REMATCH[1]}
fi
for component in plait plait-host; do
    cargo build --locked --release -p "$component" --bin "$component" --target-dir "$root/target"
done
cargo metadata --locked --filter-platform aarch64-apple-darwin --format-version 1 > target/macos-package-metadata.json
python3 scripts/rust_license_report.py aarch64-apple-darwin plait plait-host
python3 scripts/package-macos.py
