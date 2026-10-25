//! Display columns come from `felis-vt` and `felis-grid` rather than a width
//! table of pouncer's own, so wide glyphs, combining marks, and escapes land
//! where felis puts them (docs/design.md "Alignment").

use crate::style::Style;
use crossterm::style::Colored;
use felis_grid::{Grapheme, Grid};
use felis_vt::Parser;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Content {
    Glyph(String),
    /// Painted, never matched.
    Blank,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) content: Content,
    pub(crate) col: u16,
    pub(crate) width: u16,
    /// Resolved at read time: a `StyleId` does not outlive the grid.
    pub(crate) style: Style,
}

impl Cell {
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        match &self.content {
            Content::Glyph(s) => s,
            Content::Blank => "",
        }
    }
}

/// Cells that paint nothing are dropped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) cells: Vec<Cell>,
    /// Read from `felis-grid`'s soft-wrap bit, not guessed from a row that
    /// reaches `cols`: a hard `\n` from a physical-row producer must still
    /// separate rows (docs/design.md "Soft-wrapped matches").
    pub(crate) continued: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Screen {
    pub(crate) rows: Vec<Row>,
    pub(crate) scrolled_off: u64,
}

pub(crate) fn trim_trailing_empty(rows: &mut Vec<Row>) {
    while rows.last().is_some_and(|r| r.cells.is_empty()) {
        rows.pop();
    }
}

#[cfg(test)]
pub(crate) fn screen(input: &[u8], rows: u16, cols: u16) -> Vec<Row> {
    parse(input, rows, cols).rows
}

#[must_use]
pub(crate) fn parse(input: &[u8], rows: u16, cols: u16) -> Screen {
    let normalized = normalize_newlines(input);
    let mut grid = Grid::new(rows, cols);
    let mut parser = Parser::new();
    parser.advance(&mut grid, &normalized);
    // The memoized flag, not the env lookup: it is the one the renderer's own
    // colour commands consult, so a blank is kept exactly when it will paint.
    let colors_enabled = !Colored::ansi_color_disabled_memoized();
    Screen {
        rows: (0..rows)
            .map(|row| read_row(&grid, row, cols, colors_enabled))
            .collect(),
        scrolled_off: grid.scrollback_total_pushed(),
    }
}

/// Map a bare LF to CR+LF as a line discipline's `ONLCR` would: `felis-vt`
/// treats LF as cursor-down only, so `\n`-separated text from `cat` or
/// `tmux capture-pane -p` would drift right a line's width at a time.
fn normalize_newlines(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut prev = 0u8;
    for &byte in input {
        if byte == b'\n' && prev != b'\r' {
            out.push(b'\r');
        }
        out.push(byte);
        prev = byte;
    }
    out
}

fn read_row(grid: &Grid, row: u16, cols: u16, colors_enabled: bool) -> Row {
    let mut cells = Vec::new();
    for col in 0..cols {
        let Some(cell) = grid.cell(row, col) else {
            break;
        };
        let style = Style::from(grid.style(cell.style));
        let content = match cell.grapheme {
            Grapheme::Ascii(b) => Content::Glyph(char::from(b).to_string()),
            Grapheme::Char(c) => Content::Glyph(c.to_string()),
            // A wide glyph's own cell carries the text; its `Spacer` and a
            // sized-run spacer carry no glyph of their own. The spacer holds
            // the owner's pen, so skipping it loses no style either.
            Grapheme::Cluster(id) => match grid.cluster_str(id) {
                Some(s) => Content::Glyph(s.to_owned()),
                None => continue,
            },
            // An erased cell keeps the pen it was erased under (BCE). Only a
            // pen that puts ink on a space survives: the trailing-row trim and
            // the viewport read a row of such blanks as content.
            Grapheme::Empty if style.paints_blank(colors_enabled) => Content::Blank,
            Grapheme::Empty | Grapheme::Spacer | Grapheme::SizedSpacer => continue,
        };
        let followed_by_spacer = col + 1 < cols
            && matches!(
                grid.cell(row, col + 1).map(|c| c.grapheme),
                Some(Grapheme::Spacer)
            );
        let width = if followed_by_spacer && matches!(content, Content::Glyph(_)) {
            2
        } else {
            1
        };
        cells.push(Cell {
            content,
            col,
            width,
            style,
        });
    }
    Row {
        cells,
        continued: grid.row_soft_wrap_continued(row),
    }
}

#[cfg(test)]
mod tests {
    use super::{Cell, Content, Row, parse, screen, trim_trailing_empty};
    use crate::style::Style;
    use crate::test_support::AnsiColor;
    use crossterm::style::Color;

