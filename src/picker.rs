use crate::interrupt::Signals;
use crate::matcher::Match;
use crate::source::{Cell, Content, Row};
use crate::style::Style;
use crate::terminal::{self, Terminal};
use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    queue,
    style::{
        Attribute, Attributes, Color, Print, SetAttribute, SetAttributes, SetBackgroundColor,
        SetForegroundColor, SetUnderlineColor,
    },
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode, size,
    },
};
use std::borrow::Cow;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

struct Viewport {
    top: usize,
    height: usize,
    cols: u16,
}

impl Viewport {
    const fn max_top(&self, rows: usize) -> usize {
        rows.saturating_sub(self.height)
    }

    const fn scroll_to(&mut self, target: usize, rows: usize) {
        let max_top = self.max_top(rows);
        self.top = if target < max_top { target } else { max_top };
    }

    const fn scroll_up(&mut self, lines: usize) {
        self.top = self.top.saturating_sub(lines);
    }

    const fn scroll_down(&mut self, lines: usize, rows: usize) {
        self.scroll_to(self.top + lines, rows);
    }
}

pub(crate) struct Theme {
    pub(crate) hint_fg: Color,
    pub(crate) hint_bg: Color,
    pub(crate) select_fg: Color,
    pub(crate) select_bg: Color,
    pub(crate) dim: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            hint_fg: Color::Black,
            hint_bg: Color::Green,
            select_fg: Color::White,
            select_bg: Color::Blue,
            dim: false,
        }
    }
}

struct Scene<'a> {
    rows: &'a [Row],
    matches: &'a [Match],
    labels: &'a [Option<String>],
    theme: &'a Theme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Single,
    Upper,
    Multi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chosen {
    Default,
    Upper,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Pick {
    pub(crate) values: Vec<String>,
    pub(crate) chosen: Chosen,
}

pub(crate) fn pick(
    rows: &[Row],
    matches: &[Match],
    labels: &[Option<String>],
    mode: Mode,
    theme: &Theme,
) -> Result<Option<Pick>> {
    // Keys come from the terminal, never stdin: stdin is the scrollback pipe.
    // Opened before raw mode, so a console that cannot take the overlay is
    // refused while it is still untouched.
    let mut tty =
        terminal::open().with_context(|| format!("open {} for the overlay", terminal::NAME))?;

    let (cols, height) = size().unwrap_or((80, 24));
    let mut view = Viewport {
        top: 0,
        height: usize::from(height.max(1)),
        cols: cols.max(1),
    };
    // Start at the tail: the most recent output is what a just-run command's
    // result lives in, the usual reason to reach for a hint picker.
    view.top = view.max_top(rows.len());

    let scene = Scene {
        rows,
        matches,
        labels,
        theme,
    };

    let signals = Signals::register().context("catch termination signals")?;
    enable_raw_mode().context("enter raw mode")?;
    // A panic unwinds past the restore below, and its message would land on
    // the alternate screen that leaving it then erases.
    let previous_hook: Arc<dyn Fn(&std::panic::PanicHookInfo) + Send + Sync> =
        Arc::from(std::panic::take_hook());
    let hook = Arc::clone(&previous_hook);
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut tty) = terminal::open() {
            restore(&mut tty);
        }
        hook(info);
    }));
    let setup = queue!(tty, EnterAlternateScreen, Hide).context("set up overlay");
    let outcome = setup.and_then(|()| event_loop(&mut tty, &scene, mode, &mut view, &signals));
    restore(&mut tty);
    // Left in place, the hook would leave an alternate screen that a panic in
    // the action, long after this one closed, never entered.
    std::panic::set_hook(Box::new(move |info| previous_hook(info)));
    signals.release();
    outcome
}

fn restore(tty: &mut Terminal) {
    let _ = queue!(tty, Show, LeaveAlternateScreen);
    let _ = tty.flush();
    let _ = disable_raw_mode();
}

fn event_loop(
    tty: &mut Terminal,
    scene: &Scene,
    mode: Mode,
    view: &mut Viewport,
    signals: &Signals,
) -> Result<Option<Pick>> {
    let mut keys = Keys::new(scene.matches, scene.labels, mode);
    loop {
        draw(tty, scene, &keys.selected, &keys.prefix(), view)?;
        let Some(event) = next_event(signals)? else {
            return Ok(None);
        };
        let Event::Key(key) = event else {
            continue;
        };
        if let Step::Done(outcome) = keys.press(key, view, scene.rows.len()) {
            return Ok(outcome);
        }
    }
}

/// `None` once a termination signal is caught. `event::read` alone would not
/// return for one: crossterm retries its poll through EINTR.
fn next_event(signals: &Signals) -> Result<Option<Event>> {
    const SIGNAL_CHECK: Duration = Duration::from_millis(100);
    while !event::poll(SIGNAL_CHECK).context("read key")? {
        if signals.caught() {
            return Ok(None);
        }
    }
    event::read().context("read key").map(Some)
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Wait,
    Done(Option<Pick>),
}

struct Keys<'a> {
    matches: &'a [Match],
    labels: &'a [Option<String>],
    mode: Mode,
    typed: String,
    selected: Vec<bool>,
}

impl<'a> Keys<'a> {
    fn new(matches: &'a [Match], labels: &'a [Option<String>], mode: Mode) -> Self {
        Self {
            matches,
            labels,
            mode,
            typed: String::new(),
            selected: vec![false; matches.len()],
        }
    }

    fn press(&mut self, key: KeyEvent, view: &mut Viewport, rows: usize) -> Step {
        if key.kind == KeyEventKind::Release {
            return Step::Wait;
        }
        let half = (view.height / 2).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Step::Done(None),
            KeyCode::Enter => {
                if self.mode == Mode::Multi {
                    return Step::Done(confirm(self.matches, self.labels, &self.selected).map(
                        |values| Pick {
                            values,
                            chosen: Chosen::Default,
                        },
                    ));
                }
            }
            KeyCode::Up => view.scroll_up(1),
            KeyCode::Down => view.scroll_down(1, rows),
            KeyCode::PageUp => view.scroll_up(view.height),
            KeyCode::PageDown => view.scroll_down(view.height, rows),
            KeyCode::Home => view.top = 0,
            KeyCode::End => view.top = view.max_top(rows),
            KeyCode::Backspace => {
                self.typed.pop();
            }
            KeyCode::Char(c) if ctrl => match c {
                'c' | 'g' => return Step::Done(None),
                'd' => view.scroll_down(half, rows),
                'u' => view.scroll_up(half),
                'f' => view.scroll_down(view.height, rows),
                'b' => view.scroll_up(view.height),
                _ => {}
            },
            KeyCode::Char(c) => return self.type_char(c),
            _ => {}
        }
        Step::Wait
    }

    fn type_char(&mut self, c: char) -> Step {
        self.typed.push(c);
        let prefix = self.prefix();
        let labels = self.labels;
        if let Some(i) = labels
            .iter()
            .position(|l| l.as_deref() == Some(prefix.as_str()))
        {
            if self.mode != Mode::Multi {
                let chosen = if self.mode == Mode::Upper
                    && self.typed.chars().any(|c| c.is_ascii_uppercase())
                {
                    Chosen::Upper
                } else {
                    Chosen::Default
                };
                return Step::Done(Some(Pick {
                    values: vec![self.matches[i].value.clone()],
                    chosen,
                }));
            }
            let on = !self.selected[i];
            for (selected, label) in self.selected.iter_mut().zip(labels) {
                if label.as_deref() == Some(prefix.as_str()) {
                    *selected = on;
                }
            }
            self.typed.clear();
        } else if !labels.iter().flatten().any(|l| l.starts_with(&prefix)) {
            self.typed.clear();
        }
        Step::Wait
    }

    /// The typed keys keep their case so that erasing an uppercase key also
    /// forgets that it asked for the upper action.
    fn prefix(&self) -> String {
        if self.mode == Mode::Upper {
            self.typed.to_ascii_lowercase()
        } else {
            self.typed.clone()
        }
    }
}

