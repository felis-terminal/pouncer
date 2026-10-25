# pouncer

Spot it, then pounce. `pouncer` is a standalone, terminal-agnostic hint picker for scrollback: pipe text in, and every
match you describe gets a one- or two-key label. Press the label and `pouncer` lands on that match, then copies it,
opens it, or hands it to a command.

The target is already on your screen, so you pick it directly instead of typing a filter or scrolling a list.

In [felis][felis], one key binding pipes the screen into `pouncer`:

```toml
# ~/.config/felis/config.toml
# Every URL on screen gets a label: a lowercase label copies, an UPPERCASE one opens
[keymap."ctrl+shift+u"]
kind = "pipe"
source = "visible"
ansi = true
target = { command = ["pouncer", "--preset", "url", "--action", "copy", "--upper-action", "open"] }
```

Any other terminal, multiplexer, or command works the same way through a pipe:

```sh
# Every URL on the tmux pane gets a label: open the one you point at
tmux capture-pane -p -e -J | pouncer --preset url --action open

# A table full of container IDs: stop the one you pick, not the whole row
docker ps | pouncer --regex '\b[0-9a-f]{12}\b' --action 'docker stop {}'

# PR numbers buried in the log: copy just the digits via a capture group
git log --oneline | pouncer --regex '#(\d+)' --capture 1 --action copy
```

Labels go on matches, not lines, so several IDs or URLs on one line can each be picked.

If you know [tmux-thumbs][tmux-thumbs]' standalone mode or kitty's [`hints`][kitty-hints] kitten, `pouncer` is the same
single-keystroke interaction taken out of them: it runs on any terminal, reads from any producer, and takes all of its
configuration from the key binding that launches it.

## Install

`pouncer` runs on Linux, macOS, and Windows.

With [Nix][nix] (flakes enabled), run `pouncer` straight from the repo without installing anything:

```sh
git log --oneline --color=always | nix run github:felis-terminal/pouncer -- --preset sha
```

Build the binary into `./result/bin/pouncer`, or drop it into your profile:

```sh
nix build github:felis-terminal/pouncer   # -> ./result/bin/pouncer
nix profile install github:felis-terminal/pouncer
```

To pin it in your own flake, add `pouncer` as an input and use its `packages.<system>.default`:

```nix
{
  inputs.pouncer.url = "github:felis-terminal/pouncer";
  # e.g. home-manager: home.packages = [ inputs.pouncer.packages.${pkgs.system}.default ];
}
```

With [Homebrew][homebrew], the felis tap installs a prebuilt bottle on Apple silicon (macOS 15 or later) and x86_64
Linux, and builds from source elsewhere:

```sh
brew install felis-terminal/tap/pouncer
```

Or install from crates.io with Rust (>= 1.88; see `rust-version` in [Cargo.toml](Cargo.toml)):

```sh
cargo install pouncer
```

The Nix and Homebrew packages install `pouncer(1)` and bash/zsh/fish completions. A cargo build prints them on demand
instead:

```sh
pouncer --generate man > ~/.local/share/man/man1/pouncer.1
pouncer --generate complete-fish > ~/.config/fish/completions/pouncer.fish   # or complete-bash, complete-zsh
```

## Usage

### Configuration

There is no config file. Every option goes on the command line of the key binding or shell alias that launches
`pouncer`. One invocation does one job, so bind one key per job.

### Matching and presets

Describe what to label with `-e`/`--regex` (repeatable, [Rust regex syntax][regex-syntax]: no lookaround or
backreferences) and any opt-in `--preset` (comma-separated or repeatable).

Where two patterns start on the same cell, a `--regex` beats any preset, and among presets the one named first wins. For
example, `--preset filepos,path` matches `src/main.rs:12` as a file position rather than stopping at the path
`src/main.rs`.

| Preset     | Matches                                         | Example                                | Notes                                                               |
| ---------- | ----------------------------------------------- | -------------------------------------- | ------------------------------------------------------------------- |
| `url`      | `http://`, `https://`, `ftp://`, `file://` URLs | `https://example.com/a`                | Stops at whitespace and surrounding `)]>"'}` delimiters             |
| `email`    | Email addresses                                 | `dev+tag@example.org`                  | Standard `local@domain.tld`                                         |
| `ipv4`     | IPv4 dotted quads                               | `192.168.0.1`                          | Word-bounded                                                        |
| `ipv6`     | IPv6 hextet addresses                           | `2001:db8::1`                          | Supports internal `::` compression                                  |
| `path`     | File paths with at least one `/`                | `/usr/bin`, `./src/main.rs`, `~/cfg`   | Bare words without `/` are never matched                            |
| `sha`      | 7–40 lowercase hex digits                       | `1a2b3c4`, `deadbeef...`               | Pair with `--min-len` on noisy hex output                           |
| `uuid`     | Canonical 8-4-4-4-12 UUIDs                      | `550e8400-e29b-41d4-a716-446655440000` | Case-insensitive                                                    |
| `filepos`  | `path:line[:col]` positions                     | `src/main.rs:140:5`, `Makefile:12`     | Ignores clock times (`12:30:45`), `ip:port`, and `localhost:80`     |
| `hostport` | `host:port`, `ipv4:port`, `[ipv6]:port`         | `localhost:8080`, `[::1]:443`          | Requires a letter in hostnames so `12:30` is ignored                |
| `hex`      | `#rgb` and `#rrggbb` color codes                | `#ff8800`, `#abc`                      | Omits 4/8-digit forms so `#1234` issue numbers aren't matched       |
| `mdlink`   | Markdown links `[text](url)`                    | `[docs](https://x.io/a)`               | Capture group 1 is the URL (`--capture 1` extracts just the target) |
| `jwt`      | Base64url-encoded JWT tokens                    | `eyJhbGci...`                          | Matches `header.payload.[signature]` starting with `eyJ`            |
| `pod`      | Kubernetes Deployment pod names                 | `web-7d4b9c8f6d-x2kq9`                 | Matches `<name>-<replicaset-hash>-<pod-suffix>`                     |
| `number`   | Standalone integer runs                         | `42`, `8080`                           | Word-bounded; ignores digits embedded in words (`1500ms`, `1a2b`)   |

