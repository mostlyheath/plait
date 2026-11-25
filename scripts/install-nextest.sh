#!/usr/bin/env bash
# Installs the pinned cargo-nextest release into Cargo's bin directory after
# checking its SHA-256. CI uses it; scripts/check.sh falls back to cargo test without it.
set -euo pipefail

version=0.9.152
case "$(uname -s)-$(uname -m)" in
    Darwin-*)
        target=universal-apple-darwin
        digest=2fadf57df4243e691f088ee3bff4f4ab9af16da0c95a1db84b5cb1685e098a76
        ;;
    Linux-x86_64)
        target=x86_64-unknown-linux-gnu
        digest=d039b43c3e0958441b6ad0a39c8884d018b3d2e8fb9928f209074b1100b5e17a
        ;;
    Linux-aarch64)
        target=aarch64-unknown-linux-gnu
        digest=ca1f81a5b86533eb45ab97e0064aa88e925e53aab9a698ca6331a7cad1f85f07
        ;;
    *)
        echo "No pinned cargo-nextest for $(uname -s) $(uname -m)." >&2
        exit 1
        ;;
esac

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
archive="$scratch/cargo-nextest.tar.gz"
curl --proto '=https' --tlsv1.2 -sSfL -o "$archive" \
    "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$version/cargo-nextest-$version-$target.tar.gz"
if command -v sha256sum > /dev/null; then
    actual=$(sha256sum "$archive" | awk '{print $1}')
else
    actual=$(shasum -a 256 "$archive" | awk '{print $1}')
fi
if [[ $actual != "$digest" ]]; then
    echo "cargo-nextest $version checksum mismatch: $actual" >&2
    exit 1
fi
bin="${CARGO_HOME:-$HOME/.cargo}/bin"
mkdir -p "$bin"
tar -xzf "$archive" -C "$scratch" cargo-nextest
install -m 0755 "$scratch/cargo-nextest" "$bin/cargo-nextest"
cargo nextest --version