/// `None` when nothing is selected, so an empty Enter cancels instead of
/// running the action on no input. Occurrences sharing a label (`--unique`)
/// are one selection and give one value.
fn confirm(matches: &[Match], labels: &[Option<String>], selected: &[bool]) -> Option<Vec<String>> {
    let mut seen = std::collections::HashSet::new();
    let values: Vec<String> = matches
        .iter()
        .zip(labels)
        .zip(selected)
        .filter(|&((_, label), &s)| s && seen.insert(label.as_deref()))
        .map(|((m, _), _)| m.value.clone())
        .collect();
    (!values.is_empty()).then_some(values)
}

/// The repaint emits one escape per style run rather than one per glyph.
struct Run {
    col: u16,
    style: Style,
    text: String,
}

/// A column gap breaks a run so the next one repositions itself: felis' HT
/// moves the cursor without writing spaces.
///
/// Cells that do not fit in `cols` are dropped rather than left to the
/// terminal: it would wrap them onto the next screen row, over that row's own
/// text, when `--cols` parses wider than the terminal.
fn runs(row: &Row, cols: u16) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let mut next_col = 0u16;
    for cell in row.cells.iter().filter(|c| c.col + c.width <= cols) {
        let text = match &cell.content {
            Content::Glyph(glyph) => printable(glyph, cell.width),
            Content::Blank => Cow::Borrowed(" "),
        };
        match out.last_mut() {
            Some(run) if cell.col == next_col && run.style == cell.style => {
                run.text.push_str(&text);
            }
            _ => out.push(Run {
                col: cell.col,
                style: cell.style,
                text: text.into_owned(),
            }),
        }
        next_col = cell.col + cell.width;
    }
    out
}

/// felis-vt treats only C0 and DEL as controls, so a C1 control encoded as
/// UTF-8 reaches the grid as glyph text, and a terminal that executes those
/// would run it as an escape sequence the source never showed. The controls
/// are dropped and `?` fills whatever of the cell's `width` the rest leaves
/// empty, measured as the grid measured the cluster: a `?` per control, or a
/// per-scalar width, misses its clamp and its emoji presentation and runs
/// past the cell. The matched value keeps the original bytes; only the
/// repaint changes.
fn printable(glyph: &str, width: u16) -> Cow<'_, str> {
    let is_c1 = |c: char| ('\u{80}'..='\u{9f}').contains(&c);
    if !glyph.contains(is_c1) {
        return Cow::Borrowed(glyph);
    }
    let mut kept: String = glyph.chars().filter(|&c| !is_c1(c)).collect();
    let used = u16::try_from(UnicodeWidthStr::width(kept.as_str())).unwrap_or(u16::MAX);
    kept.extend(std::iter::repeat_n(
        '?',
        usize::from(width.saturating_sub(used)),
    ));
    Cow::Owned(kept)
}

fn draw<W: Write>(
    w: &mut W,
    scene: &Scene,
    selected: &[bool],
    typed: &str,
    view: &Viewport,
) -> Result<()> {
    let Scene {
        rows,
        matches,
        labels,
        theme,
    } = *scene;
    // Entering the alternate screen does not reset the pen, and on a BCE
    // terminal a clear under an inherited background paints every untouched
    // cell with it.
    queue!(w, SetAttribute(Attribute::Reset), Clear(ClearType::All))?;

    for screen_row in 0..view.height {
        let Some(row) = rows.get(view.top + screen_row) else {
            break;
        };
        for run in runs(row, view.cols) {
            let attrs = if theme.dim {
                run.style.attrs | Attribute::Dim
            } else {
                run.style.attrs
            };
            queue_run(w, run, attrs, screen_row)?;
        }
    }

    // A selected match keeps its label whatever the typed prefix filters out.
    let shown = |i: usize| {
        labels[i]
            .as_deref()
            .filter(|label| selected[i] || typed.is_empty() || label.starts_with(typed))
    };
    let on_screen = |row: usize| row.checked_sub(view.top).filter(|&r| r < view.height);

    // The matched cells, reversed on top of their own pen: a fixed highlight
    // colour could vanish into whatever the source painted there, an inverted
    // pen cannot. Not dimmed, so the match is what stands out under --dim.
    for (i, m) in matches.iter().enumerate() {
        if shown(i).is_none() {
            continue;
        }
        for extent in &m.extents {
            let Some(screen_row) = on_screen(extent.row) else {
                continue;
            };
            let row = Row {
                cells: extent_cells(&rows[extent.row], extent.start..extent.end),
                continued: false,
            };
            for run in runs(&row, view.cols) {
                let mut attrs = run.style.attrs;
                attrs.toggle(Attribute::Reverse);
                queue_run(w, run, attrs, screen_row)?;
            }
        }
    }

    for (i, m) in matches.iter().enumerate() {
        let Some(label) = shown(i) else {
            continue;
        };
        let (fg, bg) = if selected[i] {
            (theme.select_fg, theme.select_bg)
        } else {
            (theme.hint_fg, theme.hint_bg)
        };
        let cells = label_cells(m, label.len(), view.cols);
        // A label the screen is too narrow for, or one the viewport cuts,
        // would show a fragment that reads as some other, shorter key.
        if cells.len() != label.len()
            || cells
                .iter()
                .any(|&(row, col)| on_screen(row).is_none() || col >= view.cols)
        {
            continue;
        }
        queue_label(w, rows, label, &cells, (fg, bg), view)?;
    }

    draw_scroll_indicator(w, rows.len(), view)?;
    w.flush().context("flush overlay")
}

/// Every one of `cells` must be on screen.
fn queue_label<W: Write>(
    w: &mut W,
    rows: &[Row],
    text: &str,
    cells: &[(usize, u16)],
    (fg, bg): (Color, Color),
    view: &Viewport,
) -> Result<()> {
    let mut taken = 0;
    for (row, segment) in label_segments(cells) {
        let paint = painted(&rows[row], &segment, view.cols);
        let piece = &text[taken..taken + segment.len()];
        taken += segment.len();
        let padded = format!(
            "{}{piece}{}",
            " ".repeat(usize::from(segment.start - paint.start)),
            " ".repeat(usize::from(paint.end - segment.end))
        );
        // The leading reset is what makes the label win over a reversed,
        // hidden or background-painted source cell underneath it.
        queue!(
            w,
            MoveTo(paint.start, (row - view.top) as u16),
            SetAttribute(Attribute::Reset),
            SetForegroundColor(fg),
            SetBackgroundColor(bg),
            SetAttribute(Attribute::Bold),
            Print(padded),
            SetAttribute(Attribute::Reset),
        )?;
    }
    Ok(())
}

