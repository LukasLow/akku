#!/bin/sh
# Fails a pull request that has neither a changie fragment nor the
# skip-changelog label (the label is handled by the workflow).
set -eu

if [ -n "${SKIP_CHANGELOG:-}" ]; then
    exit 0
fi

if ! ls .changes/unreleased/*.yaml >/dev/null 2>&1; then
    echo "Missing changie fragment in .changes/unreleased/"
    echo "Run: changie new"
    echo "Or add the label skip-changelog (documentation only)."
    exit 1
fi

echo "Changelog fragment present."
