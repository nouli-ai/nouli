//! A terminal adaptation of the brand kit's honey bee, heading the transcript.
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Rows the greeter adds above the transcript: the bee with a row of
/// breathing room above and below it.
pub(super) const ROWS: u16 = 10;
/// The bee's width, with the gap before the copy beside it.
const BEE_COLUMNS: usize = 23;

/// The greeter as transcript rows, so it scrolls away as output arrives.
/// Narrow transcripts get none; wide ones also get the copy beside the bee.
pub(super) fn lines(width: u16, color: bool, drafting: bool) -> Vec<Line<'static>> {
    if width < 22 {
        return Vec::new();
    }
    let bold = Style::new().add_modifier(Modifier::BOLD);
    // Brand-kit honey, teal, and teal glass, with 256-color equivalents.
    let rgb = std::env::var("COLORTERM")
        .is_ok_and(|value| matches!(value.to_ascii_lowercase().as_str(), "truecolor" | "24bit"));
    let tone = |red, green, blue, indexed| {
        if !color {
            bold
        } else if rgb {
            bold.fg(Color::Rgb(red, green, blue))
        } else {
            bold.fg(Color::Indexed(indexed))
        }
    };
    let honey = tone(246, 187, 62, 221);
    let teal = tone(11, 110, 99, 29);
    let wings = tone(168, 226, 214, 152);
    let face = if color {
        honey
            .fg(if rgb {
                Color::Rgb(18, 22, 21)
            } else {
                Color::Indexed(233)
            })
            .bg(if rgb {
                Color::Rgb(246, 187, 62)
            } else {
                Color::Indexed(221)
            })
    } else {
        bold
    };
    let eyes = if drafting { " >   _ " } else { " >   < " };
    let bee = vec![
        Line::from(Span::styled("     ▄       ▄", honey)),
        Line::from(vec![
            Span::styled("      ╲", teal),
            Span::styled(" ▄█▄ ", honey),
            Span::styled("╱", teal),
        ]),
        Line::from(Span::styled("     ▄███████▄", honey)),
        Line::from(vec![
            Span::styled(" ▄█▄ ", wings),
            Span::styled("█", honey),
            Span::styled(eyes, face),
            Span::styled("█", honey),
            Span::styled(" ▄█▄", wings),
        ]),
        Line::from(vec![
            Span::styled(" ▀█▀ ", wings),
            Span::styled("▀", honey),
            Span::styled("   ▿   ", face),
            Span::styled("▀", honey),
            Span::styled(" ▀█▀", wings),
        ]),
        Line::from(vec![
            Span::styled(" ▄▀▄ ", wings),
            Span::styled(" ███████ ", honey),
            Span::styled(" ▄▀▄", wings),
        ]),
        Line::from(Span::styled("      ▀█████▀", teal)),
        Line::from(Span::styled("        ▀█▀", honey)),
    ];
    // Beside the bee's third, fifth and sixth rows, when there is room.
    let copy = if width >= 64 {
        vec![
            (2, Span::styled("Let's make a decision.", bold)),
            (4, Span::raw("Add context below, then ask a question.")),
            (
                5,
                Span::styled(
                    "/help for a hand · /sessions to pick up",
                    Style::new().add_modifier(Modifier::DIM),
                ),
            ),
        ]
    } else {
        Vec::new()
    };
    let mut rows = vec![Line::default()];
    for (index, mut row) in bee.into_iter().enumerate() {
        if let Some((_, text)) = copy.iter().find(|(at, _)| *at == index) {
            let pad = BEE_COLUMNS.saturating_sub(row.width());
            row.spans.push(Span::raw(" ".repeat(pad)));
            row.spans.push(text.clone());
        }
        rows.push(row);
    }
    rows.push(Line::default());
    rows
}

#[cfg(test)]
mod tests {
    use super::lines;
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Modifier},
        widgets::{Paragraph, Widget},
    };

    #[test]
    fn the_copy_sits_beside_the_bee_in_the_default_color() {
        let area = Rect::new(0, 0, 80, 10);
        let mut buffer = Buffer::empty(area);
        Paragraph::new(lines(80, true, false)).render(area, &mut buffer);
        for (y, symbol) in [(3, "L"), (5, "A"), (6, "/")] {
            let cell = &buffer[(23, y)];
            assert_eq!((cell.symbol(), cell.fg), (symbol, Color::Reset), "row {y}");
        }
        assert!(buffer[(23, 3)].modifier.contains(Modifier::BOLD));
        assert!(lines(40, true, false).iter().all(|row| row.width() < 23));
        assert_eq!(
            lines(21, true, false),
            [] as [ratatui::prelude::Line<'_>; 0]
        );
    }
}