fn queue_run<W: Write>(w: &mut W, run: Run, attrs: Attributes, screen_row: usize) -> Result<()> {
    queue!(
        w,
        MoveTo(run.col, screen_row as u16),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(run.style.fg),
        SetBackgroundColor(run.style.bg),
        SetUnderlineColor(run.style.underline),
        SetAttributes(attrs),
        Print(run.text),
        SetAttribute(Attribute::Reset),
    )
    .context("draw run")
}

/// The `(row, col)` cells a label `len` columns wide covers for `m`: the
/// match's own cells in reading order, so a label longer than a wrapped
/// match's first row carries on after the wrap; then the cells past the
/// match's end on its last row; and only when the right edge still leaves it
/// short, the cells before the match's start, so the label never runs off the
/// screen.
fn label_cells(m: &Match, len: usize, cols: u16) -> Vec<(usize, u16)> {
    let (Some(first), Some(last)) = (m.extents.first(), m.extents.last()) else {
        return Vec::new();
    };
    // The left shift below would put the label of a match that starts off
    // screen on the visible text before it.
    if first.start >= cols {
        return Vec::new();
    }
    let mut cells: Vec<(usize, u16)> = m
        .extents
        .iter()
        .flat_map(|e| (e.start..e.end.min(cols)).map(move |c| (e.row, c)))
        .chain((last.end..cols).map(|c| (last.row, c)))
        .collect();
    let short = u16::try_from(len.saturating_sub(cells.len())).unwrap_or(u16::MAX);
    let lead = first.start.saturating_sub(short)..first.start;
    cells.splice(0..0, lead.map(|c| (first.row, c)));
    cells.truncate(len);
    cells
}

fn label_segments(cells: &[(usize, u16)]) -> Vec<(usize, std::ops::Range<u16>)> {
    let mut out: Vec<(usize, std::ops::Range<u16>)> = Vec::new();
    for &(row, col) in cells {
        match out.last_mut() {
            Some((r, range)) if *r == row && range.end == col => range.end += 1,
            _ => out.push((row, col..col + 1)),
        }
    }
    out
}

/// The row's cells in `cols`, with every column no cell covers filled by a
/// default-pen blank: an HT inside a match leaves such a hole, and the
/// highlight must still read as one stretch.
fn extent_cells(row: &Row, cols: std::ops::Range<u16>) -> Vec<Cell> {
    let blank = |col| Cell {
        content: Content::Blank,
        col,
        width: 1,
        style: Style::default(),
    };
    let mut out = Vec::new();
    let mut next = cols.start;
    for cell in row.cells.iter().filter(|c| cols.contains(&c.col)) {
        out.extend((next..cell.col).map(blank));
        out.push(cell.clone());
        next = cell.col + cell.width;
    }
    out.extend((next..cols.end).map(blank));
    out
}

/// Every column a label segment paints. A label edge that splits a wide glyph
/// would leave half of it on screen, so the segment widens to blank that half
/// too, short of the right edge, where the glyph was never drawn.
fn painted(row: &Row, segment: &std::ops::Range<u16>, cols: u16) -> std::ops::Range<u16> {
    let start = straddled(row, segment.start).map_or(segment.start, |c| c.col);
    let end = straddled(row, segment.end).map_or(segment.end, |c| (c.col + c.width).min(cols));
    start..end
}

fn straddled(row: &Row, col: u16) -> Option<&Cell> {
    row.cells
        .iter()
        .find(|c| c.col < col && col < c.col + c.width)
}

