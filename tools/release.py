#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

USAGE = """usage: tools/release.py check <tag> [<sha>]
       tools/release.py notes <tag>
       tools/release.py forgejo <tag>

check    refuses a tag that disagrees with Cargo.toml, Cargo.lock or
         CHANGELOG.md; with <sha>, also one that is lightweight or names
         another revision.
notes    prints the release notes: the tag's CHANGELOG section, or
         Unreleased for a candidate (vX.Y.Z-rc.N).
forgejo  creates the Forgejo release for <tag>, with those notes, unless
         it exists. Reads GITHUB_SERVER_URL, GITHUB_REPOSITORY and
         RELEASE_TOKEN.
"""

ROOT = Path(__file__).resolve().parent.parent
TAG = re.compile(r"v(\d+\.\d+\.\d+)(-rc\.\d+)?")


class ReleaseError(Exception):
    pass


def parse_tag(tag: str) -> tuple[str, bool]:
    match = TAG.fullmatch(tag)
    if match is None:
        raise ReleaseError(f"{tag} is not vX.Y.Z or vX.Y.Z-rc.N")
    return match[1], match[2] is not None


def section(changelog: str, heading: str) -> str:
    lines = changelog.splitlines()
    start = next((i for i, line in enumerate(lines) if line.startswith(heading)), None)
    if start is None:
        raise ReleaseError(f"CHANGELOG.md has no {heading} section")
    body = []
    for line in lines[start + 1 :]:
        if line.startswith("## ["):
            break
        if re.match(r"\[[^]]+\]: ", line):
            continue
        body.append(line)
    return "\n".join(body).strip()


def check(root: Path, tag: str, sha: str | None = None) -> None:
    version, candidate = parse_tag(tag)

    manifest = tomllib.loads((root / "Cargo.toml").read_text())["package"]
    if manifest["version"] != version:
        raise ReleaseError(
            f"{tag} names {version}, Cargo.toml says {manifest['version']}"
        )
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    locked = [p["version"] for p in lock["package"] if p["name"] == manifest["name"]]
    if locked != [version]:
        raise ReleaseError(f"Cargo.lock records {manifest['name']} at {locked}")

    changelog = (root / "CHANGELOG.md").read_text()
    if not candidate:
        dated = re.compile(rf"## \[{re.escape(version)}\] - \d{{4}}-\d{{2}}-\d{{2}}$")
        if not any(dated.match(line) for line in changelog.splitlines()):
            raise ReleaseError(
                f"CHANGELOG.md has no dated ## [{version}] - YYYY-MM-DD section"
            )
        if not re.search(rf"^\[{re.escape(version)}\]: ", changelog, re.MULTILINE):
            raise ReleaseError(f"CHANGELOG.md has no [{version}]: link")

    if sha is not None:
        kind = git(root, "cat-file", "-t", f"refs/tags/{tag}")
        if kind != "tag":
            raise ReleaseError(f"{tag} is a {kind}, not an annotated tag")
        commit = git(root, "rev-parse", f"refs/tags/{tag}^{{commit}}")
        if commit != sha:
            raise ReleaseError(f"{tag} points at {commit}, not {sha}")


def notes(root: Path, tag: str) -> str:
    version, candidate = parse_tag(tag)
    changelog = (root / "CHANGELOG.md").read_text()
    changes = section(changelog, "## [Unreleased]" if candidate else f"## [{version}]")
    text = (
        "## Install\n\n"
        f"- **crates.io:** `cargo install pouncer --version {version}`\n"
        f"- **Nix:** `nix run github:felis-terminal/pouncer/{tag}`\n"
    )
    if changes:
        text += f"\n## Changes\n\n{changes}\n"
    return text


def git(root: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), *args], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise ReleaseError(f"git {' '.join(args)}: {result.stderr.strip()}")
    return result.stdout.strip()


def forgejo(root: Path, tag: str) -> None:
    _, candidate = parse_tag(tag)
    missing = [
        name
        for name in ("GITHUB_SERVER_URL", "GITHUB_REPOSITORY", "RELEASE_TOKEN")
        if not os.environ.get(name)
    ]
    if missing:
        raise ReleaseError(f"the environment lacks {', '.join(missing)}")
    api = (
        f"{os.environ['GITHUB_SERVER_URL'].rstrip('/')}/api/v1/repos/"
        f"{os.environ['GITHUB_REPOSITORY']}/releases"
    )
    headers = {
        "Authorization": f"token {os.environ['RELEASE_TOKEN']}",
        "Content-Type": "application/json",
        # git.natsukium.com answers urllib's default Python-urllib agent with 403.
        "User-Agent": "pouncer-release",
    }

    lookup = urllib.request.Request(
        f"{api}/tags/{urllib.parse.quote(tag, safe='')}", headers=headers
    )
    try:
        with urllib.request.urlopen(lookup):
            print(f"release: Forgejo already has {tag}")
            return
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise ReleaseError(
                f"looking up {tag} answered HTTP {error.code}"
            ) from error

    body = notes(root, tag)
    print(body, end="")
    payload = {
        "tag_name": tag,
        "name": f"pouncer {tag}",
        "body": body,
        "prerelease": candidate,
    }
    create = urllib.request.Request(
        api, data=json.dumps(payload).encode(), headers=headers, method="POST"
    )
    try:
        with urllib.request.urlopen(create):
            print(f"release: published {tag}")
    except urllib.error.HTTPError as error:
        raise ReleaseError(f"creating {tag} answered HTTP {error.code}") from error


def main(argv: list[str]) -> int:
    try:
        match argv:
            case ["check", tag]:
                check(ROOT, tag)
            case ["check", tag, sha]:
                check(ROOT, tag, sha)
            case ["notes", tag]:
                print(notes(ROOT, tag), end="")
            case ["forgejo", tag]:
                forgejo(ROOT, tag)
            case _:
                print(USAGE, file=sys.stderr, end="")
                return 64
    except ReleaseError as error:
        print(f"release: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
