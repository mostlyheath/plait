#!/bin/bash
set -euo pipefail

# On pull requests HEAD is the test merge. Compare it with its base parent.
if git diff --quiet --no-ext-diff --no-textconv "$1" HEAD -- runtime/ \
    scripts/{build-macos.sh,package-macos.py,release_macos.py,package_linux_host.py} \
    'scripts/installed-*.mjs' scripts/{check-installed-changes.sh,smoke-installed-macos.mjs,smoke-installer-macos.sh}; then
    echo 'Installed runtime layout inputs are unchanged.'
else
    # An unavailable push base also takes the conservative path.
    node scripts/installed-layout-check.mjs
fi
