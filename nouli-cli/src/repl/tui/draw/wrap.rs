//! Styled line wrapping, byte ranges, and transcript scrolling.
use ratatui::text::{Line, Span};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
/// The transcript's last rows, `scroll` rows up from its end, wrapped to
/// `width`, with `header` as its first rows so it scrolls away with them.
/// Only the lines it shows are wrapped, however long it grows; `scroll`
/// stops at the header's start.
pub(super) fn transcript(
    header: &[Line<'static>],
    lines: &[Line<'static>],
    width: usize,
    height: usize,
    scroll: &mut usize,
) -> Vec<Line<'static>> {
    let mut rows = Vec::new();
    for line in header.iter().chain(lines).rev() {
        if rows.len() >= height + *scroll {
            break;
        }
        rows.extend(wrap_line(line, width).into_iter().rev());
    }
    *scroll = (*scroll).min(rows.len().saturating_sub(height));
    let mut shown: Vec<_> = rows.into_iter().skip(*scroll).take(height).collect();
    shown.reverse();
    shown
}

/// A styled line cut into rows of at most `width` columns.
pub(super) fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    if line.width() <= width {
        return vec![line.clone()];
    }
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let mut spans = Vec::with_capacity(line.spans.len());
    let mut start = 0;
    for span in &line.spans {
        let end = start + span.content.len();
        spans.push((start..end, span.style));
        start = end;
    }
    wrap(&text, width)
        .into_iter()
        .map(|row| {
            let parts: Vec<_> = spans
                .iter()
                .filter_map(|(range, style)| {
                    let (from, to) = (range.start.max(row.start), range.end.min(row.end));
                    (from < to).then(|| Span::styled(text[from..to].to_owned(), *style))
                })
                .collect();
            Line::from(parts).style(line.style)
        })
        .collect()
}

/// Byte ranges of `text` for rows of at most `width` columns, breaking after
/// a space where one fits and inside a word longer than a row. The ranges
/// cover the text without gaps, so every cursor offset falls in one of them.
pub(super) fn wrap(text: &str, width: usize) -> Vec<Range<usize>> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let (mut start, mut used, mut space) = (0, 0, None);
    for (at, grapheme) in text.grapheme_indices(true) {
        if grapheme == "\n" {
            rows.push(start..at);
            start = at + 1;
            used = 0;
            space = None;
            continue;
        }
        let columns = grapheme.width();
        if used + columns > width && at > start {
            let end = space.filter(|&after| after > start).unwrap_or(at);
            rows.push(start..end);
            start = end;
            used = text[start..at].width();
            space = None;
            if used + columns > width && at > start {
                rows.push(start..at);
                start = at;
                used = 0;
            }
        }
        used += columns;
        if grapheme.chars().all(char::is_whitespace) {
            space = Some(at + grapheme.len());
        }
    }
    rows.push(start..text.len());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_wrap_after_spaces_and_cover_every_byte() {
        let text = "the quick brown fox";
        let rows = wrap(text, 10);
        let parts: Vec<_> = rows.iter().map(|row| &text[row.clone()]).collect();
        assert_eq!(parts, ["the quick ", "brown fox"]);
        let parts: Vec<_> = wrap("abcdefghijkl", 5)
            .into_iter()
            .map(|row| &"abcdefghijkl"[row])
            .collect();
        assert_eq!(parts, ["abcde", "fghij", "kl"]);
        // Wide characters count two columns each.
        assert_eq!(wrap("東京東京", 5).len(), 2);
        assert!(matches!(wrap("", 5).as_slice(), [row] if *row == (0..0)));
        for width in 1..12 {
            let rows = wrap("a bb ccc dddd eeeee", width);
            assert_eq!(rows.first().unwrap().start, 0);
            assert_eq!(rows.last().unwrap().end, 19);
            assert!(rows.windows(2).all(|pair| pair[0].end == pair[1].start));
        }
    }

    #[test]
    fn the_transcript_shows_its_end_and_scrolls_back_to_its_start() {
        let lines: Vec<Line<'static>> = (0..10).map(|n| Line::from(n.to_string())).collect();
        let text = |rows: Vec<Line<'static>>| -> Vec<String> {
            rows.iter().map(ToString::to_string).collect()
        };
        let mut scroll = 0;
        assert_eq!(
            text(transcript(&[], &lines, 20, 3, &mut scroll)),
            ["7", "8", "9"]
        );
        scroll = 2;
        assert_eq!(
            text(transcript(&[], &lines, 20, 3, &mut scroll)),
            ["5", "6", "7"]
        );
        scroll = 100;
        assert_eq!(
            text(transcript(&[], &lines, 20, 3, &mut scroll)),
            ["0", "1", "2"]
        );
        assert_eq!(scroll, 7);
        // A header is the transcript's first rows, and scrolling reaches it.
        let header = [Line::from("h0"), Line::from("h1")];
        scroll = 100;
        assert_eq!(
            text(transcript(&header, &lines, 20, 3, &mut scroll)),
            ["h0", "h1", "0"]
        );
        assert_eq!(scroll, 9);
        let long = [Line::from("aaaa bbbb cccc")];
        let mut scroll = 0;
        assert_eq!(
            text(transcript(&[], &long, 5, 3, &mut scroll)),
            ["aaaa ", "bbbb ", "cccc"]
        );
    }
}