fn draw_scroll_indicator<W: Write>(w: &mut W, rows: usize, view: &Viewport) -> Result<()> {
    let max_top = view.max_top(rows);
    if max_top == 0 {
        return Ok(());
    }
    let up = if view.top > 0 { '^' } else { ' ' };
    let down = if view.top < max_top { 'v' } else { ' ' };
    let pct = view.top * 100 / max_top;
    let indicator = format!(" {up}{down} {pct:>3}% ");
    let Some(col) = u16::try_from(indicator.chars().count())
        .ok()
        .and_then(|width| view.cols.checked_sub(width))
    else {
        return Ok(());
    };
    queue!(
        w,
        MoveTo(col, (view.height as u16).saturating_sub(1)),
        SetAttribute(Attribute::Reverse),
        Print(indicator),
        SetAttribute(Attribute::Reset),
    )
    .context("draw scroll indicator")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{Content, Row, Scene, Theme, Viewport, confirm, draw, label_cells, runs};
    use crate::matcher::{Extent, Match};
    use crate::source::screen;
    use crate::test_support::AnsiColor;

    fn view(top: usize, height: usize) -> Viewport {
        Viewport {
            top,
            height,
            cols: 80,
        }
    }

    fn at(value: &str) -> Match {
        Match {
            extents: vec![Extent {
                row: 0,
                start: 0,
                end: 1,
            }],
            width: 1,
            value: value.to_owned(),
        }
    }

    fn render_with(input: &[u8], cols: u16, theme: &Theme, labels: &[Option<String>]) -> String {
        let rows = screen(input, 1, cols);
        let matches: Vec<Match> = labels.iter().map(|_| at("x")).collect();
        let scene = Scene {
            rows: &rows,
            matches: &matches,
            labels,
            theme,
        };
        let mut out = Vec::new();
        draw(
            &mut out,
            &scene,
            &vec![false; matches.len()],
            "",
            &view(0, 1),
        )
        .expect("draw into a buffer");
        String::from_utf8(out).expect("the overlay emits utf-8")
    }

    fn render(input: &[u8], cols: u16) -> String {
        render_with(input, cols, &Theme::default(), &[])
    }

    fn row_of(input: &[u8], cols: u16) -> Row {
        screen(input, 1, cols).remove(0)
    }

    #[test]
    fn confirm_gives_one_value_per_shared_label() {
        let matches = [at("x"), at("y"), at("x")];
        let labels = ["a", "s", "a"].map(|l| Some(l.to_owned()));
        let selected = [true, true, true];
        assert_eq!(
            confirm(&matches, &labels, &selected),
            Some(vec!["x".to_owned(), "y".to_owned()])
        );
    }

    #[test]
    fn adjacent_cells_with_one_style_become_one_run() {
        let row = row_of(b"\x1b[31mabc", 10);
        let out = runs(&row, u16::MAX);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].col, out[0].text.as_str()), (0, "abc"));
    }

    #[test]
    fn a_style_change_splits_a_run() {
        let row = row_of(b"ab\x1b[31mcd", 10);
        let out = runs(&row, u16::MAX);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].col, out[0].text.as_str()), (0, "ab"));
        assert_eq!((out[1].col, out[1].text.as_str()), (2, "cd"));
    }

    #[test]
    fn a_column_gap_splits_a_run() {
        // felis' HT moves the cursor without writing spaces, so a tab-aligned
        // row has a hole the repaint must step over rather than close up.
        let row = row_of(b"ab\tcd", 20);
        let out = runs(&row, u16::MAX);
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].col, out[0].text.as_str()), (0, "ab"));
        assert_eq!((out[1].col, out[1].text.as_str()), (8, "cd"));
    }

    #[test]
    fn a_wide_glyph_merges_with_the_next_same_style_glyph() {
        // The next cell starts at col + width, not col + 1, so the run stays
        // whole across a double-width glyph.
        let row = row_of("世a".as_bytes(), 10);
        let out = runs(&row, u16::MAX);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "世a");
    }

    #[test]
    fn a_styled_blank_contributes_a_space_to_its_run() {
        let _colors = AnsiColor::set(true);
        let row = row_of(b"\x1b[41m\x1b[K", 3);
        let out = runs(&row, u16::MAX);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "   ");
        assert!(matches!(row.cells[0].content, Content::Blank));
    }

    #[test]
    fn a_c1_control_in_the_input_is_repainted_as_a_question_mark() {
        let row = row_of("a\u{9b}31mb".as_bytes(), 20);
        let out = runs(&row, u16::MAX);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "a?31mb");
    }

    #[test]
    fn a_c1_control_after_a_wide_glyph_adds_no_column() {
        let row = row_of("世\u{9b}a".as_bytes(), 20);
        assert_eq!(runs(&row, u16::MAX)[0].text, "世a");
    }

    #[test]
    fn a_c1_control_after_an_emoji_presentation_adds_no_column() {
        for input in ["☺\u{fe0f}\u{9b}b", "1\u{fe0f}\u{20e3}\u{9b}b"] {
            let row = row_of(input.as_bytes(), 20);
            let text = &runs(&row, u16::MAX)[0].text;
            assert!(!text.contains('?'), "{input:?} repainted as {text:?}");
        }
    }

    #[test]
    fn two_c1_controls_in_one_cell_fill_only_its_width() {
        let row = row_of("a\u{9b}\u{9b}b".as_bytes(), 20);
        assert_eq!(row.cells[0].width, 2);
        assert_eq!(runs(&row, u16::MAX)[0].text, "a?b");
    }

    #[test]
    fn a_plain_row_renders_as_one_run() {
        // The "no regression for plain producers" claim: colourless input is
        // still one escape sequence for the whole stretch.
        assert_eq!(runs(&row_of(b"hello world", 40), u16::MAX).len(), 1);
    }

    #[test]
    fn the_draw_starts_with_a_reset_before_the_clear() {
        // Entering the alternate screen does not reset the pen; on a BCE
        // terminal the clear would otherwise paint the inherited background.
        let _colors = AnsiColor::set(true);
        assert!(render(b"a", 10).starts_with("\x1b[0m\x1b[2J"));
    }

    #[test]
    fn a_row_ends_with_exactly_one_reset() {
        let _colors = AnsiColor::set(true);
        let out = render(b"\x1b[1mab", 10);
        assert!(out.ends_with("\x1b[0m"));
        assert!(!out.ends_with("\x1b[0m\x1b[0m"));
    }

    #[test]
    fn a_default_run_after_a_styled_run_starts_with_a_reset() {
        // Re-specifying the pen per run is what keeps a stale attribute from
        // leaking into the next run.
        let _colors = AnsiColor::set(true);
        let out = render(b"\x1b[1;7mA\x1b[0mB", 10);
        assert!(out.contains("\x1b[1;2H\x1b[0m"));
    }

    #[test]
    fn the_underline_color_is_emitted_as_default_or_as_the_set_color() {
        let _colors = AnsiColor::set(true);
        assert!(render(b"a", 10).contains("\x1b[59m"));
        assert!(render(b"\x1b[4;58;5;5mx", 10).contains("58;5;5"));
        assert!(render(b"\x1b[4;58;2;1;2;3mx", 10).contains("58;2;1;2;3"));
    }

    #[test]
    fn no_color_drops_the_colors_but_keeps_the_attributes() {
        let _colors = AnsiColor::set(false);
        let out = render_with(
            b"\x1b[1;31mA",
            10,
            &Theme::default(),
            &[Some("a".to_owned())],
        );
        assert!(!out.contains("38;5;1"), "colors survived NO_COLOR: {out:?}");
        // One bold each from the text run, its highlight, and the label.
        assert_eq!(out.matches("\x1b[1m").count(), 3, "{out:?}");
    }

    #[test]
    fn a_label_is_drawn_bold_over_the_text_after_its_own_reset() {
        let _colors = AnsiColor::set(true);
        let out = render_with(b"\x1b[7mA", 10, &Theme::default(), &[Some("a".to_owned())]);
        let label = out.rsplit("\x1b[1;1H").next().expect("a label pass");
        assert!(label.starts_with("\x1b[0m"));
        assert!(label.contains("\x1b[1m"));
        assert!(label.ends_with("\x1b[0m"));
    }

    fn on(row: usize, start: u16, end: u16) -> Extent {
        Extent { row, start, end }
    }

    fn spanning(extents: Vec<Extent>) -> Match {
        Match {
            extents,
            width: 0,
            value: String::new(),
        }
    }

    fn render_match(input: &[u8], cols: u16, m: Match, label: &str, theme: &Theme) -> String {
        let rows = screen(input, 1, cols);
        let matches = [m];
        let labels = [Some(label.to_owned())];
        let scene = Scene {
            rows: &rows,
            matches: &matches,
            labels: &labels,
            theme,
        };
        let mut out = Vec::new();
        let mut v = view(0, 1);
        v.cols = cols;
        draw(&mut out, &scene, &[false], "", &v).expect("draw into a buffer");
        String::from_utf8(out).expect("the overlay emits utf-8")
    }

    #[test]
    fn a_label_covers_the_leading_cells_of_its_match() {
        let m = spanning(vec![on(0, 4, 9)]);
        assert_eq!(label_cells(&m, 2, 20), [(0, 4), (0, 5)]);
    }

    #[test]
    fn a_label_carries_on_after_the_wrap_of_a_wrapped_match() {
        let m = spanning(vec![on(0, 9, 10), on(1, 0, 6)]);
        assert_eq!(label_cells(&m, 2, 10), [(0, 9), (1, 0)]);
    }

    #[test]
    fn a_label_longer_than_its_match_runs_on_past_the_end() {
        let m = spanning(vec![on(0, 3, 4)]);
        assert_eq!(label_cells(&m, 3, 10), [(0, 3), (0, 4), (0, 5)]);
    }

    #[test]
    fn a_label_at_the_right_edge_shifts_left_to_stay_on_screen() {
        let m = spanning(vec![on(0, 9, 10)]);
        assert_eq!(label_cells(&m, 3, 10), [(0, 7), (0, 8), (0, 9)]);
    }

    #[test]
    fn the_matched_cells_are_repainted_reversed() {
        let _colors = AnsiColor::set(true);
        let out = render_match(
            b"abc",
            10,
            spanning(vec![on(0, 1, 2)]),
            "x",
            &Theme::default(),
        );
        assert!(out.contains("\x1b[7mb"), "{out:?}");
    }

    #[test]
    fn an_already_reversed_cell_is_highlighted_by_dropping_the_reverse() {
        let _colors = AnsiColor::set(true);
        let out = render_match(
            b"\x1b[7mA",
            10,
            spanning(vec![on(0, 0, 1)]),
            "x",
            &Theme::default(),
        );
        // The repaint reverses it once; the highlight does not.
        assert_eq!(out.matches("\x1b[7m").count(), 1, "{out:?}");
    }

    #[test]
    fn the_highlight_is_not_dimmed() {
        let _colors = AnsiColor::set(true);
        let theme = Theme {
            dim: true,
            ..Theme::default()
        };
        let out = render_match(b"ab", 10, spanning(vec![on(0, 1, 2)]), "x", &theme);
        let highlight = out.split("\x1b[1;2H").nth(1).expect("a highlight pass");
        assert!(!highlight.contains("\x1b[2m"), "{highlight:?}");
    }

    #[test]
    fn a_label_ending_inside_a_wide_glyph_blanks_its_other_half() {
        let _colors = AnsiColor::set(true);
        let out = render_match(
            "世x".as_bytes(),
            10,
            spanning(vec![on(0, 0, 3)]),
            "a",
            &Theme::default(),
        );
        let label = out.rsplit("\x1b[1;1H").next().expect("a label pass");
        assert!(label.contains("a "), "{label:?}");
    }

    fn render_scene(
        input: &[u8],
        (height, cols): (u16, u16),
        matches: &[Match],
        labels: &[&str],
        theme: &Theme,
        top: usize,
    ) -> String {
        let rows = screen(input, height, cols);
        let labels: Vec<Option<String>> = labels.iter().map(|&l| Some(l.to_owned())).collect();
        let scene = Scene {
            rows: &rows,
            matches,
            labels: &labels,
            theme,
        };
        let mut out = Vec::new();
        let mut v = view(top, usize::from(height) - top);
        v.cols = cols;
        draw(&mut out, &scene, &vec![false; matches.len()], "", &v).expect("draw into a buffer");
        String::from_utf8(out).expect("the overlay emits utf-8")
    }

    #[test]
    fn a_label_on_one_row_is_drawn_in_one_piece() {
        let _colors = AnsiColor::set(true);
        let matches = [spanning(vec![on(0, 0, 6)])];
        let out = render_scene(b"abcdef", (1, 10), &matches, &["cd"], &Theme::default(), 0);
        assert!(
            out.contains("\x1b[1;1H\x1b[0m\x1b[38;5;0m\x1b[48;5;10m\x1b[1mcd\x1b[0m"),
            "{out:?}"
        );
    }

    #[test]
    fn a_wrapped_label_draws_each_key_on_its_own_row() {
        let _colors = AnsiColor::set(true);
        let matches = [spanning(vec![on(0, 4, 5), on(1, 0, 3)])];
        let out = render_scene(b"xxxxyzzz", (2, 5), &matches, &["ab"], &Theme::default(), 0);
        assert!(
            out.contains("\x1b[1;5H\x1b[0m\x1b[38;5;0m\x1b[48;5;10m\x1b[1ma\x1b[0m"),
            "{out:?}"
        );
        assert!(
            out.contains("\x1b[2;1H\x1b[0m\x1b[38;5;0m\x1b[48;5;10m\x1b[1mb\x1b[0m"),
            "{out:?}"
        );
    }

    #[test]
    fn a_label_the_viewport_cuts_is_not_drawn_at_all() {
        let _colors = AnsiColor::set(true);
        // Row 0 is scrolled away; the label would start there and continue on
        // row 1, so a lone `b` on row 1 must not appear.
        let matches = [spanning(vec![on(0, 4, 5), on(1, 0, 3)])];
        let out = render_scene(b"xxxxyzzz", (2, 5), &matches, &["ab"], &Theme::default(), 1);
        let drawn = out.split("48;5;10m\x1b[1m").skip(1).count();
        assert_eq!(drawn, 0, "{out:?}");
    }

    fn render_narrower(
        input: &[u8],
        grid_cols: u16,
        term_cols: u16,
        matches: &[Match],
        label: &str,
    ) -> String {
        let rows = screen(input, 1, grid_cols);
        let labels = vec![Some(label.to_owned()); matches.len()];
        let scene = Scene {
            rows: &rows,
            matches,
            labels: &labels,
            theme: &Theme::default(),
        };
        let mut out = Vec::new();
        let mut v = view(0, 1);
        v.cols = term_cols;
        draw(&mut out, &scene, &vec![false; matches.len()], "", &v).expect("draw into a buffer");
        String::from_utf8(out).expect("the overlay emits utf-8")
    }

    #[test]
    fn a_grid_wider_than_the_terminal_draws_only_the_columns_that_fit() {
        let out = render_narrower(b"abcdefghijklmnop", 20, 10, &[], "x");
        assert!(out.contains("abcdefghij"), "{out:?}");
        assert!(!out.contains('k'), "{out:?}");
    }

    #[test]
    fn a_match_past_the_terminal_width_gets_no_label_or_highlight() {
        let _colors = AnsiColor::set(true);
        let matches = [spanning(vec![on(0, 0, 2)]), spanning(vec![on(0, 12, 14)])];
        let out = render_narrower(b"abcdefghijklmnop", 20, 10, &matches, "x");
        let drawn = out.split("48;5;10m\x1b[1m").skip(1).count();
        assert_eq!(drawn, 1, "{out:?}");
        assert!(!out.contains("\x1b[1;13H"), "{out:?}");
    }

    #[test]
    fn a_match_starting_at_the_first_clipped_column_gets_no_label() {
        let _colors = AnsiColor::set(true);
        let matches = [spanning(vec![on(0, 10, 12)])];
        let out = render_narrower(b"abcdefghijklmnop", 20, 10, &matches, "x");
        let drawn = out.split("48;5;10m\x1b[1m").skip(1).count();
        assert_eq!(drawn, 0, "{out:?}");
    }

    #[test]
    fn a_label_on_a_match_crossing_the_right_edge_shifts_left_inside_it() {
        let _colors = AnsiColor::set(true);
        let matches = [spanning(vec![on(0, 9, 15)])];
        let out = render_narrower(b"abcdefghijklmnop", 20, 10, &matches, "xx");
        assert!(
            out.contains("\x1b[1;9H\x1b[0m\x1b[38;5;0m\x1b[48;5;10m\x1b[1mxx\x1b[0m"),
            "{out:?}"
        );
    }

    #[test]
    fn the_scroll_indicator_is_not_drawn_in_a_terminal_narrower_than_it() {
        let rows = screen(b"a\nb\nc", 3, 5);
        let scene = Scene {
            rows: &rows,
            matches: &[],
            labels: &[],
            theme: &Theme::default(),
        };
        let mut out = Vec::new();
        let mut v = view(0, 1);
        v.cols = 5;
        draw(&mut out, &scene, &[], "", &v).expect("draw into a buffer");
        let out = String::from_utf8(out).expect("the overlay emits utf-8");
        assert!(!out.contains('%'), "{out:?}");
    }

    #[test]
    fn a_label_padding_a_wide_glyph_at_the_right_edge_stops_at_the_edge() {
        let _colors = AnsiColor::set(true);
        // 世 starts at 0-based column 9 and is two columns wide, so it does not
        // fit a 10-column terminal and is not drawn; the label at column 9 must
        // print one column, not pad over the glyph's far half.
        let matches = [spanning(vec![on(0, 9, 11)])];
        let out = render_narrower("abcdefghi世".as_bytes(), 20, 10, &matches, "x");
        let label = out.rsplit("\x1b[1;10H").next().expect("a label pass");
        assert!(label.contains("\x1b[1mx\x1b[0m"), "{label:?}");
    }

    #[test]
    fn the_highlight_spans_a_tab_gap_inside_the_match() {
        let _colors = AnsiColor::set(true);
        let matches = [spanning(vec![on(0, 0, 9)])];
        let out = render_scene(b"a\tb", (1, 20), &matches, &["x"], &Theme::default(), 0);
        assert!(out.contains("\x1b[7ma       b"), "{out:?}");
    }

    #[test]
    fn a_label_wider_than_the_screen_is_not_drawn_truncated() {
        let _colors = AnsiColor::set(true);
        let matches = [spanning(vec![on(0, 0, 1)])];
        let out = render_scene(b"a", (1, 2), &matches, &["xyz"], &Theme::default(), 0);
        let drawn = out.split("48;5;10m\x1b[1m").skip(1).count();
        assert_eq!(drawn, 0, "{out:?}");
    }
}

