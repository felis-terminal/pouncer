//! Terminal-agnostic single-keystroke hint picker for scrollback.

mod action;
mod alphabet;
mod color;
mod interrupt;
mod label;
mod matcher;
mod picker;
mod preset;
mod source;
mod style;
mod terminal;
#[cfg(test)]
mod test_support;

use action::Action;
use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser};
use crossterm::style::Color;
use label::{Assignment, assign};
use std::io::{Read, Write};
use std::process::ExitCode;

// Each flag is an independent knob on one invocation, not a mode: a state
// machine would have to enumerate their product. The doc comments are --help
// text, where Markdown backticks would print literally.
#[allow(clippy::struct_excessive_bools, clippy::doc_markdown)]
/// Terminal-agnostic single-keystroke hint picker for scrollback.
///
/// Patterns are matched on logical rows, so a match that soft-wraps at the
/// grid's right edge is still found whole. That needs a source that pipes
/// logical lines: "tmux capture-pane -J", or felis' pipe action. A
/// physical-row source ("tmux capture-pane -p", or a "cat" of already-wrapped
/// output) has replaced the wrap with a newline before pouncer sees it, and a
/// match split across that newline cannot be recovered.
///
/// The repaint carries the source's colors when the producer kept them:
/// "tmux capture-pane -e", "kitty @ get-text --ansi", "--color=always" on a
/// plain pipe, felis' pipe action with "ansi". A producer that strips SGR
/// yields a plain repaint; nothing is guessed back.
#[derive(Parser)]
// bin_name, or clap names the program after argv[0]: `pouncer.exe` on Windows.
#[command(version, about, long_about, after_long_help = AFTER_LONG_HELP, bin_name = "pouncer")]
struct Cli {
    /// Regex pattern to match (repeatable).
    ///
    /// Rust regex syntax: no lookaround or backreferences. Nothing matches
    /// without --regex or --preset.
    #[arg(short = 'e', long = "regex", value_name = "RE")]
    regexes: Vec<String>,

    /// Curated pattern(s): url, email, ipv4, ipv6, path, sha, uuid, filepos,
    /// hostport, hex, mdlink, jwt, pod, number.
    ///
    /// Comma-separated and repeatable; off unless listed. Where two start on the same cell, a
    /// --regex beats any preset, and among presets the one named first wins:
    /// "filepos,path" keeps "src/main.rs:12" whole.
    #[arg(long = "preset", value_name = "NAME", value_delimiter = ',')]
    presets: Vec<String>,

    /// Pick capture group N instead of the whole match.
    ///
    /// The label still covers the whole match. Every pattern must have group N.
    #[arg(long, value_name = "N")]
    capture: Option<usize>,

    /// Drop matches narrower than N display columns.
    ///
    /// Measured on the whole match (across a soft wrap), not on the --capture
    /// value. Keeps a permissive pattern usable: "--preset sha" on a hex dump.
    #[arg(long, value_name = "N", default_value_t = 0)]
    min_len: u16,

    /// Characters labels are drawn from.
    ///
    /// Earlier characters make the shorter labels. Labels are at most two keys,
    /// so N characters label at most N×N matches.
    #[arg(long, default_value_t = String::from(alphabet::DEFAULT))]
    alphabet: String,

    /// Give the shortest labels to the last matches.
    #[arg(long)]
    reverse: bool,

    /// Share one label across identical match values.
    #[arg(long)]
    unique: bool,

    /// Pick several matches: a label toggles selection, Enter confirms the set.
    #[arg(short = 'm', long)]
    multi: bool,

    /// copy | open | <command with {}>. Absent prints the pick to stdout.
    ///
    /// "copy" sets the clipboard with OSC 52. "open" runs xdg-open (open on
    /// macOS, the default handler on Windows). Anything else is a command where
    /// {} stands for the picked value(s) as separate arguments: a shell command
    /// run with sh -c on Unix, and on Windows a program and its arguments, split
    /// like a C program's command line and run without a shell. All picks go
    /// to one run. Without --action each value is printed on its own line.
    #[arg(long, value_name = "ACTION")]
    action: Option<String>,

    /// Action for a label typed in uppercase, in the same form as --action.
    ///
    /// One binding then offers two actions. Single-pick only; rejected with
    /// --multi.
    #[arg(long, value_name = "ACTION")]
    upper_action: Option<String>,

    /// Grid width in columns for parsing and label placement.
    ///
    /// Defaults to the terminal width; set it when pouncer's terminal is not the source's
    /// width (a tmux popup narrower than the captured pane) so soft-wrap
    /// reconstructs as it did at the source. Columns past pouncer's own right
    /// edge are not drawn, and a match starting there gets no label.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..))]
    cols: Option<u16>,

    /// Hint label foreground. Default black.
    ///
    /// A color is a name (red, dark_blue, ...), an ANSI index 0-255, or
    /// #rrggbb.
    #[arg(long, value_name = "COLOR", value_parser = color::parse)]
    hint_fg: Option<Color>,

