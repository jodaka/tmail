#!/usr/bin/env python3
"""Assemble release notes from the commits since the previous release.

Used by .github/workflows/release.yml. Commits are grouped by their
conventional-commit type. Merge commits and the version-bump commit are
omitted because they carry no change of their own.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys

# Conventional-commit type -> changelog section, in display order.
SECTIONS = {
    "feat": "Features",
    "fix": "Fixes",
    "perf": "Performance",
    "docs": "Documentation",
}
OTHER = "Other changes"

# "type(scope)!:" prefix of a conventional-commit subject.
PREFIX = re.compile(r"^(?P<type>[a-zA-Z]+)(?:\([^)]*\))?!?:\s")
# The version bump commit created by the `bump` job of the release workflow.
VERSION_BUMP = re.compile(r"^chore(?:\([^)]*\))?!?:\s*bump version\b")


def git(*args: str, check: bool = True) -> str:
    """Run git, returning stdout; exit cleanly on failure when checked."""
    result = subprocess.run(["git", *args], capture_output=True, text=True, check=False)
    if check and result.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def resolve_tag(explicit: str | None) -> str:
    """Return the release tag: the explicit one or the nearest from HEAD."""
    if explicit:
        return explicit
    return git("describe", "--tags", "--abbrev=0").strip()


def resolve_prev(tag: str, explicit: str | None) -> str | None:
    """Return the previous release tag, or None for the first release."""
    if explicit:
        return explicit
    # Nearest tag reachable from the release commit's parent; absent when
    # the release is the first tagged commit.
    return (
        git("describe", "--tags", "--abbrev=0", f"{tag}^", check=False).strip() or None
    )


def resolve_repo_url(explicit: str | None) -> str | None:
    """Return the repository web URL for links, or None outside Actions."""
    if explicit:
        return explicit.rstrip("/")
    server = os.environ.get("GITHUB_SERVER_URL")
    repo = os.environ.get("GITHUB_REPOSITORY")
    if server and repo:
        return f"{server}/{repo}"
    # Local runs have no repository URL; fall back to bare commit hashes.
    return None


def read_commits(rev_range: str) -> list[tuple[str, str, str]]:
    """Return (short hash, full hash, subject) for non-merge commits."""
    out = git("log", "--no-merges", "--pretty=format:%h%x09%H%x09%s", rev_range)
    commits = []
    for line in out.splitlines():
        if line:
            short, sha, subject = line.split("\t", 2)
            commits.append((short, sha, subject))
    return commits


def render(
    prev: str | None,
    tag: str,
    repo_url: str | None,
    commits: list[tuple[str, str, str]],
) -> str:
    """Render the changelog markdown for the given commits."""
    groups: dict[str, list[str]] = {}
    for short, sha, subject in commits:
        if VERSION_BUMP.match(subject):
            continue
        match = PREFIX.match(subject)
        kind = match.group("type") if match else None
        if kind not in SECTIONS:
            kind = "other"
        ref = f"[{short}]({repo_url}/commit/{sha})" if repo_url else short
        groups.setdefault(kind, []).append(f"- {subject} ({ref})")

    lines = [f"## Changes since {prev}" if prev else "## Changes", ""]
    for kind in [*SECTIONS, "other"]:
        if kind not in groups:
            continue
        lines.append(f"### {SECTIONS.get(kind, OTHER)}")
        lines.append("")
        lines.extend(groups[kind])
        lines.append("")
    if prev and repo_url:
        lines.append(f"**Full Changelog**: {repo_url}/compare/{prev}...{tag}")
    return "\n".join(lines).rstrip() + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate release notes from commits since the previous release."
    )
    parser.add_argument(
        "--tag",
        help="release tag; defaults to the nearest tag reachable from HEAD",
    )
    parser.add_argument(
        "--prev",
        help="previous release tag; defaults to the nearest tag before --tag",
    )
    parser.add_argument(
        "--repo-url",
        help=(
            "repository web URL for links; defaults to GITHUB_SERVER_URL and "
            "GITHUB_REPOSITORY, without which links are omitted"
        ),
    )
    args = parser.parse_args()

    tag = resolve_tag(args.tag)
    prev = resolve_prev(tag, args.prev)
    repo_url = resolve_repo_url(args.repo_url)
    rev_range = f"{prev}..{tag}" if prev else tag
    sys.stdout.write(render(prev, tag, repo_url, read_commits(rev_range)))


if __name__ == "__main__":
    main()
