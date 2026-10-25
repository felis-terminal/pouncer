---
title: Design
---

`pouncer` is a standalone CLI that reads terminal text on stdin (or from a file argument), overlays single-keystroke
labels on the substrings a user describes, and on selection copies, opens, or pipes the picked value. This document
records what `pouncer` is and is not, and why each boundary and implementation choice sits where it does, so that later
changes do not undo an intentional trade-off or re-discover a problem already solved here.

`pouncer` takes the interaction model of [kitty's `hints` kitten][kitty-hints] and [tmux-thumbs][thumbs] — overlay a
one- or two-key label on each visible match in place and pick it directly, rather than typing a query into a separate
filter list like `fzf` — and unbundles it into a terminal-agnostic CLI. Like kitty's `hints`, it reads terminal text
from a pipe, takes all of its configuration from the launching key binding, and matches only the patterns the user asks
for; unlike `tmux-thumbs`, it has no forced built-in patterns and runs its input through a real VT parser and cell grid
so labels stay aligned across ANSI escapes, wide glyphs, combining marks, tabs, and soft-wrapped rows.

## Principles and non-goals

1. **Terminal-agnostic pipe.** `pouncer` reads from a pipe or file and never talks to a terminal's PTY, daemon, or IPC
   protocol to fetch content. Capturing scrollback belongs to the producer (`tmux capture-pane -p`, `kitty @ get-text`,
   [felis][felis]' pipe action, or a bare `cat`). `pouncer` links the `felis-vt` and `felis-grid` library crates to
   parse the byte stream it receives, without requiring or bundling any particular terminal.
2. **One-shot hint picker, not a copy mode or pager.** `pouncer` detects matches, draws labels, and returns a pick. It
   has no modal cursor, visual selection, or incremental search; persistent navigation belongs in `less`, `$EDITOR`, or
   a terminal's own copy mode.
3. **No config file.** Every option lives on the command line of the key binding or shell alias that launches `pouncer`.
   [README.md](../README.md) and `pouncer --help` document the flags.
4. **No embedded scripting.** Follow-on work is delegated to a command (`--action '<cmd>'`) rather than a Lua, Python,
   or Wasm plugin host.
5. **Declared patterns only.** Nothing matches unless requested via `--regex` or an opt-in `--preset`. All patterns in a
   single invocation share the same `--action` (and optional `--upper-action`).

## Architecture

```
producer ──pipe──▶ source ──▶ matcher ──▶ hint assigner ──▶ renderer ──▶ action
 (tmux/kitty/…)    (rows of    (regex)     (collision-free   (redraw +    (copy/
                    cells)                  labels)           overlay)      open/cmd)
```

### Source abstraction

A `source` yields rows of cells with their display columns and a `continued` bit taken from `felis-grid`'s soft-wrap
flag. Raw bytes from stdin or a file path argument are fed through `felis-vt` (a Williams-DFA VT parser) into a
`felis-grid` cell grid, which resolves ANSI/OSC escapes, UTF-8 boundaries, wide characters, combining marks, and
autowrap before `pouncer` inspects a single row. Reusing `felis-vt` and `felis-grid` as libraries means following their
pre-1.0 releases and pulling in grid features `pouncer` does not use (such as image storage and damage tracking), in
exchange for not maintaining a second VT parser and width table.

Scrollback can arrive on stdin (for shell pipelines, `tmux capture-pane | pouncer`, or kitty's `--stdin-source`) or as a
file path argument (for producers like [felis][felis]' pipe action that write the captured region to a temp file and
spawn `pouncer` inside an interactive PTY). Both paths feed the same byte parser. Keyboard input and overlay rendering
always go directly to the controlling terminal: `/dev/tty` on Unix, or `CONIN$` and `CONOUT$` on Windows. On Windows,
the overlay is written with `WriteConsoleW` rather than raw bytes so the console's active code page (such as CP932 or
CP437) cannot corrupt UTF-8 output.

The grid width defaults to the terminal's width, which matches the source pane when `pouncer` runs in place. When
`pouncer` runs inside a narrower container such as a tmux popup, the producer passes the original pane width via
`--cols` so soft-wraps and labels stay where they were in the source. Cells and labels past the terminal's right edge
are clipped so the terminal does not wrap them onto the next row.

### Core stages

- `matcher`: applies the user's `--regex` patterns and opt-in `--preset` patterns to each logical row (a physical row
  joined with any consecutive soft-wrap continuation rows) using the linear-time, ReDoS-safe `regex` crate. On
  overlapping matches that start at the same cell, `--regex` beats any preset and earlier presets beat later ones.
- `hint assigner`: assigns short, prefix-free labels of at most two characters from the configured alphabet (see "Label
  assignment"). With an alphabet of `N` keys, up to `N²` matches receive labels; any excess count is reported on stderr.
- `renderer`: repaints the captured cells with their preserved SGR attributes and draws the labels on top, opening at
  the tail of the input where recent command output sits. Because both the background text and the labels are drawn from
  the same cell grid, label positions always match the rendered text.
- `action`: prints the picked value(s) to stdout by default, copies them to the clipboard via OSC 52, opens them with
  the platform handler (`xdg-open`, `open`, or `ShellExecuteExW`), or passes them as arguments to a command template.

## Alignment

Computing display columns by running a regex over raw input lines and estimating widths from byte or character offsets
breaks whenever the stream contains ANSI/OSC escapes, multibyte characters, zero-width combining marks, East-Asian wide
glyphs, tabs, or soft-wrapped rows.

`pouncer` avoids this class of bug by not doing byte-to-column accounting itself:

- `felis-vt` parses ANSI/OSC/UTF-8 (and Kitty graphics, text-sizing, and bidi) the way a terminal does, so escape bytes
  never leak into match offsets.
- `felis-grid` lays the parsed stream onto cells with real wide-character, combining-mark, tab, and soft-wrap handling,
  and exposes scrollback rows as cell slices. The display column of a match is then a cell index, never a recomputation
  from bytes.
- Redraw-style rendering repaints those cells and overlays labels, so labels stay consistent with the repaint.

For any producer, escapes are handled completely, cells come from a real VT engine, and labels stay consistent with the
repaint, so the byte-counting bug class is gone whoever feeds the pipe. Width itself still comes from `felis-grid`'s
`unicode-width`, a `wcwidth`-class heuristic; because both text and labels are painted from the same grid, any width
disagreement with the host terminal is confined to the repainted row.

## Soft-wrapped matches

Whether a soft-wrapped match can be detected at all depends on what the producer pipes in. This limit comes from the
pipe and is separate from rendering:

- Physical rows (`tmux capture-pane -p`, raw `cat`): a line that wrapped in the source already has a hard `\n` at the
  wrap point, so the match is split across two input lines and a regex that does not span `\n` won't find it.
  Information the producer discarded cannot be recovered.
- Logical lines (`tmux capture-pane -J`, felis' pipe action): the match is one unit; `felis-grid` then re-wraps for
  display, and the problem reduces to rendering.

`pouncer` does not re-stitch physical rows; it asks for a logical source instead. Joining a row filled to `cols` with no
trailing space onto the next one before matching would be a guess, and it would mis-join independent rows that happen to
reach the edge. The rest of `pouncer` takes column facts from `felis` instead of estimating them, and re-stitching by
estimate would bring back the class of error the tool is meant to avoid. `pouncer` accepts the per-physical-row loss on
a physical-row source and relies on the producer for logical lines.

Matching happens on logical rows, before re-wrap, and needs no heuristic because `felis` already records the wrap:
`Grid::row_soft_wrap_continued(row)` is `true` exactly when a row is an autowrap continuation of the row above.
`source::Row` carries that bit, and `matcher::find` matches once per maximal run of continued rows, joined with nothing
between them, so a wrapped URL is one match whose value has no newline.

A match is anchored at the row and column of its first glyph. `matcher::Match` records one column span per physical row,
so a match that crosses a soft wrap is highlighted across each row in reverse video (which remains visible over any
background color, unlike a fixed highlight color).

The label itself is laid onto the match's cells in reading order, continuing across the wrap if needed. If a label is
longer than its match, it extends past the match's end, shifting left if it would cross the right edge of the screen.
When a label boundary splits a wide glyph, the other half of the glyph is blanked in the label's colors. A label that
would be cut by the viewport edge or is wider than the screen is omitted so a truncated fragment cannot be misread as a
different key.

## Label assignment

Given N matches and a k-character alphabet, the labeler assigns each match a label so that (1) no label is a prefix of
another, so the pick resolves as soon as the last needed key is pressed and never needs `Enter`; (2) total and maximum
keystrokes are minimized; and (3) labels are stable and home-row-friendly. Such a labeling is a k-ary prefix code,
equivalent to the leaves of a full k-ary tree with N leaves.

`pouncer` consumes single letters from the front of the alphabet first and sacrifices single letters from the tail to
expand each into k two-character hints only as overflow demands (`abcd` with 6 matches gives `[a, b, c, da, db, dc]`),
stopping at two levels. Labels are one or two characters, so a k-letter alphabet labels at most k² matches (676 for the
default 26 letters). For a key-binding tool, capping a pick at two keys matters more than labelling an unbounded number
of matches.

`pouncer`'s default alphabet puts the QWERTY home row first (`asdfghjklqwertyuiopzxcvbnm`) so the shortest labels land
on the easiest keys. `--alphabet CHARS` takes any other set of literal ASCII graphic characters and is rejected at
startup if it repeats a character, holds something untypeable, or has fewer than two. The shortest labels go to the
first matches; `--reverse` gives them to the last (bottom-up).

## Edge-case handling

1. Tabs are expanded by `felis-vt` / `felis-grid` as part of normal VT processing, and the renderer steps over column
   gaps with explicit cursor moves.
2. Input taller than the screen is kept in full. `pouncer` labels every match in it and opens the overlay at the tail,
   where the most recent output of a just-run command is. The arrows, PgUp/PgDn, Home/End, and Ctrl-d/u/f/b scroll the
   rest into view. Input past the grid's 65535-row limit is dropped, and `pouncer` reports that on stderr instead of
   losing it silently.
3. Zero-width matches are dropped before labelling, and the `regex` crate's iterators step past an empty match so a user
   pattern that matches the empty string cannot loop forever.
4. ANSI escapes are stripped before matching through cell parsing: `pouncer` matches against the text of the parsed
   cells, so escapes never reach the regex, and invalid UTF-8 is replaced by `felis-vt` without panicking.
5. The repaint preserves styles and sanitizes control characters. Because `pouncer` parses the input through `felis-vt`,
   each cell retains its SGR pen and `--dim` composes with those styles. C1 control characters (`U+0080` to `U+009F`,
   which `felis-vt` passes through as text) are repainted as `?` so piped input cannot trigger escape sequences on
   terminals that interpret UTF-8 C1, while the matched value itself stays untouched.
6. Overlay teardown restores the terminal on both user cancel and termination signals. Pressing `Esc`, `Ctrl-c`, or
   `Ctrl-g` exits with status `130`. If the process receives `SIGTERM`, `SIGHUP`, `SIGINT`, or `SIGQUIT`, it restores
   the terminal from raw mode and the alternate screen before re-raising the signal.
7. OSC 52 is the only portable clipboard path. Probing for `wl-copy`, `pbcopy`, or `xclip` only reaches the local host
   and fails over SSH. `copy` writes an OSC 52 sequence to `/dev/tty` (`CONOUT$` on Windows) so the terminal displaying
   the overlay sets the clipboard.
8. Picked values are never interpolated into shell code. On Unix, `--action <cmd>` runs `sh -c` with `{}` replaced by
   `"$@"` and passes picked values as positional parameters, so a match containing `; rm -rf .` is treated purely as
   data. On Windows, where `cmd.exe` and PowerShell re-parse their argument strings as script, `pouncer` splits the
   command template using the C runtime's argv rules, expands `{}` into separate arguments, resolves the program on
   `PATH` with `PATHEXT` (ignoring the current working directory), and spawns it directly; `.bat`/`.cmd` targets use
   `std`'s `cmd.exe`-safe quoting and reject values that cannot be escaped safely. Similarly, `open` passes each value
   to `xdg-open`, `open`, or `ShellExecuteExW` as a file/URL parameter rather than a command line.
9. Duplicates are labeled per occurrence by default and collapsed with `--unique`. Labeling each occurrence lets the
   user pick a specific position on screen, whereas `--unique` treats identical values as a single target: if only one
   unique value exists on screen it is auto-picked, and in `--multi` mode its label toggles all occurrences together and
   passes the value to the action once.

[thumbs]: https://github.com/fcsonline/tmux-thumbs
[kitty-hints]: https://sw.kovidgoyal.net/kitty/kittens/hints/
[felis]: https://github.com/felis-terminal/felis
