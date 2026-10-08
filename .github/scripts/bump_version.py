#!/usr/bin/env python3
"""Bump the top-level [package] version in Cargo.toml.

Prints the new version to stdout. Used by .github/workflows/release.yml.
"""

from __future__ import annotations

import argparse
import re
import sys

# Exactly one top-level version key; dependency versions are indented and
# therefore not matched.
VERSION_KEY = re.compile(r'(?m)^(version\s*=\s*")([^"]+)(")')


def read_version(text: str) -> str:
    """Return the single top-level version from a Cargo.toml."""
    matches = VERSION_KEY.findall(text)
    if len(matches) != 1:
        sys.exit("expected exactly one top-level version key in Cargo.toml")
    return matches[0][1]


def bump(version: str, component: str) -> str:
    """Return version with the requested semver component bumped."""
    major, minor, patch = (int(part) for part in version.split("."))
    if component == "patch":
        patch += 1
    elif component == "minor":
        minor += 1
        patch = 0
    elif component == "major":
        major += 1
        minor = 0
        patch = 0
    else:
        sys.exit(f"unknown bump kind: {component}")
    return f"{major}.{minor}.{patch}"


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Bump the top-level [package] version in Cargo.toml."
    )
    parser.add_argument("component", choices=["patch", "minor", "major"])
    parser.add_argument("--manifest", default="Cargo.toml", help="manifest to bump")
    args = parser.parse_args()

    with open(args.manifest, encoding="utf-8") as manifest:
        text = manifest.read()
    new = bump(read_version(text), args.component)
    with open(args.manifest, "w", encoding="utf-8") as manifest:
        manifest.write(VERSION_KEY.sub(rf"\g<1>{new}\g<3>", text, count=1))
    print(new)


if __name__ == "__main__":
    main()
