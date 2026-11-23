#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

require_sandbox=false
if [[ $# -gt 1 || (${1:-} != '' && ${1:-} != '--require-command-sandbox') ]]; then
    echo 'Usage: bash scripts/check.sh [--require-command-sandbox]' >&2
    exit 1
fi
if [[ ${1:-} == '--require-command-sandbox' ]]; then
    require_sandbox=true
fi

case "$(uname -s)" in
    Darwin)
        packages=(--workspace)
        echo 'Checking the macOS workspace and command sandbox.'
        ;;
    Linux)
        if "$require_sandbox"; then
            echo 'Linux command-sandbox acceptance is blocked. See docs/LINUX-SANDBOX.md.' >&2
            exit 1
        fi
        packages=(-p plait-client -p plait-engine -p plait-protocol -p plait-transport -p plait-release -p plait-host -p plait-interop-tests)
        echo 'Checking Linux headless development. Native UI and Linux command execution are not validated.'
        ;;
    *)
        echo 'Development checks support macOS and Linux only.' >&2
        exit 1
        ;;
esac

cargo fmt --all -- --check
# Only pure acceptance logic: no window, permission query, clipboard or input.
if [[ $(uname -s) == Darwin ]]; then
    bash scripts/acceptance-macos.sh --self-test
else
    python3 -m unittest discover -s scripts/acceptance -p 'test_*.py'
fi
# The protocol dependency closure must stay free of platform I/O and UI crates, on
# every target. The cryptography entries are pure Ed25519/SHA-2 implementations
# (fiat-crypto is curve25519-dalek's verified pure-Rust backend on some targets);
# libc is allowed only because cpufeatures reads CPU features through it on Apple
# silicon.
protocol_dependencies=$(cargo tree --locked -p plait-protocol --edges normal --prefix none --target all)
protocol_allowed='^(plait-protocol|base64|serde|serde_core|serde_derive|serde_json|proc-macro2|unicode-ident|quote|syn|itoa|memchr|zmij|unicode-properties|ed25519-dalek|curve25519-dalek|curve25519-dalek-derive|fiat-crypto|cfg-if|cpufeatures|libc|digest|block-buffer|generic-array|typenum|crypto-common|subtle|zeroize|ed25519|signature|sha2) v'
if unexpected=$(grep -Ev "$protocol_allowed" <<< "$protocol_dependencies"); then
    echo 'Protocol dependency is not on the I/O-free allowlist; review it before adding it:' >&2
    sort -u <<< "$unexpected" >&2
    exit 1
fi
host_dependencies=$(cargo tree --locked -p plait-host --edges normal --prefix none)
if grep -Eq '^plait-client( |$)' <<< "$host_dependencies"; then
    echo 'The host must not depend on plait-client.' >&2
    exit 1
fi
for package in plait-client plait; do
    dependencies=$(cargo tree --locked -p "$package" -e normal --prefix none)
    if grep -Eq '^plait-engine( |$)' <<< "$dependencies"; then
        echo "$package must not depend on plait-engine." >&2
        exit 1
    fi
done
# The daemon and shared core must stay independent of desktop dependencies.
dependencies=$(cargo tree --locked -p plait-client -p plait-host --edges normal,build,dev --prefix none)
if grep -Eq '^gpui([-_ ]|$)' <<< "$dependencies"; then
    echo 'Headless crates must not depend on GPUI.' >&2
    exit 1
fi
cargo check --locked -p plait-client --no-default-features
if rustup target list --installed | grep -qx aarch64-apple-ios; then
    cargo check --locked -p plait-client --no-default-features --target aarch64-apple-ios
else
    echo 'Skipping iOS client check: aarch64-apple-ios target is not installed.'
fi
# The interop tests start a real host on the runtime's test fixture, so its packages come first.
# Optional packages are only the Agent SDK's platform builds of Claude Code, which no test runs.
(cd runtime && npm ci --ignore-scripts --omit=optional)
cargo clippy --locked "${packages[@]}" --all-targets -- -D warnings
cargo test --locked "${packages[@]}"
cargo build --locked -p plait-host
(
    cd runtime
    npm run check
    if "$require_sandbox"; then
        npm run test:sandbox
    else
        npm test
    fi
)
# CI's package job builds and tests the release binaries itself, beside this one.
if [[ -z ${PLAIT_CHECK_SKIP_RELEASE:-} ]]; then
    cargo build --locked --release "${packages[@]}"
fi
