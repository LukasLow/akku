#!/bin/sh
# Fails a pull request that does not add its own changie fragment to
# .changes/unreleased/. Fragments that already exist on the base branch do not
# count. Exemptions (the skip-changelog label, release/* branches) are handled
# by the workflow; SKIP_CHANGELOG=1 skips the check locally.
#
# The base is origin/$GITHUB_BASE_REF (set by GitHub on pull_request events,
# default: main), so the checkout needs the base history (fetch-depth: 0).
set -eu

if [ -n "${SKIP_CHANGELOG:-}" ]; then
    exit 0
fi

base="origin/${GITHUB_BASE_REF:-main}"

if ! git rev-parse -q --verify "$base^{commit}" >/dev/null; then
    echo "Base $base is not available; fetch it (actions/checkout fetch-depth: 0)."
    exit 1
fi

added=$(git diff --name-only --diff-filter=A "$base...HEAD" -- '.changes/unreleased/*.yaml')

if [ -z "$added" ]; then
    echo "This PR adds no changie fragment in .changes/unreleased/ (base: $base)."
    echo "Run: changie new"
    echo "Or add the label skip-changelog (not user-facing: docs, CI, tests, refactor)."
    exit 1
fi

echo "Changelog fragment(s) added by this PR:"
echo "$added"
