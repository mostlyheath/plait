#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
checkout=${1:-$root}
if (( $# > 1 )); then
  echo "usage: $0 [checkout]" >&2
  exit 1
fi
cd "$root"
cargo build --release -p plait-host
cargo build --release -p plait-engine --example git_speed
store=$(mktemp -d /tmp/plait-speed.XXXXXXXX)
cleanup() {
  # Match the exact scratch store argument, never the user's host.
  pids=$(ps -ax -o pid= -o command= | awk -v store="$store" '$0 ~ /plait-host run --store / && index($0, "--store " store " --runtime ") { print $1 }')
  if [[ -n "$pids" ]]; then
    kill $pids 2>/dev/null || true
  fi
  rm -rf "$store"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
"$root/target/release/plait-host" start --store "$store"
"$root/target/release/examples/git_speed" "$store/host.sock" "$checkout"
