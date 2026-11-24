#!/bin/sh
# Fails if the `unsafe` keyword appears in Rust code under crates/ outside the
# files listed in .github/unsafe-allowlist.txt.
#
# Usage: sh .github/scripts/check-unsafe.sh [repo-root]
#
# Comments (line, nested block, doc), string literals (normal, byte, raw) and
# char literals are blanked before matching, so prose such as "no unsafe" in
# docs, test strings, and the lint name in #![forbid(unsafe_code)] do not trip
# it. Any remaining `unsafe` token in code is reported: blocks, fns, impls,
# traits, extern blocks, and unsafe attributes. `r#unsafe` raw identifiers are
# ignored.
#
# Allowlist entries must exist and live in a `sys` directory (see
# .agents/skills/akku-unsafe/SKILL.md).
set -eu

root=${1:-$(cd "$(dirname "$0")/../.." && pwd)}
cd "$root"

allowlist=.github/unsafe-allowlist.txt
status=0

allowed=$(sed -e 's/#.*//' -e 's/[[:space:]]*$//' -e 's/^[[:space:]]*//' "$allowlist" | grep -v '^$' || true)

for path in $allowed; do
    if [ ! -f "$path" ]; then
        echo "unsafe allowlist: $path does not exist; remove the stale entry"
        status=1
    fi
    case "$path" in
        */sys/*) ;;
        *)
            echo "unsafe allowlist: $path is not inside a sys/ directory"
            status=1
            ;;
    esac
done

files=$(find crates -type f -name '*.rs' -not -path '*/target/*' | sort)

checked=""
for file in $files; do
    skip=0
    for path in $allowed; do
        if [ "$file" = "$path" ]; then
            skip=1
        fi
    done
    if [ "$skip" -eq 0 ]; then
        checked="$checked $file"
    fi
done

if [ -n "$checked" ]; then
    # shellcheck disable=SC2086 # word splitting of the file list is intended
    hits=$(LC_ALL=C awk '
        FNR == 1 { state = 0; depth = 0 }
        {
            line = $0; out = ""; n = length(line); i = 1
            while (i <= n) {
                c = substr(line, i, 1); two = substr(line, i, 2)
                if (state == 1) {                      # block comment
                    if (two == "*/") { depth--; i += 2; if (depth == 0) state = 0; continue }
                    if (two == "/*") { depth++; i += 2; continue }
                    i++; continue
                }
                if (state == 2) {                      # string literal
                    if (c == "\\") { i += 2; continue }
                    if (c == "\"") state = 0
                    i++; continue
                }
                if (state == 3) {                      # raw string literal
                    if (c == "\"" && substr(line, i + 1, hashes) == hashstr) {
                        state = 0; i += 1 + hashes; continue
                    }
                    i++; continue
                }
                if (two == "//") break
                if (two == "/*") { state = 1; depth = 1; i += 2; out = out " "; continue }
                if (c == "\"") { state = 2; i++; out = out " "; continue }
                if (c == "r") {
                    h = 0
                    while (substr(line, i + 1 + h, 1) == "#") h++
                    if (substr(line, i + 1 + h, 1) == "\"") {
                        state = 3; hashes = h; hashstr = substr(line, i + 1, h)
                        i += 2 + h; out = out " "; continue
                    }
                }
                if (c == "\047") {                     # char literal or lifetime
                    if (substr(line, i + 1, 1) == "\\") {
                        j = i + 2
                        while (j <= n && substr(line, j, 1) != "\047") j++
                        i = j + 1; out = out " "; continue
                    }
                    if (substr(line, i + 2, 1) == "\047") { i += 3; out = out " "; continue }
                }
                out = out c; i++
            }
            if (out ~ /(^|[^A-Za-z0-9_#])unsafe([^A-Za-z0-9_]|$)/)
                print FILENAME ":" FNR ": " $0
        }
    ' $checked)
    if [ -n "$hits" ]; then
        echo "unsafe outside $allowlist:"
        echo "$hits"
        echo "Policy: .agents/skills/akku-unsafe/SKILL.md"
        status=1
    fi
fi

if [ "$status" -eq 0 ]; then
    count=$(echo "$files" | grep -c . || true)
    echo "unsafe gate OK ($count Rust file(s) under crates/, allowlist: $(echo "$allowed" | grep -c . || true) file(s))"
fi
exit "$status"
