//! A terminal adaptation of the brand kit's honey bee, shown before output.
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

/// Leave a row of breathing room above and below the bee. Short terminals
/// keep the ordinary one-line header and all the available editing space.
pub(super) fn draw(frame: &mut Frame, area: Rect, color: bool, drafting: bool) {
    if area.width < 22 || area.height < 10 {
        return;
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
        Line::styled("     ▄       ▄", honey),
        Line::from(vec![
            Span::styled("      ╲", teal),
            Span::styled(" ▄█▄ ", honey),
            Span::styled("╱", teal),
        ]),
        Line::styled("     ▄███████▄", honey),
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
        Line::styled("      ▀█████▀", teal),
        Line::styled("        ▀█▀", honey),
    ];
    let bee_area = Rect {
        y: area.y + 1,
        height: 8,
        ..area
    };
    frame.render_widget(Paragraph::new(bee), bee_area);
    if area.width >= 64 {
        let copy = vec![
            Line::styled("Let's make a decision.", bold),
            Line::default(),
            Line::from("Add context below, then ask a question."),
            Line::styled(
                "/help for a hand · /sessions to pick up",
                Style::new().add_modifier(Modifier::DIM),
            ),
        ];
        frame.render_widget(
            Paragraph::new(copy),
            Rect {
                x: area.x + 23,
                y: area.y + 3,
                width: area.width - 23,
                height: 4,
            },
        );
    }
}
