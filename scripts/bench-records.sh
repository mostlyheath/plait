#!/usr/bin/env bash
# Measures what the host's checkout records cost on a 50,000-file checkout,
# in process and through the helper the runtime starts (sandboxed on macOS),
# and what the two records add to a run in a small repository.
# Set PLAIT_BENCH_FILES to measure a different size. Target: a warm record of
# an unchanged 50,000-file checkout in under 300 ms on an Apple silicon Mac.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
export PLAIT_BENCH_FILES=${PLAIT_BENCH_FILES:-50000}

echo "Machine: $(uname -sm), $(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown CPU)"
echo "Git: $(git --version)"
echo
# Preserve the actual error when a timing test fails.
report=$(mktemp)
trap 'rm -f "$report"' EXIT
measure() {
    if "$@" >"$report" 2>&1; then
        grep -E 'files=|host (large|small):' "$report"
    else
        cat "$report" >&2
        return 1
    fi
}
echo "Recorder, in process (release build):"
measure cargo test --locked --release -p plait-engine --lib record_timing -- --ignored --nocapture --test-threads=1
echo
echo "Through plait-host record, as the runtime runs it (release build):"
measure cargo test --locked --release -p plait-host --test record host_record_timing -- --ignored --nocapture --test-threads=1