#[cfg(test)]
mod proptests {
    #![allow(clippy::expect_used)]

    use super::{Scene, Theme, Viewport, draw};
    use crate::source::{Content, screen};
    use crate::test_support::AnsiColor;
    use proptest::prelude::*;

    #[derive(Debug, Clone, Copy)]
    enum Scroll {
        Up(usize),
        Down(usize),
        To(usize),
    }

    fn scroll_strategy() -> impl Strategy<Value = Scroll> {
        prop_oneof![
            (0usize..50).prop_map(Scroll::Up),
            (0usize..50).prop_map(Scroll::Down),
            (0usize..100).prop_map(Scroll::To),
        ]
    }

    const SGR: &[&str] = &[
        "\x1b[0m",
        "\x1b[1m",
        "\x1b[2m",
        "\x1b[3m",
        "\x1b[4m",
        "\x1b[4:3m",
        "\x1b[5m",
        "\x1b[7m",
        "\x1b[8m",
        "\x1b[9m",
        "\x1b[53m",
        "\x1b[31m",
        "\x1b[38;5;200m",
        "\x1b[38;2;1;2;3m",
        "\x1b[44m",
        "\x1b[48;2;9;8;7m",
        "\x1b[58;5;3m",
        "\x1b[K",
    ];

    fn styled_text_strategy() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![
                proptest::sample::select(SGR).prop_map(str::to_owned),
                proptest::sample::select(&["a", "b", " ", "漢", "\t"][..]).prop_map(str::to_owned),
            ],
            0..40,
        )
        .prop_map(|parts| parts.concat())
    }

    /// A styled blank is repainted as a space under the same pen, which reads
    /// back as a glyph.
    fn painted(content: &Content) -> &str {
        match content {
            Content::Glyph(text) => text,
            Content::Blank => " ",
        }
    }

    proptest! {
        #[test]
        fn scrolling_never_leaves_the_viewport_past_the_tail(
            rows in 0usize..60,
            height in 1usize..30,
            ops in proptest::collection::vec(scroll_strategy(), 0..20),
        ) {
            let mut view = Viewport { top: 0, height, cols: 80 };
            let max_top = rows.saturating_sub(height);
            for op in ops {
                let before = view.top;
                match op {
                    Scroll::Up(n) => {
                        view.scroll_up(n);
                        prop_assert_eq!(view.top, before.saturating_sub(n));
                    }
                    Scroll::Down(n) => {
                        view.scroll_down(n, rows);
                        prop_assert_eq!(view.top, (before + n).min(max_top));
                    }
                    Scroll::To(n) => {
                        view.scroll_to(n, rows);
                        prop_assert_eq!(view.top, n.min(max_top));
                    }
                }
            }
        }

        #[test]
        fn the_repaint_reparses_to_the_cells_it_was_drawn_from(
            input in styled_text_strategy(),
            dim in any::<bool>(),
        ) {
            let _colors = AnsiColor::set(true);
            let (height, cols) = (4u16, 12u16);
            let rows = screen(input.as_bytes(), height, cols);
            let scene = Scene { rows: &rows, matches: &[], labels: &[], theme: &Theme { dim, ..Theme::default() } };
            let mut out = Vec::new();
            draw(&mut out, &scene, &[], "", &Viewport { top: 0, height: usize::from(height), cols })
                .expect("draw into a buffer");
            let back = screen(&out, height, cols);
            for (drawn, source) in back.iter().zip(&rows) {
                prop_assert_eq!(drawn.cells.len(), source.cells.len());
                for (d, s) in drawn.cells.iter().zip(&source.cells) {
                    prop_assert_eq!((painted(&d.content), d.col, d.width), (painted(&s.content), s.col, s.width));
                    let mut want = s.style;
                    if dim {
                        want.attrs.set(crossterm::style::Attribute::Dim);
                    }
                    prop_assert_eq!(d.style, want);
                }
            }
        }
    }
}

