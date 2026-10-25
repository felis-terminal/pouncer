use crate::source::{Content, Row};
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Match {
    /// One per physical row the match covers, top to bottom.
    pub(crate) extents: Vec<Extent>,
    /// Summed over every row the match spans, and over the whole match even
    /// when `value` is a capture group.
    pub(crate) width: u16,
    /// Never contains a newline: the rows of a soft wrap are joined with
    /// nothing between them.
    pub(crate) value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Extent {
    pub(crate) row: usize,
    pub(crate) start: u16,
    /// Exclusive.
    pub(crate) end: u16,
}

#[derive(Debug, Clone, Copy)]
struct Span {
    byte: usize,
    len: usize,
    row: usize,
    col: u16,
    width: u16,
}

struct Hit {
    start: usize,
    end: usize,
    pattern: usize,
    value: String,
}

#[must_use]
pub(crate) fn find(rows: &[Row], patterns: &[Regex], capture: Option<usize>) -> Vec<Match> {
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < rows.len() {
        let mut end = start + 1;
        while end < rows.len() && rows[end].continued {
            end += 1;
        }
        let (text, spans) = layout(&rows[start..end], start);
        out.extend(find_in_logical_row(&text, &spans, patterns, capture));
        start = end;
    }
    out
}

fn layout(rows: &[Row], first_row: usize) -> (String, Vec<Span>) {
    let mut text = String::new();
    let mut spans = Vec::with_capacity(rows.iter().map(|r| r.cells.len()).sum());
    for (offset, row) in rows.iter().enumerate() {
        for cell in &row.cells {
            // A styled blank is paint, not text: giving it a span would let a
            // pattern match through a coloured erase as if it were a space.
            let Content::Glyph(glyph) = &cell.content else {
                continue;
            };
            spans.push(Span {
                byte: text.len(),
                len: glyph.len(),
                row: first_row + offset,
                col: cell.col,
                width: cell.width,
            });
            text.push_str(glyph);
        }
    }
    (text, spans)
}

fn find_in_logical_row(
    text: &str,
    spans: &[Span],
    patterns: &[Regex],
    capture: Option<usize>,
) -> Vec<Match> {
    let mut hits: Vec<Hit> = Vec::new();
    for (pattern, re) in patterns.iter().enumerate() {
        for caps in re.captures_iter(text) {
            let Some(whole) = caps.get(0) else { continue };
            if whole.start() == whole.end() {
                continue;
            }
            let value = match capture {
                Some(n) => match caps.get(n) {
                    Some(g) => g.as_str().to_owned(),
                    None => continue, // optional group did not participate
                },
                None => whole.as_str().to_owned(),
            };
            hits.push(Hit {
                start: whole.start(),
                end: whole.end(),
                pattern,
                value,
            });
        }
    }

    // On the same start the earlier pattern wins; `pattern_strings` puts every
    // `--regex` ahead of the presets for this.
    hits.sort_by_key(|hit| (hit.start, hit.pattern));

    let widths = prefix_widths(spans);
    let mut matches = Vec::new();
    let mut consumed_to = 0usize;
    for hit in hits {
        if !matches.is_empty() && hit.start < consumed_to {
            continue;
        }
        matches.push(Match {
            extents: extents_of(spans, hit.start, hit.end),
            width: width_of(spans, &widths, hit.start, hit.end),
            value: hit.value,
        });
        consumed_to = hit.end;
    }
    matches
}

/// `widths[i]` is the total width of `spans[..i]`.
fn prefix_widths(spans: &[Span]) -> Vec<usize> {
    std::iter::once(0)
        .chain(spans.iter().scan(0usize, |sum, span| {
            *sum += usize::from(span.width);
            Some(*sum)
        }))
        .collect()
}

/// The spans whose text shares a byte with `[start, end)`. A regex can match
/// inside a cluster (only the combining mark of `é`), and that glyph is still
/// the one the match is drawn on.
fn covered(spans: &[Span], start: usize, end: usize) -> std::ops::Range<usize> {
    spans.partition_point(|span| span.byte + span.len <= start)
        ..spans.partition_point(|span| span.byte < end)
}

fn extents_of(spans: &[Span], start: usize, end: usize) -> Vec<Extent> {
    let mut out: Vec<Extent> = Vec::new();
    for span in &spans[covered(spans, start, end)] {
        match out.last_mut() {
            Some(extent) if extent.row == span.row => extent.end = span.col + span.width,
            _ => out.push(Extent {
                row: span.row,
                start: span.col,
                end: span.col + span.width,
            }),
        }
    }
    out
}

fn width_of(spans: &[Span], widths: &[usize], start: usize, end: usize) -> u16 {
    let range = covered(spans, start, end);
    u16::try_from(widths[range.end] - widths[range.start]).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{Extent, find};
    use crate::source::screen;
    use crate::test_support::AnsiColor;
    use regex::Regex;

    fn re(p: &str) -> Regex {
        Regex::new(p).expect("test regex compiles")
    }

    #[test]
    fn capture_group_sets_value_but_label_and_width_cover_the_whole_match() {
        let rows = screen(b"PR #1234 merged", 1, 40);
        let m = find(&rows, &[re(r"#(\d+)")], Some(1));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].value, "1234");
        assert_eq!(m[0].extents[0].start, 3); // the '#'
        assert_eq!(m[0].width, 5);
    }

    #[test]
    fn a_url_soft_wrapped_at_the_grid_edge_is_one_match() {
        // The whole point of matching logical rows: a URL longer than the grid
        // is one link, not two halves neither of which is usable.
        let rows = screen(b"http://example.com/abcdefghij", 4, 20);
        let m = find(&rows, &[re(r"https?://\S+")], None);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].value, "http://example.com/abcdefghij");
        assert!(!m[0].value.contains('\n'));
        // The label anchors where the URL starts, on the leading row.
        assert_eq!((m[0].extents[0].row, m[0].extents[0].start), (0, 0));
    }

    #[test]
    fn two_lines_are_not_joined_when_the_first_fills_the_width() {
        // A row that happens to reach the last column is not a wrap: the hard
        // newline between the rows keeps them two matches.
        let rows = screen(b"aaaaaaaa\nbbbbbbbb", 4, 8);
        let m = find(&rows, &[re(r"[ab]+")], None);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].value.as_str(), m[0].extents[0].row), ("aaaaaaaa", 0));
        assert_eq!((m[1].value.as_str(), m[1].extents[0].row), ("bbbbbbbb", 1));
    }

    #[test]
    fn a_wide_glyph_pushed_over_the_wrap_keeps_its_columns() {
        // A CJK glyph that will not fit in the last column moves whole to the
        // next row, so the match spanning the wrap must read columns per row:
        // 'd' at column 3 of row 0, 世 at column 0 of row 1.
        let rows = screen("abcd世xy".as_bytes(), 4, 5);
        let m = find(&rows, &[re("d世x")], None);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].extents[0].row, m[0].extents[0].start), (0, 3));
        // 'x' sits at column 2 of row 1, past the wide glyph's two columns;
        // column 4 of row 0, left empty by the wrap, is not part of the match.
        assert_eq!(
            m[0].extents,
            [
                Extent {
                    row: 0,
                    start: 3,
                    end: 4
                },
                Extent {
                    row: 1,
                    start: 0,
                    end: 3
                },
            ]
        );
        assert_eq!(m[0].width, 4);
    }

    #[test]
    fn width_of_a_match_wider_than_u16_saturates() {
        let rows = screen(&vec![b'a'; 70_000], 1_000, 80);
        let m = find(&rows, &[re("a+")], None);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].width, u16::MAX);
    }

    #[test]
    fn a_match_starting_on_a_continuation_row_anchors_there() {
        // Joining rows must not drag every label onto the leading row: a match
        // that begins after the wrap belongs where its first glyph is.
        let rows = screen(b"aaaaaaaaaaxyz", 4, 10);
        let m = find(&rows, &[re("xyz")], None);
        assert_eq!(m.len(), 1);
        assert_eq!((m[0].extents[0].row, m[0].extents[0].start), (1, 0));
    }

    #[test]
    fn styled_blanks_around_a_url_contribute_no_bytes_and_no_columns() {
        // A coloured erase paints the rest of the row; the URL must still
        // match whole, at its own column, with no blank folded into the value.
        let _colors = AnsiColor::set(true);
        // Erase first so the blanks sit *before* the match, then CUP to
        // column 5: the label must still anchor there, not at column 0.
        let rows = screen(b"\x1b[44m\x1b[K\x1b[5Ghttp://a.io", 1, 30);
        let m = find(&rows, &[re(r"https?://\S+")], None);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].value, "http://a.io");
        assert!(!m[0].value.contains(' '));
        assert_eq!(m[0].extents[0].start, 4);
    }

    #[test]
    fn a_match_inside_a_cluster_covers_the_cluster_glyph() {
        // `e` + U+0301 is one cell; the regex matches only the mark.
        let rows = screen("xe\u{301}y".as_bytes(), 1, 10);
        let m = find(&rows, &[re("\u{301}")], None);
        assert_eq!(m.len(), 1);
        assert_eq!(
            m[0].extents,
            [Extent {
                row: 0,
                start: 1,
                end: 2
            }]
        );
        assert_eq!(m[0].width, 1);
    }
}

