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
unset PLAIT_RELEASE_VERSION
if [[ -n ${PLAIT_RELEASE_TAG:-} ]]; then
    [[ $PLAIT_RELEASE_TAG =~ ^v([0-9]+\.[0-9]+\.[0-9]+(-preview\.[1-9][0-9]*)?)$ ]] || {
        echo "Unrecognized release tag: $PLAIT_RELEASE_TAG" >&2
        exit 1
    }
    export PLAIT_RELEASE_VERSION=${BASH_REMATCH[1]}
fi
cargo build --locked --release --workspace --target-dir "$root/target"
cargo metadata --locked --no-deps --format-version 1 > target/macos-package-metadata.json
# License harvesting is a build tool, not a dependency shipped inside the app.
cargo install cargo-about --locked --version 0.9.3 --features cli --root target/build-tools
target/build-tools/bin/cargo-about generate --locked --fail \
    --manifest-path crates/plait/Cargo.toml --config packaging/macos/about.toml \
    --output-file target/THIRD_PARTY_NOTICES.txt packaging/macos/notices.hbs
target/build-tools/bin/cargo-about generate --locked --fail \
    --manifest-path crates/plait-host/Cargo.toml --config packaging/macos/about.toml \
    --output-file target/HOST_THIRD_PARTY_NOTICES.txt packaging/macos/notices.hbs
python3 scripts/package-macos.py
