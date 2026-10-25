#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

USAGE = """usage: tools/mutants_report.py body <mutants.out>
       tools/mutants_report.py comment <mutants.out> <pr-number>

body     prints the report for a cargo-mutants output directory.
comment  posts that report on the pull request, editing the earlier one
         in place. A diff with no survivors and no earlier report posts
         nothing. Reads GITHUB_SERVER_URL, GITHUB_REPOSITORY and
         MUTANTS_TOKEN.
"""

MARKER = "<!-- pouncer-mutants-report -->"


class ReportError(Exception):
    pass


def outcomes(out: Path, name: str) -> list[str]:
    path = out / f"{name}.txt"
    if not path.exists():
        return []
    return [line for line in path.read_text().splitlines() if line.strip()]


def body(out: Path) -> tuple[str, bool]:
    missed = outcomes(out, "missed")
    timeout = outcomes(out, "timeout")
    caught = outcomes(out, "caught")
    unviable = outcomes(out, "unviable")
    tested = len(missed) + len(timeout) + len(caught) + len(unviable)
    lines = [MARKER, "### cargo mutants on this diff", ""]
    if tested == 0:
        lines.append("No mutants in the lines this pull request changes.")
        return "\n".join(lines) + "\n", False
    lines.append(
        f"{tested} mutants: {len(caught)} caught, {len(missed)} missed, "
        f"{len(timeout)} timed out, {len(unviable)} unviable."
    )
    if missed:
        lines += ["", "**Missed**", ""]
        lines += [f"- `{entry}`" for entry in missed]
        lines += [
            "",
            "A missed mutant is a change no test notices: add or tighten a "
            "test, or leave it if the mutant behaves the same as the code.",
        ]
    if timeout:
        lines += ["", "**Timed out**", ""]
        lines += [f"- `{entry}`" for entry in timeout]
        lines += [
            "",
            "A timeout is usually a mutant that loops forever, which the suite "
            "did notice; one that is merely slow is worth a look.",
        ]
    return "\n".join(lines) + "\n", bool(missed)


def api(method: str, url: str, token: str, payload: dict | None = None) -> object:
    request = urllib.request.Request(
        url,
        data=None if payload is None else json.dumps(payload).encode(),
        headers={
            "Authorization": f"token {token}",
            "Content-Type": "application/json",
            # git.natsukium.com answers urllib's default Python-urllib agent with 403.
            "User-Agent": "pouncer-mutants",
        },
        method=method,
    )
    try:
        with urllib.request.urlopen(request) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        raise ReportError(f"{method} {url} answered HTTP {error.code}") from error


def comment(out: Path, pr: str) -> None:
    missing = [
        name
        for name in ("GITHUB_SERVER_URL", "GITHUB_REPOSITORY", "MUTANTS_TOKEN")
        if not os.environ.get(name)
    ]
    if missing:
        raise ReportError(f"the environment lacks {', '.join(missing)}")
    token = os.environ["MUTANTS_TOKEN"]
    repo = (
        f"{os.environ['GITHUB_SERVER_URL'].rstrip('/')}/api/v1/repos/"
        f"{os.environ['GITHUB_REPOSITORY']}"
    )
    text, survived = body(out)

    earlier = None
    page = 1
    while earlier is None:
        comments = api(
            "GET", f"{repo}/issues/{pr}/comments?page={page}&limit=50", token
        )
        if not comments:
            break
        earlier = next((c for c in comments if MARKER in c["body"]), None)
        page += 1

    if earlier is not None:
        api("PATCH", f"{repo}/issues/comments/{earlier['id']}", token, {"body": text})
        print(f"mutants: updated the report on #{pr}")
    elif survived:
        api("POST", f"{repo}/issues/{pr}/comments", token, {"body": text})
        print(f"mutants: reported on #{pr}")
    else:
        print("mutants: nothing survived and nothing to update")


def main(argv: list[str]) -> int:
    try:
        match argv:
            case ["body", out]:
                print(body(Path(out))[0], end="")
            case ["comment", out, pr]:
                comment(Path(out), pr)
            case _:
                print(USAGE, file=sys.stderr, end="")
                return 64
    except ReportError as error:
        print(f"mutants: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