    /// Hint label background, as --hint-fg. Default green.
    #[arg(long, value_name = "COLOR", value_parser = color::parse)]
    hint_bg: Option<Color>,

    /// Selected-label foreground in --multi, as --hint-fg. Default white.
    #[arg(long, value_name = "COLOR", value_parser = color::parse)]
    select_fg: Option<Color>,

    /// Selected-label background in --multi, as --hint-fg. Default blue.
    #[arg(long, value_name = "COLOR", value_parser = color::parse)]
    select_bg: Option<Color>,

    /// Dim the repainted text so labels stand out more.
    ///
    /// Composes with the source style: a bold red cell stays bold red, dimmed.
    #[arg(long)]
    dim: bool,

    /// Scrollback to scan: a path, or "-"/omitted to read stdin.
    ///
    /// A file is how a producer that runs pouncer inside an interactive terminal hands over
    /// the region, since there stdin is the keyboard (felis' pipe action).
    #[arg(value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
    file: Option<String>,

    /// Print the man page or a shell completion script to stdout and exit.
    #[arg(long, value_name = "KIND", exclusive = true)]
    generate: Option<Generate>,
}

const AFTER_LONG_HELP: &str = "\
Keys:
  a label        pick it (with --multi: toggle it)
  Shift+label    pick it for --upper-action
  Enter          confirm the --multi selection
  Backspace      undo the last typed key
  Esc, Ctrl-c, Ctrl-g
                 cancel
  arrows, PgUp/PgDn, Home/End, Ctrl-d/u/f/b
                 scroll

A single match is picked without showing the overlay.

Exit status:
  0    a match was picked and its action succeeded
  1    no matches, or an error (including a failed action)
  2    invalid command line
  130  cancelled";

#[derive(Clone, Copy, clap::ValueEnum)]
enum Generate {
    /// pouncer(1) in roff.
    Man,
    /// bash completion script.
    CompleteBash,
    /// zsh completion script.
    CompleteZsh,
    /// fish completion script.
    CompleteFish,
}

fn generate(kind: Generate) -> Result<()> {
    let mut cmd = Cli::command();
    // Rendered into memory first: clap_complete panics on a failed write, and
    // a reader that stops early (`| head`) is not an error.
    let mut buf = Vec::new();
    let shell = match kind {
        Generate::Man => None,
        Generate::CompleteBash => Some(clap_complete::Shell::Bash),
        Generate::CompleteZsh => Some(clap_complete::Shell::Zsh),
        Generate::CompleteFish => Some(clap_complete::Shell::Fish),
    };
    match shell {
        Some(shell) => clap_complete::generate(shell, &mut cmd, "pouncer", &mut buf),
        None => clap_mangen::Man::new(cmd).render(&mut buf)?,
    }
    match std::io::stdout().lock().write_all(&buf) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e).context("write stdout"),
        _ => Ok(()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("pouncer: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    if let Some(kind) = cli.generate {
        generate(kind)?;
        return Ok(ExitCode::SUCCESS);
    }

    let mode = validate(&cli)?;
    let patterns = compile(&pattern_strings(&cli)?, cli.capture)?;

    let input = read_input(cli.file.as_deref())?;

    let cols = cli
        .cols
        .unwrap_or_else(|| crossterm::terminal::size().unwrap_or((80, 24)).0)
        .max(1);
    let rows = grid_rows(&input, cols);
    let parsed = source::parse(&input, rows, cols);
    // Below the cap the grid holds all of the input, so a scroll there is the
    // stream's own (SU, a scroll region) and drops what a terminal would too.
    if rows == u16::MAX && parsed.scrolled_off > 0 {
        eprintln!(
            "pouncer: input exceeds {rows} rows; the oldest {} rows were not scanned",
            parsed.scrolled_off
        );
    }
    let mut grid = parsed.rows;
    source::trim_trailing_empty(&mut grid);

    let mut matches = matcher::find(&grid, &patterns, cli.capture);
    // Before the empty check and before labelling: a filter that empties the
    // set must report "no matches" like any other, and one that leaves a
    // single match must reach the no-keystroke path.
    matches.retain(|m| m.width >= cli.min_len);
    if matches.is_empty() {
        eprintln!("pouncer: no matches");
        return Ok(ExitCode::from(1));
    }

    let values: Vec<&str> = matches.iter().map(|m| m.value.as_str()).collect();
    let labels = assign(
        &values,
        &Assignment {
            alphabet: &cli.alphabet,
            reverse: cli.reverse,
            unique: cli.unique,
        },
    );
    let unlabelled = labels.iter().filter(|l| l.is_none()).count();
    if unlabelled > 0 {
        eprintln!(
            "pouncer: {unlabelled} of {} matches are past what --alphabet can label and cannot be picked; \
             narrow the pattern, add --min-len or --unique, or lengthen --alphabet",
            matches.len()
        );
    }

    // An upper action keeps the overlay for a single target: the keystroke is
    // how the user chooses which of the two actions runs. Occurrences that
    // share a --unique label are one target.
    let single = labels.iter().all(|l| *l == labels[0]);
    let picked = if single && mode != picker::Mode::Upper {
        Some(picker::Pick {
            values: vec![matches[0].value.clone()],
            chosen: picker::Chosen::Default,
        })
    } else {
        picker::pick(&grid, &matches, &labels, mode, &theme(&cli))?
    };

    match picked {
        Some(picker::Pick { values, chosen }) => {
            let action = match chosen {
                picker::Chosen::Upper => cli.upper_action.as_deref(),
                picker::Chosen::Default => cli.action.as_deref(),
            };
            Action::parse(action).run(&values)?;
            Ok(ExitCode::SUCCESS)
        }
        None => Ok(ExitCode::from(130)),
    }
}

