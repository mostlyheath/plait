#!/bin/bash
# Prepare trusted build dependencies outside the agent's offline command sandbox.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
    echo 'Self-development in Plait currently requires an Apple Silicon Mac.' >&2
    exit 1
fi
for tool in git rustup cargo node npm xcodebuild; do
    command -v "$tool" >/dev/null || { echo "Missing $tool. See CONTRIBUTING.md#build-plait-with-plait." >&2; exit 1; }
done
if ! xcrun --sdk macosx --find metal >/dev/null; then
    echo 'Install the Xcode Metal component: xcodebuild -downloadComponent MetalToolchain' >&2
    exit 1
fi
printf 'Preparing the pinned Rust toolchain and locked sources. No provider account is used.\n'
rustup show active-toolchain
cargo fetch --locked
printf '\nInstalling locked runtime packages without install scripts.\n'
(cd runtime && npm ci --ignore-scripts && npm run check)
printf '\nBuilding Plait and running the Rust workspace tests.\n'
cargo build --offline --locked --release --workspace
cargo test --offline --locked --workspace
printf '\nReady for offline locked Cargo builds inside Plait.\n'
printf 'Open this checkout in Plait: %s\n' "$root"
printf 'Use an authenticated model and review each edit/command approval.\n'
printf 'Git writes, dependency updates, native app launches, and packaging remain outside the agent sandbox.\n'