#[cfg(test)]
mod snapshots {
    #![allow(clippy::expect_used)]

    use super::{Scene, Theme, Viewport, draw};
    use crate::label::{Assignment, assign};
    use crate::matcher::find;
    use crate::source::{Cell, Content, Row, screen, trim_trailing_empty};
    use crate::test_support::AnsiColor;
    use crossterm::style::Color;
    use regex::Regex;

    /// Backgrounds no scene's source uses, so a label cell is told apart from
    /// the text under it by its read-back pen alone.
    const THEME: Theme = Theme {
        hint_fg: Color::AnsiValue(0),
        hint_bg: Color::AnsiValue(201),
        select_fg: Color::AnsiValue(0),
        select_bg: Color::AnsiValue(202),
        dim: false,
    };

    struct Overlay<'a> {
        input: &'a str,
        pattern: &'a str,
        cols: u16,
        height: usize,
        alphabet: &'a str,
        reverse: bool,
        typed: &'a str,
        selected: &'a [usize],
        scroll_up: usize,
    }

    impl Default for Overlay<'_> {
        fn default() -> Self {
            Self {
                input: "",
                pattern: r"https?://\S+",
                cols: 30,
                height: 4,
                alphabet: crate::alphabet::DEFAULT,
                reverse: false,
                typed: "",
                selected: &[],
                scroll_up: 0,
            }
        }
    }

    impl Overlay<'_> {
        fn draw(&self) -> Vec<u8> {
            let _colors = AnsiColor::set(true);
            let mut rows = screen(self.input.as_bytes(), 64, self.cols);
            trim_trailing_empty(&mut rows);
            let re = Regex::new(self.pattern).expect("pattern compiles");
            let matches = find(&rows, &[re], None);
            let values: Vec<&str> = matches.iter().map(|m| m.value.as_str()).collect();
            let labels = assign(
                &values,
                &Assignment {
                    alphabet: self.alphabet,
                    reverse: self.reverse,
                    unique: false,
                },
            );
            let mut selected = vec![false; matches.len()];
            for &i in self.selected {
                selected[i] = true;
            }
            let mut view = Viewport {
                top: 0,
                height: self.height,
                cols: self.cols,
            };
            view.top = view.max_top(rows.len());
            view.scroll_up(self.scroll_up);
            let scene = Scene {
                rows: &rows,
                matches: &matches,
                labels: &labels,
                theme: &THEME,
            };
            let mut out = Vec::new();
            draw(&mut out, &scene, &selected, self.typed, &view).expect("draw into a buffer");
            out
        }

        fn screen(&self) -> String {
            let back = screen(&self.draw(), self.height as u16, self.cols);
            let border = format!("+{}+", "-".repeat(usize::from(self.cols)));
            let mut out = vec![border.clone()];
            for row in &back {
                out.push(format!(
                    "|{}|",
                    self.line(row, |cell| match &cell.content {
                        Content::Glyph(text) => text.clone(),
                        Content::Blank => " ".to_owned(),
                    })
                ));
                let marks = self.line(row, |cell| {
                    let mark = match cell.style.bg {
                        bg if bg == THEME.hint_bg => "^",
                        bg if bg == THEME.select_bg => "*",
                        _ => " ",
                    };
                    mark.repeat(usize::from(cell.width))
                });
                if !marks.trim().is_empty() {
                    out.push(format!(" {}", marks.trim_end()));
                }
            }
            out.push(border);
            out.join("\n")
        }

        fn line(&self, row: &Row, paint: impl Fn(&Cell) -> String) -> String {
            let mut line = String::new();
            let mut col = 0;
            for cell in &row.cells {
                line.push_str(&" ".repeat(usize::from(cell.col - col)));
                line.push_str(&paint(cell));
                col = cell.col + cell.width;
            }
            line.push_str(&" ".repeat(usize::from(self.cols - col)));
            line
        }
    }

    fn tall() -> String {
        (1..=9)
            .map(|n| format!("{n} http://h{n}.example\n"))
            .collect::<Vec<_>>()
            .concat()
    }

    #[test]
    fn labels_cover_the_start_of_each_match() {
        insta::assert_snapshot!(
            Overlay {
                input: "see https://a.example\nand http://b.example/x",
                ..Overlay::default()
            }
            .screen()
        );
    }

    #[test]
    fn a_label_after_a_wide_glyph_lands_on_its_display_column() {
        insta::assert_snapshot!(
            Overlay {
                input: "漢字 https://a.example",
                ..Overlay::default()
            }
            .screen()
        );
    }

    #[test]
    fn a_soft_wrapped_match_is_labelled_once_on_its_first_row() {
        insta::assert_snapshot!(
            Overlay {
                input: "x https://a.example/a/very/long/path y",
                cols: 16,
                ..Overlay::default()
            }
            .screen()
        );
    }

    #[test]
    fn a_tall_input_opens_at_the_tail_with_a_scroll_indicator() {
        let input = tall();
        insta::assert_snapshot!(
            Overlay {
                input: &input,
                ..Overlay::default()
            }
            .screen()
        );
    }

    #[test]
    fn scrolling_up_moves_the_labels_with_their_rows() {
        let input = tall();
        insta::assert_snapshot!(
            Overlay {
                input: &input,
                scroll_up: 3,
                ..Overlay::default()
            }
            .screen()
        );
    }

    #[test]
    fn at_the_top_the_indicator_points_only_down() {
        let input = tall();
        insta::assert_snapshot!(
            Overlay {
                input: &input,
                scroll_up: usize::MAX,
                ..Overlay::default()
            }
            .screen()
        );
    }

    #[test]
    fn a_typed_prefix_hides_the_labels_it_rules_out() {
        insta::assert_snapshot!(
            Overlay {
                input: "1 http://a 2 http://b\n3 http://c 4 http://d",
                alphabet: "ab",
                typed: "b",
                ..Overlay::default()
            }
            .screen()
        );
    }

    #[test]
    fn a_selected_label_survives_a_prefix_that_rules_it_out() {
        insta::assert_snapshot!(
            Overlay {
                input: "1 http://a 2 http://b\n3 http://c 4 http://d",
                alphabet: "ab",
                typed: "b",
                selected: &[0],
                ..Overlay::default()
            }
            .screen()
        );
    }

    #[test]
    fn matches_past_the_alphabet_capacity_get_no_label() {
        insta::assert_snapshot!(
            Overlay {
                input: "http://1 http://2 http://3\nhttp://4 http://5",
                alphabet: "ab",
                ..Overlay::default()
            }
            .screen()
        );
    }

    #[test]
    fn reverse_gives_the_shortest_labels_to_the_last_matches() {
        insta::assert_snapshot!(
            Overlay {
                input: "1 http://a 2 http://b\n3 http://c",
                alphabet: "ab",
                reverse: true,
                ..Overlay::default()
            }
            .screen()
        );
    }

    #[test]
    fn the_escape_stream_for_a_styled_row_under_a_label() {
        let out = Overlay {
            input: "\x1b[1;31mred\x1b[0m \x1b[4:3;58;5;2mhttp://u\x1b[0m\x1b[44m\x1b[K",
            cols: 20,
            height: 1,
            ..Overlay::default()
        }
        .draw();
        let readable = String::from_utf8(out)
            .expect("the overlay emits utf-8")
            .replace('\x1b', "␛");
        insta::assert_snapshot!(readable);
    }
}