Additional matching flags:

- `--capture N` extracts capture group `N` as the picked value while the label still anchors on the whole match; every
  pattern must have group `N`.
- `--min-len N` drops matches narrower than `N` display columns (measured on the whole match across soft-wrap).
- `--unique` gives identical match values one shared label, and treats them as one target: with `--multi` the label
  selects them together and the action receives the value once.
- `--alphabet CHARS` sets the label keys, earliest on the shortest labels (default: QWERTY home row first,
  `asdfghjklqwertyuiopzxcvbnm`). Labels are at most two keys, so `N` characters label `N×N` matches.
- `--reverse` hands the shortest labels to the last matches (bottom-up) instead of the first.

### Actions and picking

Typing a label picks its match immediately. Labels are prefix-free, so no `Enter` is needed. When there is only one
match, `pouncer` picks it without showing the overlay, unless `--upper-action` is set.

`--action` decides what happens to the picked value:

- Omitted (the default): each picked value is printed on its own line to stdout, so `pouncer` works inside `$(...)` or a
  pipeline.
- `copy`: sets the clipboard via OSC 52 written to the terminal, which works over SSH and inside tmux (with
  `set-clipboard` enabled).
- `open`: passes the picked value(s) to `xdg-open` (`open` on macOS, the default handler on Windows).
- `<command>`: runs the command with `{}` replaced by the picked value(s) as separate positional arguments (`sh -c` with
  `"$@"` on Unix; direct process execution on Windows). Picked text is never interpolated into a shell script, so
  characters in a match cannot inject commands.
  - On Windows, `<command>` is split like a standard C command line (double quotes group; single quotes are literal) and
    resolved on `PATH` using `PATHEXT` (`code` finds `code.cmd`). Simple commands like `code -g {}` or `git show {}`
    work on every platform; for shell pipelines on Windows, pipe the default stdout output into PowerShell:
    `pouncer --preset url | ForEach-Object { code -g $_ }`.

Two flags extend this:

- `--upper-action ACTION` binds a second action to the uppercase form of each label (`Shift+label`), so one key binding
  can both copy and open. It applies to single picks only and cannot be combined with `--multi`.
- `-m`, `--multi` makes labels toggle selection instead of exiting; `Enter` confirms and runs the action once with all
  selected values.

### Keys and exit status

| Key                                                            | Action                                                          |
| -------------------------------------------------------------- | --------------------------------------------------------------- |
| `label`                                                        | Pick match (with `--multi`: toggle selection)                   |
| `Shift+label`                                                  | Pick match and run `--upper-action`                             |
| `Enter`                                                        | Confirm selection in `--multi` mode                             |
| `Backspace`                                                    | Undo the last typed label character                             |
| `Esc`, `Ctrl-c`, `Ctrl-g`                                      | Cancel and exit 130                                             |
| `Up`/`Down`, `PgUp`/`PgDn`, `Home`/`End`, `Ctrl-d`/`u`/`f`/`b` | Scroll when input is taller than the screen (opens at the tail) |

| Exit Code | Meaning                                     |
| --------- | ------------------------------------------- |
| `0`       | Match picked and action succeeded           |
| `1`       | No matches found, or runtime / action error |
| `2`       | Invalid command-line arguments              |
| `130`     | Cancelled by user                           |

### Input, soft-wrap, and colors

`pouncer` reads scrollback from stdin or from a file path argument (`pouncer --preset url FILE`, with `-` meaning
stdin). It reads keys from, and draws on, the terminal itself (`/dev/tty` on Unix, or `CONIN$`/`CONOUT$` on Windows).

How you capture the text affects what the overlay can show:

