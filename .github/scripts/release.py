#!/usr/bin/env python3
"""Prepare or validate an Akku release.

Usage:
    python3 .github/scripts/release.py auto
    python3 .github/scripts/release.py 0.0.2
    python3 .github/scripts/release.py --check

The workspace version in Cargo.toml is the single source of truth. The script
updates it, runs changie batch/merge and writes releases/v<version>-<date>.md.
It does not commit, tag or publish.
"""

from __future__ import annotations

import argparse
import datetime as dt
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CARGO = ROOT / "Cargo.toml"
CHANGELOG = ROOT / "CHANGELOG.md"
UNRELEASED = ROOT / ".changes" / "unreleased"
RELEASES = ROOT / "releases"
CHANGIE = shutil.which("changie") or "changie"

VERSION = re.compile(r"^\d+\.\d+\.\d+$")
KIND_BUMP = {
    "Fixed": "patch",
    "Security": "patch",
    "Added": "minor",
    "Changed": "minor",
    "Deprecated": "minor",
    "Removed": "minor",
}


def fail(message: str) -> "NoReturn":
    print(f"release: {message}", file=sys.stderr)
    raise SystemExit(1)


def parse_version(value: str) -> tuple[int, int, int]:
    if not VERSION.match(value):
        fail(f"invalid semantic version {value!r}")
    return tuple(int(part) for part in value.split("."))  # type: ignore[return-value]


def format_version(version: tuple[int, int, int]) -> str:
    return ".".join(str(part) for part in version)


def bump(version: str, kind: str) -> str:
    major, minor, patch = parse_version(version)
    if kind == "major":
        return format_version((major + 1, 0, 0))
    if kind == "minor":
        return format_version((major, minor + 1, 0))
    return format_version((major, minor, patch + 1))


def cargo_version() -> str:
    try:
        return tomllib.loads(CARGO.read_text())["workspace"]["package"]["version"]
    except KeyError:
        fail("Cargo.toml has no [workspace.package] version")


def fragment_bump(path: Path) -> str:
    text = path.read_text()
    explicit = re.search(r"(?m)^\s*version:\s*(patch|minor|major)\s*$", text)
    if explicit:
        return explicit.group(1)
    kind = re.search(r"(?m)^kind:\s*([A-Za-z]+)\s*$", text)
    if not kind:
        fail(f"{path}: missing `kind:`")
    result = KIND_BUMP.get(kind.group(1))
    if result is None:
        fail(f"{path}: unknown kind {kind.group(1)!r}")
    return result


def fragment_bumps() -> list[str]:
    if not UNRELEASED.is_dir():
        return []
    return [
        fragment_bump(path)
        for path in sorted(UNRELEASED.glob("*.yaml"))
        if path.name != ".gitkeep"
    ]


def highest(bumps: list[str]) -> str:
    rank = {"patch": 1, "minor": 2, "major": 3}
    if not bumps:
        fail("no changie fragments in .changes/unreleased")
    return max(bumps, key=rank.__getitem__)


def set_cargo_version(version: str) -> None:
    text = CARGO.read_text()
    start = text.find("[workspace.package]")
    if start == -1:
        fail("Cargo.toml has no [workspace.package]")
    end = text.find("\n[", start + 1)
    if end == -1:
        end = len(text)
    section = text[start:end]
    updated, count = re.subn(
        r'(?m)^version\s*=\s*"[^"]+"',
        f'version = "{version}"',
        section,
        count=1,
    )
    if count != 1:
        fail("[workspace.package] has no version line")
    text = text[:start] + updated + text[end:]

    dep_start = text.find("[workspace.dependencies]")
    if dep_start != -1:
        dep_end = text.find("\n[", dep_start + 1)
        if dep_end == -1:
            dep_end = len(text)
        deps = text[dep_start:dep_end].splitlines()
        for index, line in enumerate(deps):
            if line.startswith(("akku =", "akku-drive =")):
                deps[index] = re.sub(
                    r'version\s*=\s*"[^"]+"', f'version = "{version}"', line
                )
        text = text[:dep_start] + "\n".join(deps) + text[dep_end:]

    tomllib.loads(text)
    CARGO.write_text(text)


def run_changie(*args: str) -> None:
    if not shutil.which("changie"):
        fail("changie is not installed")
    subprocess.run([CHANGIE, *args], cwd=ROOT, check=True)


def changelog_section(version: str) -> str:
    lines = CHANGELOG.read_text().splitlines()
    start = None
    for index, line in enumerate(lines):
        if line.startswith(f"## {version}") and (
            len(line) == len(version) + 3 or line[len(version) + 3] in " -\t"
        ):
            start = index
            break
    if start is None:
        fail(f"CHANGELOG.md has no section for {version}")
    end = len(lines)
    for index in range(start + 1, len(lines)):
        if lines[index].startswith("## "):
            end = index
            break
    return "\n".join(lines[start + 1 : end]).strip()


def write_notes(version: str, date: str) -> Path:
    body = changelog_section(version)
    RELEASES.mkdir(exist_ok=True)
    path = RELEASES / f"v{version}-{date}.md"
    path.write_text(f"# akku {version}\n\nReleased {date}.\n\n{body}\n")
    return path


def check() -> None:
    version = cargo_version()
    parse_version(version)
    notes = sorted(RELEASES.glob(f"v{version}-*.md")) if RELEASES.is_dir() else []
    if notes:
        section = changelog_section(version)
        if not section:
            fail(f"release notes exist but CHANGELOG.md section for {version} is empty")
        if (ROOT / ".changes" / f"{version}.md").is_file():
            print(
                f"release: {version} is released ({notes[0].name}); "
                "fragments are for the next version"
            )
            return
        pending = fragment_bumps()
        if pending:
            fail(
                f"release {version} is prepared but {len(pending)} fragment(s) "
                "remain in .changes/unreleased"
            )
        print(f"release: {version} is prepared ({notes[0].name})")
        return
    print(f"release: no prepared release for {version}; fragments are allowed")


def prepare(requested: str) -> None:
    current = cargo_version()
    parse_version(current)

    if requested == "auto":
        kind = highest(fragment_bumps())
        version = bump(current, kind)
    elif requested in {"patch", "minor", "major"}:
        version = bump(current, requested)
    else:
        version = requested
        parse_version(version)
        if parse_version(version) <= parse_version(current):
            fail(f"new version {version} must be greater than {current}")

    original = CARGO.read_text()
    try:
        set_cargo_version(version)
        subprocess.run(["cargo", "update", "--workspace"], cwd=ROOT, check=True)
        run_changie("batch", version)
        run_changie("merge")
    except Exception:
        CARGO.write_text(original)
        raise

    date = dt.date.today().isoformat()
    notes = write_notes(version, date)
    print(f"release: {current} -> {version}")
    print(f"release: wrote {notes.relative_to(ROOT)}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Prepare an Akku release")
    parser.add_argument("version", nargs="?", help="auto, patch, minor, major or X.Y.Z")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        check()
        return
    if not args.version:
        parser.error("version is required (auto, patch, minor, major or X.Y.Z)")
    prepare(args.version)


if __name__ == "__main__":
    main()