    fn glyph(text: &str, col: u16, width: u16) -> Cell {
        Cell {
            content: Content::Glyph(text.to_owned()),
            col,
            width,
            style: Style::default(),
        }
    }

    fn texts(row: &Row) -> Vec<&str> {
        row.cells.iter().map(Cell::text).collect()
    }

    #[test]
    fn a_row_wrapped_from_the_row_above_is_marked_continued() {
        // The fact the matcher joins logical rows on: felis-grid records that
        // row 1 only exists because row 0 ran out of columns.
        let rows = screen(b"abcdef", 2, 3);
        assert!(!rows[0].continued);
        assert!(rows[1].continued);
    }

    #[test]
    fn a_row_that_exactly_fills_the_width_does_not_mark_the_next_continued() {
        // "Filled to cols" is not evidence of a wrap: these are two lines
        // separated by a hard newline, and the grid says so.
        let rows = screen(b"abc\ndef", 2, 3);
        assert!(!rows[0].continued);
        assert!(!rows[1].continued);
    }

    #[test]
    fn a_wide_glyph_takes_two_columns_and_shifts_what_follows() {
        // The bug class tmux-thumbs falls into: 'b' must land at column 3,
        // not 2, because the CJK glyph occupies two columns. felis-grid gets
        // this right, so pouncer does too.
        let rows = screen("a世b".as_bytes(), 1, 10);
        assert_eq!(
            rows[0].cells,
            vec![glyph("a", 0, 1), glyph("世", 1, 2), glyph("b", 3, 1)]
        );
    }

    #[test]
    fn a_colored_cell_carries_its_foreground_and_its_neighbours_do_not() {
        let rows = screen(b"a\x1b[31mb\x1b[0mc", 1, 10);
        assert_eq!(rows[0].cells[0].style, Style::default());
        assert_eq!(rows[0].cells[1].style.fg, Color::AnsiValue(1));
        assert_eq!(rows[0].cells[2].style, Style::default());
    }

    #[test]
    fn a_wide_glyph_carries_its_style() {
        let rows = screen("a\x1b[32m世\x1b[0mb".as_bytes(), 1, 10);
        let wide = &rows[0].cells[1];
        assert_eq!(wide.text(), "世");
        assert_eq!(wide.width, 2);
        assert_eq!(wide.style.fg, Color::AnsiValue(2));
    }

    #[test]
    fn a_colored_erase_to_end_of_line_yields_styled_blanks() {
        // BCE: the erased cells keep the pen, which is a full-width coloured
        // background the overlay must repaint, not empty space.
        let _colors = AnsiColor::set(true);
        let rows = screen(b"ab\x1b[41m\x1b[K", 1, 5);
        assert_eq!(rows[0].cells.len(), 5);
        for cell in &rows[0].cells[2..] {
            assert_eq!(cell.content, Content::Blank);
            assert_eq!(cell.style.bg, Color::AnsiValue(1));
            assert_eq!(cell.width, 1);
        }
    }

    #[test]
    fn a_default_pen_erase_yields_no_cells() {
        let rows = screen(b"ab\x1b[K", 1, 5);
        assert_eq!(texts(&rows[0]), ["a", "b"]);
    }

    #[test]
    fn a_foreground_only_erase_leaves_the_rows_empty() {
        // felis stamps the whole pen on an erased cell, so a fg-only or
        // bold-only clear would otherwise fill every row with blanks that
        // paint nothing, and the trailing-row trim would keep them, opening
        // the viewport on emptiness while the real content scrolls away.
        let _colors = AnsiColor::set(true);
        for erase in [
            &b"\x1b[31m\x1b[2J\x1b[1;1Hhit"[..],
            &b"\x1b[1m\x1b[2J\x1b[1;1Hhit"[..],
        ] {
            let rows = screen(erase, 4, 6);
            assert_eq!(texts(&rows[0]), ["h", "i", "t"], "{erase:?}");
            for row in &rows[1..] {
                assert!(row.cells.is_empty(), "{erase:?} left blanks: {row:?}");
            }
        }
    }

    #[test]
    fn a_background_erase_still_keeps_its_blanks() {
        let _colors = AnsiColor::set(true);
        let rows = screen(b"\x1b[41m\x1b[2J\x1b[1;1Hhit", 4, 6);
        for row in &rows[1..] {
            assert_eq!(row.cells.len(), 6);
            assert!(row.cells.iter().all(|c| c.content == Content::Blank));
        }
    }