fn validate(cli: &Cli) -> Result<picker::Mode> {
    if cli.regexes.is_empty() && cli.presets.is_empty() {
        bail!("no patterns: pass at least one --regex or --preset (nothing matches by default)");
    }
    if let Err(why) = alphabet::validate(&cli.alphabet) {
        bail!("invalid --alphabet: {why}");
    }
    // --multi confirms a set with Enter, which carries no case, so the
    // uppercase-label action cannot be expressed there.
    if cli.multi && cli.upper_action.is_some() {
        bail!("--upper-action needs a single pick; it has no meaning with --multi");
    }
    if cli.upper_action.is_some() && cli.alphabet.chars().any(|c| c.is_ascii_uppercase()) {
        bail!(
            "--upper-action reads an uppercase key as the alternate action, so --alphabet must be lowercase"
        );
    }
    Ok(if cli.multi {
        picker::Mode::Multi
    } else if cli.upper_action.is_some() {
        picker::Mode::Upper
    } else {
        picker::Mode::Single
    })
}

fn theme(cli: &Cli) -> picker::Theme {
    let d = picker::Theme::default();
    picker::Theme {
        hint_fg: cli.hint_fg.unwrap_or(d.hint_fg),
        hint_bg: cli.hint_bg.unwrap_or(d.hint_bg),
        select_fg: cli.select_fg.unwrap_or(d.select_fg),
        select_bg: cli.select_bg.unwrap_or(d.select_bg),
        dim: cli.dim,
    }
}

fn read_input(path: Option<&str>) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    match path {
        Some(p) if p != "-" => {
            std::fs::File::open(p)
                .with_context(|| format!("open {p}"))?
                .read_to_end(&mut buf)
                .with_context(|| format!("read {p}"))?;
        }
        _ => {
            std::io::stdin()
                .read_to_end(&mut buf)
                .context("read scrollback from stdin")?;
        }
    }
    Ok(buf)
}

struct Pattern {
    origin: String,
    source: String,
}

/// User `--regex`es come first so they win the matcher's same-start ties.
fn pattern_strings(cli: &Cli) -> Result<Vec<Pattern>> {
    let mut patterns: Vec<Pattern> = cli
        .regexes
        .iter()
        .map(|re| Pattern {
            origin: format!("--regex {re:?}"),
            source: re.clone(),
        })
        .collect();
    for name in &cli.presets {
        match preset::pattern(name) {
            Some(p) => patterns.push(Pattern {
                origin: format!("--preset {name}"),
                source: p.to_owned(),
            }),
            None => bail!(
                "unknown --preset {name:?}; available: {}",
                preset::available()
            ),
        }
    }
    Ok(patterns)
}

/// Compile every pattern, refusing a `capture` group one of them lacks: its
/// matches would otherwise all be dropped and the run end in "no matches".
fn compile(patterns: &[Pattern], capture: Option<usize>) -> Result<Vec<regex::Regex>> {
    patterns
        .iter()
        .map(|p| {
            let re = regex::Regex::new(&p.source)
                .with_context(|| format!("invalid regex {:?}", p.source))?;
            if let Some(n) = capture
                && n >= re.captures_len()
            {
                bail!("--capture {n}: {} has no group {n}", p.origin);
            }
            Ok(re)
        })
        .collect()
}

/// Tall enough that no input scrolls off before matching, up to the grid's
/// `u16::MAX` limit; past it the caller reports the lost head.
fn grid_rows(input: &[u8], cols: u16) -> u16 {
    let lines = input.split(|&b| b == b'\n').count();
    let wrap_slack = input.len() / cols as usize;
    u16::try_from((lines + wrap_slack + 2).min(usize::from(u16::MAX))).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod proptests {
    use super::grid_rows;
    use crate::source::parse;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn grid_rows_holds_text_without_scrolling_any_of_it_off(
            text in proptest::collection::vec(
                prop_oneof![Just("a"), Just("漢"), Just("🐈"), Just("\n"), Just("\r\n"), Just("\t")],
                0..200,
            ).prop_map(|parts| parts.concat()),
            cols in 1u16..80,
        ) {
            let rows = grid_rows(text.as_bytes(), cols);
            prop_assert_eq!(parse(text.as_bytes(), rows, cols).scrolled_off, 0);
        }
    }
}
