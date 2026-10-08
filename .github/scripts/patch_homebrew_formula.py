#!/usr/bin/env python3
"""Patch the Homebrew formula for a new tmail release.

Rewrites the version line (release URLs interpolate it) and each sha256
line, matched by the target triple in the preceding URL. Used by
.github/workflows/release.yml.
"""

from __future__ import annotations

import argparse
import re
import sys

# Formula URLs whose checksum needs refreshing, matched by target triple.
TRIPLE = re.compile(
    r'tmail-v[^"]*-(aarch64-apple-darwin|x86_64-apple-darwin'
    r'|x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu)\.tar\.gz"'
)
SHA = re.compile(r'^(\s*sha256 ")([0-9a-f]{64})(")$')
VERSION = re.compile(r'(?m)^  version "[^"]+"$')


def read_sums(path: str) -> dict[str, str]:
    """Return asset name -> digest from a sha256sum output file."""
    sums = {}
    with open(path, encoding="utf-8") as sums_file:
        for line in sums_file:
            digest, name = line.split(maxsplit=1)
            sums[name.strip()] = digest
    return sums


def patch(text: str, version: str, sums: dict[str, str]) -> str:
    """Return the formula text updated to the given version and digests."""
    text, count = VERSION.subn(f'  version "{version}"', text)
    if count != 1:
        sys.exit(f"expected exactly one version line, patched {count}")

    lines = text.split("\n")
    pending = None
    patched = 0
    for i, line in enumerate(lines):
        match = TRIPLE.search(line)
        if match:
            pending = match.group(1)
            continue
        match = SHA.match(line)
        if match and pending:
            asset = f"tmail-v{version}-{pending}.tar.gz"
            if asset not in sums:
                sys.exit(f"missing checksum for {asset}")
            lines[i] = f"{match.group(1)}{sums[asset]}{match.group(3)}"
            patched += 1
            pending = None
    if patched != 4:
        sys.exit(f"expected 4 sha256 lines, patched {patched}")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Patch the Homebrew formula for a new tmail release."
    )
    parser.add_argument("version", help="new release version, without the leading v")
    parser.add_argument(
        "--sums",
        default="SHA256SUMS.txt",
        help="sha256sum output for the release assets",
    )
    parser.add_argument(
        "--formula",
        default="tap/Formula/tmail.rb",
        help="formula file to patch",
    )
    args = parser.parse_args()

    with open(args.formula, encoding="utf-8") as formula:
        text = formula.read()
    with open(args.formula, "w", encoding="utf-8") as formula:
        formula.write(patch(text, args.version, read_sums(args.sums)))


if __name__ == "__main__":
    main()