    #[test]
    fn a_background_erase_leaves_the_rows_empty_under_no_color() {
        // crossterm suppresses the background under NO_COLOR, so these cells
        // would repaint as plain spaces; keeping them would push every match
        // off the tail-aligned viewport.
        let _colors = AnsiColor::set(false);
        let rows = screen(b"\x1b[41m\x1b[2J\x1b[1;1Hhit", 4, 6);
        assert_eq!(texts(&rows[0]), ["h", "i", "t"]);
        for row in &rows[1..] {
            assert!(row.cells.is_empty(), "left blanks under NO_COLOR: {row:?}");
        }
    }

    #[test]
    fn a_reverse_erase_keeps_its_blanks_under_no_color() {
        // Reverse is an attribute, not a colour: it survives NO_COLOR, so the
        // blanks it paints are still content.
        let _colors = AnsiColor::set(false);
        let rows = screen(b"\x1b[7m\x1b[2J\x1b[1;1Hhit", 4, 6);
        for row in &rows[1..] {
            assert_eq!(row.cells.len(), 6);
            assert!(row.cells.iter().all(|c| c.content == Content::Blank));
        }
    }

    #[test]
    fn trimming_drops_only_the_empty_rows_after_the_last_content() {
        let mut rows = screen(b"a\n\nb\n", 6, 10);
        trim_trailing_empty(&mut rows);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].cells, []);
    }

    #[test]
    fn input_that_fits_reports_nothing_scrolled_off() {
        assert_eq!(parse(b"a\nb", 2, 10).scrolled_off, 0);
    }

    #[test]
    fn input_taller_than_the_grid_keeps_the_tail_and_counts_the_lost_head() {
        let parsed = parse(b"a\nb\nc\nd", 2, 10);
        assert_eq!(parsed.scrolled_off, 2);
        assert_eq!(texts(&parsed.rows[0]), ["c"]);
        assert_eq!(texts(&parsed.rows[1]), ["d"]);
    }
}

#[cfg(test)]
mod proptests {
    use super::{Content, normalize_newlines, parse, screen};
    use proptest::prelude::*;

    fn without_cr_before_lf(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        for (i, &byte) in bytes.iter().enumerate() {
            if byte == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                continue;
            }
            out.push(byte);
        }
        out
    }

    proptest! {
        #[test]
        fn any_bytes_parse_into_cells_that_fit_the_grid_without_overlapping(
            input in proptest::collection::vec(any::<u8>(), 0..512),
            rows in 1u16..8,
            cols in 1u16..40,
        ) {
            let screen = parse(&input, rows, cols);
            prop_assert_eq!(screen.rows.len(), usize::from(rows));
            for row in &screen.rows {
                let mut next_free = 0u16;
                for cell in &row.cells {
                    prop_assert!(cell.col >= next_free, "{:?}", row.cells);
                    prop_assert!((1..=2).contains(&cell.width));
                    prop_assert!(cell.col + cell.width <= cols, "{:?}", row.cells);
                    if let Content::Glyph(text) = &cell.content {
                        prop_assert!(!text.is_empty());
                    }
                    next_free = cell.col + cell.width;
                }
            }
        }

        #[test]
        fn printable_ascii_lines_read_back_verbatim_one_per_row(
            lines in proptest::collection::vec("[!-~][ -~]{0,18}[!-~]|[!-~]", 0..6),
        ) {
            let input = lines.join("\n");
            let rows = screen(input.as_bytes(), lines.len() as u16 + 1, 20);
            for (row, line) in rows.iter().zip(&lines) {
                let text: String = row.cells.iter().map(super::Cell::text).collect();
                prop_assert_eq!(&text, line);
                prop_assert!(!row.continued);
                prop_assert_eq!(row.cells.first().map(|c| c.col), Some(0));
            }
        }

        #[test]
        fn normalized_newlines_always_follow_a_cr_and_lose_nothing_else(
            input in proptest::collection::vec(prop_oneof![
                Just(b'\n'), Just(b'\r'), Just(b'a'), any::<u8>(),
            ], 0..64),
        ) {
            let out = normalize_newlines(&input);
            for (i, &byte) in out.iter().enumerate() {
                if byte == b'\n' {
                    prop_assert!(i > 0 && out[i - 1] == b'\r', "{:?}", out);
                }
            }
            prop_assert_eq!(without_cr_before_lf(&out), without_cr_before_lf(&input));
            prop_assert_eq!(normalize_newlines(&out), out);
        }
    }
}