#[cfg(test)]
mod keys {
    use super::{Chosen, Keys, Mode, Pick, Step, Viewport};
    use crate::matcher::Match;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use proptest::prelude::*;

    fn matches(n: usize) -> Vec<Match> {
        (0..n)
            .map(|i| Match {
                extents: Vec::new(),
                width: 1,
                value: format!("v{i}"),
            })
            .collect()
    }

    fn labelled(labels: &[&str]) -> Vec<Option<String>> {
        labels.iter().map(|&l| Some(l.to_owned())).collect()
    }

    fn view() -> Viewport {
        Viewport {
            top: 50,
            height: 10,
            cols: 80,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn press_all(keys: &mut Keys, events: &[KeyEvent]) -> Step {
        let mut view = view();
        for &event in events {
            let step = keys.press(event, &mut view, 100);
            if step != Step::Wait {
                return step;
            }
        }
        Step::Wait
    }

    fn chars(s: &str) -> Vec<KeyEvent> {
        s.chars().map(|c| key(KeyCode::Char(c))).collect()
    }

    fn picked(value: &str, chosen: Chosen) -> Step {
        Step::Done(Some(Pick {
            values: vec![value.to_owned()],
            chosen,
        }))
    }

    #[test]
    fn a_two_key_label_waits_for_its_second_key() {
        let (m, l) = (matches(3), labelled(&["a", "sa", "ss"]));
        let mut keys = Keys::new(&m, &l, Mode::Single);
        assert_eq!(press_all(&mut keys, &chars("s")), Step::Wait);
        assert_eq!(keys.typed, "s");
        assert_eq!(
            press_all(&mut keys, &chars("s")),
            picked("v2", Chosen::Default)
        );
    }

    #[test]
    fn a_key_no_label_continues_starts_over() {
        let (m, l) = (matches(3), labelled(&["a", "sa", "ss"]));
        let mut keys = Keys::new(&m, &l, Mode::Single);
        assert_eq!(press_all(&mut keys, &chars("sx")), Step::Wait);
        assert_eq!(keys.typed, "");
        assert_eq!(
            press_all(&mut keys, &chars("a")),
            picked("v0", Chosen::Default)
        );
    }

    #[test]
    fn backspace_drops_the_last_typed_key() {
        let (m, l) = (matches(3), labelled(&["a", "sa", "ss"]));
        let mut keys = Keys::new(&m, &l, Mode::Single);
        let mut events = chars("s");
        events.push(key(KeyCode::Backspace));
        events.extend(chars("a"));
        assert_eq!(press_all(&mut keys, &events), picked("v0", Chosen::Default));
    }

    #[test]
    fn an_unlabelled_match_cannot_be_picked() {
        let (m, l) = (matches(2), vec![Some("a".to_owned()), None]);
        let mut keys = Keys::new(&m, &l, Mode::Single);
        assert_eq!(press_all(&mut keys, &chars("b")), Step::Wait);
        assert_eq!(keys.typed, "");
    }

    #[test]
    fn a_key_release_is_not_a_keystroke() {
        let (m, l) = (matches(1), labelled(&["a"]));
        let mut keys = Keys::new(&m, &l, Mode::Single);
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        assert_eq!(press_all(&mut keys, &[release]), Step::Wait);
        assert_eq!(keys.typed, "");
    }

    #[test]
    fn escape_ctrl_c_and_ctrl_g_cancel() {
        let (m, l) = (matches(1), labelled(&["a"]));
        for event in [key(KeyCode::Esc), ctrl('c'), ctrl('g')] {
            let mut keys = Keys::new(&m, &l, Mode::Single);
            assert_eq!(
                press_all(&mut keys, &[event]),
                Step::Done(None),
                "{event:?}"
            );
        }
    }

    #[test]
    fn a_control_chord_is_never_typed_as_a_label() {
        let (m, l) = (matches(1), labelled(&["a"]));
        let mut keys = Keys::new(&m, &l, Mode::Single);
        assert_eq!(press_all(&mut keys, &[ctrl('a')]), Step::Wait);
        assert_eq!(keys.typed, "");
    }

    #[test]
    fn an_uppercase_label_key_picks_the_upper_action() {
        let (m, l) = (matches(2), labelled(&["a", "s"]));
        let mut keys = Keys::new(&m, &l, Mode::Upper);
        assert_eq!(
            press_all(&mut keys, &chars("S")),
            picked("v1", Chosen::Upper)
        );
    }

    #[test]
    fn a_lowercase_label_key_picks_the_default_action_even_with_an_upper_action() {
        let (m, l) = (matches(2), labelled(&["a", "s"]));
        let mut keys = Keys::new(&m, &l, Mode::Upper);
        assert_eq!(
            press_all(&mut keys, &chars("s")),
            picked("v1", Chosen::Default)
        );
    }

    #[test]
    fn without_an_upper_action_an_uppercase_key_is_not_its_lowercase_label() {
        let (m, l) = (matches(2), labelled(&["a", "s"]));
        let mut keys = Keys::new(&m, &l, Mode::Single);
        assert_eq!(press_all(&mut keys, &chars("S")), Step::Wait);
    }

    #[test]
    fn an_abandoned_uppercase_prefix_does_not_carry_into_the_next_pick() {
        let (m, l) = (matches(3), labelled(&["a", "sa", "ss"]));
        let mut keys = Keys::new(&m, &l, Mode::Upper);
        assert_eq!(press_all(&mut keys, &chars("Sx")), Step::Wait);
        assert_eq!(
            press_all(&mut keys, &chars("a")),
            picked("v0", Chosen::Default)
        );
    }

    #[test]
    fn an_erased_uppercase_key_does_not_pick_the_upper_action() {
        let (m, l) = (matches(3), labelled(&["a", "sa", "ss"]));
        let mut keys = Keys::new(&m, &l, Mode::Upper);
        let mut events = chars("S");
        events.push(key(KeyCode::Backspace));
        events.extend(chars("a"));
        assert_eq!(press_all(&mut keys, &events), picked("v0", Chosen::Default));
    }

    #[test]
    fn an_erased_uppercase_second_key_does_not_pick_the_upper_action() {
        let (m, l) = (matches(3), labelled(&["a", "saa", "sss"]));
        let mut keys = Keys::new(&m, &l, Mode::Upper);
        let mut events = chars("sA");
        events.push(key(KeyCode::Backspace));
        events.extend(chars("ss"));
        assert_eq!(press_all(&mut keys, &events), picked("v2", Chosen::Default));
    }

    #[test]
    fn an_uppercase_key_kept_after_backspace_still_picks_the_upper_action() {
        let (m, l) = (matches(3), labelled(&["a", "sa", "ss"]));
        let mut keys = Keys::new(&m, &l, Mode::Upper);
        let mut events = chars("S");
        events.push(key(KeyCode::Backspace));
        events.extend(chars("Sa"));
        assert_eq!(press_all(&mut keys, &events), picked("v1", Chosen::Upper));
    }

    #[test]
    fn enter_does_nothing_in_single_pick_mode() {
        let (m, l) = (matches(1), labelled(&["a"]));
        let mut keys = Keys::new(&m, &l, Mode::Single);
        assert_eq!(press_all(&mut keys, &[key(KeyCode::Enter)]), Step::Wait);
    }

    #[test]
    fn multi_toggles_labels_and_enter_confirms_them_in_match_order() {
        let (m, l) = (matches(3), labelled(&["a", "s", "d"]));
        let mut keys = Keys::new(&m, &l, Mode::Multi);
        let mut events = chars("dasa");
        events.push(key(KeyCode::Enter));
        assert_eq!(
            press_all(&mut keys, &events),
            Step::Done(Some(Pick {
                values: vec!["v1".to_owned(), "v2".to_owned()],
                chosen: Chosen::Default,
            }))
        );
    }

    #[test]
    fn multi_clears_the_typed_prefix_after_each_toggle() {
        let (m, l) = (matches(3), labelled(&["a", "sa", "ss"]));
        let mut keys = Keys::new(&m, &l, Mode::Multi);
        assert_eq!(press_all(&mut keys, &chars("sa")), Step::Wait);
        assert_eq!(keys.typed, "");
        assert_eq!(keys.selected, [false, true, false]);
    }

    #[test]
    fn multi_toggles_every_occurrence_that_shares_a_label() {
        let (m, l) = (matches(3), labelled(&["a", "s", "a"]));
        let mut keys = Keys::new(&m, &l, Mode::Multi);
        assert_eq!(press_all(&mut keys, &chars("a")), Step::Wait);
        assert_eq!(keys.selected, [true, false, true]);
        assert_eq!(press_all(&mut keys, &chars("a")), Step::Wait);
        assert_eq!(keys.selected, [false, false, false]);
    }

    #[test]
    fn multi_enter_with_nothing_selected_cancels() {
        let (m, l) = (matches(2), labelled(&["a", "s"]));
        let mut keys = Keys::new(&m, &l, Mode::Multi);
        assert_eq!(
            press_all(&mut keys, &[key(KeyCode::Enter)]),
            Step::Done(None)
        );
    }

    #[test]
    fn each_scroll_key_moves_the_view_by_its_own_distance() {
        let (m, l) = (matches(0), labelled(&[]));
        let cases = [
            (key(KeyCode::Up), 49),
            (key(KeyCode::Down), 51),
            (key(KeyCode::PageUp), 40),
            (key(KeyCode::PageDown), 60),
            (key(KeyCode::Home), 0),
            (key(KeyCode::End), 90),
            (ctrl('u'), 45),
            (ctrl('d'), 55),
            (ctrl('b'), 40),
            (ctrl('f'), 60),
        ];
        for (event, top) in cases {
            let mut keys = Keys::new(&m, &l, Mode::Single);
            let mut view = view();
            assert_eq!(keys.press(event, &mut view, 100), Step::Wait);
            assert_eq!(view.top, top, "{event:?}");
        }
    }

    proptest! {
        #[test]
        fn typing_any_label_picks_exactly_its_match(
            count in 1usize..60,
            pick in any::<proptest::sample::Index>(),
        ) {
            let m = matches(count);
            let names = crate::label::labels(count, crate::alphabet::DEFAULT);
            let labels: Vec<Option<String>> = names.iter().cloned().map(Some).collect();
            let i = pick.index(count);
            let mut keys = Keys::new(&m, &labels, Mode::Single);
            prop_assert_eq!(
                press_all(&mut keys, &chars(&names[i])),
                picked(&format!("v{i}"), Chosen::Default)
            );
        }
    }
}
