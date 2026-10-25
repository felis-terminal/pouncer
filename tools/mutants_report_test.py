from __future__ import annotations

from pathlib import Path

from mutants_report import MARKER, body


def out_dir(root: Path, **outcomes: list[str]) -> Path:
    for name, entries in outcomes.items():
        (root / f"{name}.txt").write_text("".join(f"{e}\n" for e in entries))
    return root


def test_a_diff_without_mutants_says_so_and_reports_no_survivor(tmp_path: Path) -> None:
    text, survived = body(tmp_path / "absent")
    assert "No mutants" in text
    assert not survived


def test_all_caught_counts_them_and_lists_nothing(tmp_path: Path) -> None:
    text, survived = body(out_dir(tmp_path, caught=["a", "b"], unviable=["c"]))
    assert "3 mutants: 2 caught, 0 missed, 0 timed out, 1 unviable." in text
    assert "**Missed**" not in text
    assert not survived


def test_missed_and_timed_out_mutants_are_listed(tmp_path: Path) -> None:
    text, survived = body(
        out_dir(
            tmp_path,
            caught=["c"],
            missed=["src/a.rs:1:2: replace < with <= in f"],
            timeout=["src/b.rs:3:4: replace += with -= in g"],
        )
    )
    assert "- `src/a.rs:1:2: replace < with <= in f`" in text
    assert "- `src/b.rs:3:4: replace += with -= in g`" in text
    assert survived


def test_a_timeout_alone_is_not_a_survivor(tmp_path: Path) -> None:
    # A mutant that loops forever never lets the suite pass.
    _, survived = body(out_dir(tmp_path, caught=["c"], timeout=["t"]))
    assert not survived


def test_the_marker_leads_every_report_so_it_can_be_found_again(tmp_path: Path) -> None:
    for out in (tmp_path / "absent", out_dir(tmp_path, missed=["m"])):
        assert body(out)[0].startswith(MARKER)