- Keep logical lines (`tmux capture-pane -J`, [felis][felis]' pipe action). `pouncer` matches across soft-wrapped rows,
  so a long URL that wraps at the right edge is found whole. If a producer turns soft-wraps into hard newlines before
  piping, a match split across them cannot be rejoined.
- Pass `--cols N` when the geometry differs. If `pouncer` runs in a window narrower than the captured source, such as a
  tmux popup, pass the source pane's width with `--cols` so soft-wraps break where they originally did. Columns past the
  window's right edge are clipped, and a match that starts past it gets no label.
- Keep ANSI SGR escapes (`tmux capture-pane -e`, `kitty @ get-text --ansi`, `--color=always`, [felis][felis]' `ansi`
  pipe). `pouncer` repaints the captured cells with their original colors, bold, italic, underline shape and color, and
  reverse video, then highlights each match in reverse video across soft wraps.

Overlay colors can be adjusted on the command line: `--dim` dims the repainted background text so labels and matches
stand out, and `--hint-fg`, `--hint-bg`, `--select-fg`, and `--select-bg` set the label colors (accepting color names
like `red` or `dark_blue`, ANSI indices `0` to `255`, or `#rrggbb`).

## Recipes

### Git

```sh
# Pick a commit hash from the log and inspect it
git log --oneline -n 30 --color=always | pouncer --preset sha --reverse --action 'git show {}'

# Stage one or more modified/untracked files from git status
git -c color.status=always status -s | pouncer -e '\S+$' --multi --action 'git add -- {}'

# Switch to a branch picked from git branch output
git branch -a --color=always | pouncer -e '[\w./-]+' --min-len 3 --action 'git switch {}'

# Grab a PR/issue number from recent commits into the clipboard
git log --oneline --color=always | pouncer -e '#(\d+)' --capture 1 --action copy
```

### Editor jump (`file:line:col`)

Compiler errors, ripgrep output, and stack traces emit `file:line[:col]`. Use `--preset filepos` to jump straight to
that location in `$EDITOR`:

```sh
# Run cargo check (or rg -n, pytest, etc.) and open the chosen error in Neovim / Vim / VS Code
cargo check --color=always 2>&1 | pouncer --preset filepos --reverse --action '
  IFS=: read -r file line col <<EOF
$1
EOF
  nvim "+call cursor(${line:-1}, ${col:-1})" "$file"
'

# VS Code accepts file:line:col directly with --goto
rg -n --color=always "TODO" | pouncer --preset filepos --action 'code --goto {}'
```

### Docker and Kubernetes

```sh
# View logs for a running container (lowercase) or stop it (uppercase)
docker ps | pouncer -e '\b[0-9a-f]{12}\b' --action 'docker logs -f {}' --upper-action 'docker stop {}'

# Open a shell inside a Kubernetes Deployment pod, or tail its logs with Shift
kubectl get pods | pouncer --preset pod --action 'kubectl exec -it {} -- sh' --upper-action 'kubectl logs -f {}'

# Delete multiple evicted/failed pods at once
kubectl get pods | pouncer --preset pod --multi --action 'kubectl delete pod {}'
```

### Markdown and general pipelines

```sh
# Extract the target URL from a Markdown link [label](url) and open it
cat README.md | pouncer --preset mdlink --capture 1 --action open

# Command substitution: pass the picked IPv4 address to ssh
ssh "root@$(arp -a | pouncer --preset ipv4)"
```

### Terminal and multiplexer bindings

```toml
# ~/.config/felis/config.toml: pipe scrollback with ANSI colors preserved
# felis writes the region to a temp file and appends its path to the argv.
[keymap."ctrl+shift+p"]
kind = "pipe"
source = "scrollback"
ansi = true
target = { command = ["pouncer", "--preset", "url,path,filepos", "--reverse", "--action", "copy", "--upper-action", "open"] }
```

```tmux
# ~/.tmux.conf: popup over the current pane's scrollback
# Lowercase copies via OSC 52; uppercase opens in the browser.
bind-key u display-popup -E -w 100% -h 100% \
  "tmux capture-pane -p -e -J -t #{pane_id} | pouncer --cols #{pane_width} --preset url,path,sha --reverse --action copy --upper-action open"
```

```conf
# ~/.config/kitty/kitty.conf: pipe scrollback with ANSI colors preserved
map kitty_mod+p launch --stdin-source=@screen_scrollback --stdin-add-formatting --type=overlay \
  pouncer --preset url,path,filepos --reverse --action copy --upper-action open
```

## Design

`pouncer` is written in Rust. Turning raw terminal bytes into correctly positioned cells is handled by the
[`felis-vt`][felis] VT parser and [`felis-grid`][felis] cell-grid crates, used as libraries, so `pouncer` does not
reimplement the byte and width math where alignment bugs come from.

[docs/design.md](docs/design.md) covers the architecture and the reasons behind `pouncer`'s design and implementation
choices.

[kitty-hints]: https://sw.kovidgoyal.net/kitty/kittens/hints/
[tmux-thumbs]: https://github.com/fcsonline/tmux-thumbs
[felis]: https://github.com/felis-terminal/felis
[homebrew]: https://brew.sh
[nix]: https://nixos.org/download/
[regex-syntax]: https://docs.rs/regex/latest/regex/#syntax