#[cfg(test)]
mod proptests {
    #![allow(clippy::expect_used)]

    use super::find;
    use crate::source::{Content, screen};
    use proptest::prelude::*;
    use regex::Regex;

    const PATTERNS: &[&str] = &["[ab]+", "a漢", "b", "a*", "漢+b?"];

    fn text_strategy() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            prop_oneof![Just('a'), Just('b'), Just(' '), Just('漢')],
            0..40,
        )
        .prop_map(|chars| chars.into_iter().collect())
    }

    fn columns(s: &str) -> u16 {
        s.chars().map(|c| if c == '漢' { 2 } else { 1 }).sum()
    }

    /// A wide glyph pushed past the edge leaves a gap, so `columns / cols`
    /// undercounts; one row per char cannot scroll at any `cols >= 2`.
    fn rows_for(text: &str) -> u16 {
        text.chars().count() as u16 + 1
    }

    proptest! {
        #[test]
        fn one_unwrapped_row_matches_like_regex_find_iter_at_display_columns(
            text in text_strategy(),
            pattern in proptest::sample::select(PATTERNS),
        ) {
            let re = Regex::new(pattern).expect("pattern compiles");
            let rows = screen(text.as_bytes(), 1, 100);
            let got = find(&rows, std::slice::from_ref(&re), None);
            let want: Vec<(u16, u16, &str)> = re
                .find_iter(&text)
                .filter(|m| !m.is_empty())
                .map(|m| (columns(&text[..m.start()]), columns(m.as_str()), m.as_str()))
                .collect();
            let got: Vec<(u16, u16, &str)> = got
                .iter()
                .inspect(|m| assert_eq!(m.extents[0].row, 0))
                .map(|m| (m.extents[0].start, m.width, m.value.as_str()))
                .collect();
            prop_assert_eq!(got, want);
        }

        #[test]
        fn soft_wrapping_at_any_width_finds_the_same_values(
            text in text_strategy(),
            patterns in proptest::sample::subsequence(PATTERNS, 1..=PATTERNS.len()).prop_shuffle(),
            cols in 2u16..12,
        ) {
            let patterns: Vec<Regex> =
                patterns.iter().map(|p| Regex::new(p).expect("pattern compiles")).collect();
            let values = |rows: &[crate::source::Row]| -> Vec<String> {
                find(rows, &patterns, None).into_iter().map(|m| m.value).collect()
            };
            let wide = screen(text.as_bytes(), 1, 100);
            let narrow = screen(text.as_bytes(), rows_for(&text), cols);
            prop_assert_eq!(values(&narrow), values(&wide));
        }

        #[test]
        fn every_match_is_anchored_on_its_own_first_glyph(
            text in text_strategy(),
            patterns in proptest::sample::subsequence(PATTERNS, 1..=PATTERNS.len()).prop_shuffle(),
            cols in 2u16..12,
        ) {
            let patterns: Vec<Regex> =
                patterns.iter().map(|p| Regex::new(p).expect("pattern compiles")).collect();
            let rows = screen(text.as_bytes(), rows_for(&text), cols);
            let matches = find(&rows, &patterns, None);
            for pair in matches.windows(2) {
                prop_assert!((pair[0].extents[0].row, pair[0].extents[0].start) < (pair[1].extents[0].row, pair[1].extents[0].start));
            }
            for m in &matches {
                prop_assert!(!m.value.is_empty());
                prop_assert_eq!(m.width, columns(&m.value));
                let anchor = rows[m.extents[0].row].cells.iter().find(|c| c.col == m.extents[0].start);
                let first = m.value.chars().next().map(String::from);
                prop_assert_eq!(
                    anchor.map(|c| c.content.clone()),
                    first.map(Content::Glyph),
                );
            }
        }
    }
}
