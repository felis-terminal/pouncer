from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

from release import ReleaseError, check, notes

CHANGELOG = """# Changelog

## [Unreleased]

### Added

- Something new.

## [0.2.0] - 2026-10-20

### Fixed

- Something broken.

## [0.1.0] - 2026-09-26

Initial release.

[unreleased]: https://example.invalid/compare/v0.2.0...HEAD
[0.2.0]: https://example.invalid/compare/v0.1.0...v0.2.0
[0.1.0]: https://example.invalid/releases/tag/v0.1.0
"""


class Repo:
    def __init__(self, root: Path, version: str = "0.2.0", locked: str | None = None):
        self.root = root
        (root / "Cargo.toml").write_text(
            f'[package]\nname = "pouncer"\nversion = "{version}"\n'
        )
        (root / "Cargo.lock").write_text(
            "version = 4\n\n"
            f'[[package]]\nname = "pouncer"\nversion = "{locked or version}"\n'
        )
        (root / "CHANGELOG.md").write_text(CHANGELOG)

    def git(self, *args: str) -> str:
        return subprocess.run(
            ["git", "-C", str(self.root), *args],
            capture_output=True,
            text=True,
            check=True,
            env=os.environ
            | {
                "GIT_AUTHOR_NAME": "t",
                "GIT_AUTHOR_EMAIL": "t@example.invalid",
                "GIT_COMMITTER_NAME": "t",
                "GIT_COMMITTER_EMAIL": "t@example.invalid",
                "GIT_CONFIG_GLOBAL": "/dev/null",
                "GIT_CONFIG_NOSYSTEM": "1",
            },
        ).stdout.strip()

    def commit(self) -> str:
        self.git("init", "-q")
        self.git("add", ".")
        self.git("commit", "-q", "-m", "release")
        return self.git("rev-parse", "HEAD")


def test_accepts_a_final_tag_that_matches_the_manifest_and_a_dated_section(
    tmp_path: Path,
) -> None:
    Repo(tmp_path)
    check(tmp_path, "v0.2.0")


@pytest.mark.parametrize("tag", ["0.2.0", "v0.2", "v0.2.0-beta.1", "v0.2.0-rc1"])
def test_rejects_a_tag_that_is_not_semver_with_a_v_prefix(
    tmp_path: Path, tag: str
) -> None:
    Repo(tmp_path)
    with pytest.raises(ReleaseError):
        check(tmp_path, tag)


def test_rejects_a_tag_whose_version_differs_from_cargo_toml(tmp_path: Path) -> None:
    Repo(tmp_path, version="0.2.1")
    with pytest.raises(ReleaseError, match="Cargo.toml says 0.2.1"):
        check(tmp_path, "v0.2.0")


def test_rejects_a_stale_cargo_lock(tmp_path: Path) -> None:
    Repo(tmp_path, locked="0.1.0")
    with pytest.raises(ReleaseError, match="Cargo.lock"):
        check(tmp_path, "v0.2.0")


def test_rejects_a_final_tag_without_a_dated_changelog_section(tmp_path: Path) -> None:
    Repo(tmp_path, version="0.3.0")
    with pytest.raises(ReleaseError, match="no dated"):
        check(tmp_path, "v0.3.0")


def test_accepts_a_candidate_before_its_changelog_section_exists(
    tmp_path: Path,
) -> None:
    Repo(tmp_path, version="0.3.0")
    check(tmp_path, "v0.3.0-rc.1")


def test_accepts_an_annotated_tag_on_the_given_revision(tmp_path: Path) -> None:
    repo = Repo(tmp_path)
    sha = repo.commit()
    repo.git("tag", "-a", "v0.2.0", "-m", "v0.2.0")
    check(tmp_path, "v0.2.0", sha)


def test_rejects_a_lightweight_tag(tmp_path: Path) -> None:
    repo = Repo(tmp_path)
    sha = repo.commit()
    repo.git("tag", "v0.2.0")
    with pytest.raises(ReleaseError, match="not an annotated tag"):
        check(tmp_path, "v0.2.0", sha)


def test_rejects_a_tag_on_another_revision(tmp_path: Path) -> None:
    repo = Repo(tmp_path)
    repo.commit()
    repo.git("tag", "-a", "v0.2.0", "-m", "v0.2.0")
    with pytest.raises(ReleaseError, match="points at"):
        check(tmp_path, "v0.2.0", "0" * 40)


@pytest.fixture
def released(tmp_path: Path) -> Path:
    Repo(tmp_path)
    return tmp_path


def test_final_notes_carry_only_that_version_section(released: Path) -> None:
    text = notes(released, "v0.2.0")
    assert "- Something broken." in text
    assert "Something new" not in text
    assert "Initial release" not in text


def test_notes_drop_the_link_references(released: Path) -> None:
    text = notes(released, "v0.1.0")
    assert "Initial release." in text
    assert "example.invalid" not in text


def test_candidate_notes_carry_the_unreleased_section(released: Path) -> None:
    text = notes(released, "v0.3.0-rc.1")
    assert "- Something new." in text
    assert "Something broken" not in text


def test_notes_name_the_tag_in_the_install_commands(released: Path) -> None:
    text = notes(released, "v0.2.0")
    assert "cargo install pouncer --version 0.2.0" in text
    assert "nix run github:felis-terminal/pouncer/v0.2.0" in text
