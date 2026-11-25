#!/usr/bin/env bash
# Measure headless UI frame cost for heavy scenes and print a table.
#
# A frame is Window::draw on GPUI's test platform with real text shaping:
# render, layout, prepaint, and paint. No GPU submission, no native window.
# Scenes hold generated data; no host is contacted and no agent runs.
# PLAIT_BENCH_FRAMES sets the measured frames per scenario (default 30).
# PLAIT_BENCH_COMPOSER_ONLY=1 selects the four 1380x860 composer scenarios.
# Pass --debug for a quick unoptimized run; the default is the release profile.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out=$(mktemp)
trap 'rm -f -- "$out"' EXIT

profile=(--release)
[[ ${1:-} == --debug ]] && profile=()

cd "$root"
PLAIT_BENCH_OUT="$out" \
    cargo test --locked "${profile[@]}" -p plait ui::snapshot::frame_bench -- --exact --ignored
cat "$out"
