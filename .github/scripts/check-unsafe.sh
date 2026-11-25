#!/bin/sh
# Fails if the `unsafe` keyword or an `unsafe_code` lint override (such as
# `#[allow(unsafe_code)]`, `#![allow(unsafe_code)]`, `warn`, or `expect`)
# appears in Rust code under crates/ outside the files listed in
# .github/unsafe-allowlist.txt.
#
# Usage: sh .github/scripts/check-unsafe.sh [repo-root]
#
# Comments (line, nested block, doc), string literals (normal, byte, raw) and
# char literals are blanked before matching, so prose such as "no unsafe" in
# docs, test strings, and crate-level `#![deny(unsafe_code)]` /
# `#![forbid(unsafe_code)]` attributes do not trip it. Any remaining `unsafe`
# token in code (blocks, fns, impls, traits, extern blocks, unsafe attributes)
# or any non-forbid/non-deny `unsafe_code` attribute is reported. `r#unsafe`
# raw identifiers are ignored.
#
# Allowlist entries must exist and be `.rs` files inside a `crates/.../sys/`
# directory (see .agents/skills/akku-unsafe/SKILL.md).
set -eu
set -f

root=${1:-$(cd "$(dirname "$0")/../.." && pwd)}
cd "$root"

allowlist=.github/unsafe-allowlist.txt
status=0

if [ ! -f "$allowlist" ]; then
    echo "unsafe allowlist: $allowlist is missing"
    exit 1
fi

allowed=$(sed -e 's/#.*//' -e 's/[[:space:]]*$//' -e 's/^[[:space:]]*//' "$allowlist" | grep -v '^$' || true)

for path in $allowed; do
    if [ -L "$path" ]; then
        echo "unsafe allowlist: $path must not be a symlink"
        status=1
    elif [ ! -f "$path" ]; then
        echo "unsafe allowlist: $path does not exist; remove the stale entry"
        status=1
    fi
    case "$path" in
        *..* | /* | *//* | */./* | ./*)
            echo "unsafe allowlist: $path must be a normalized repo-relative path"
            status=1
            ;;
        crates/*/sys/*.rs) ;;
        *)
            echo "unsafe allowlist: $path must be a .rs file inside a crates/.../sys/ directory"
            status=1
            ;;
    esac
done

symlinks=$(find crates -type l ! -path '*/target/*' | sort)
if [ -n "$symlinks" ]; then
    echo "symlinks are not allowed under crates/:"
    echo "$symlinks"
    status=1
fi

files=$(find crates -type f -name '*.rs' ! -path '*/target/*' | sort)

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
    # shellcheck disable=SC2086 # word splitting of the file list is intended (noglob is set)
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
                    prev = (i > 1) ? substr(line, i - 1, 1) : ""
                    prev2 = (i > 2) ? substr(line, i - 2, 1) : ""
                    if (prev !~ /[A-Za-z0-9_]/ || ((prev == "b" || prev == "c") && prev2 !~ /[A-Za-z0-9_]/)) {
                        h = 0
                        while (substr(line, i + 1 + h, 1) == "#") h++
                        if (substr(line, i + 1 + h, 1) == "\"") {
                            state = 3; hashes = h; hashstr = substr(line, i + 1, h)
                            i += 2 + h; out = out " "; continue
                        }
                    }
                    if (prev !~ /[A-Za-z0-9_]/ && substr(line, i, 8) == "r#unsafe" && substr(line, i + 8, 1) !~ /[A-Za-z0-9_]/) {
                        i += 8; out = out " "; continue
                    }
                }
                if (c == "\047") {                     # char literal or lifetime
                    if (substr(line, i + 1, 1) == "\\") {
                        j = i + 3
                        while (j <= n && substr(line, j, 1) != "\047") j++
                        i = j + 1; out = out " "; continue
                    }
                    if (substr(line, i + 2, 1) == "\047") { i += 3; out = out " "; continue }
                }
                out = out c; i++
            }
            if (out ~ /(^|[^A-Za-z0-9_])unsafe([^A-Za-z0-9_]|$)/) {
                print FILENAME ":" FNR ": " $0
            } else if (out ~ /(^|[^A-Za-z0-9_])unsafe_code([^A-Za-z0-9_]|$)/ &&
                out !~ /^[ \t]*#!?\[[ \t]*(forbid|deny)[ \t]*\([ \t]*unsafe_code[ \t]*\)[ \t]*\][ \t]*$/) {
                print FILENAME ":" FNR ": " $0
            }
        }
    ' $checked)
    if [ -n "$hits" ]; then
        echo "unsafe or unsafe_code override outside $allowlist:"
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
