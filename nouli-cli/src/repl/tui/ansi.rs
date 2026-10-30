//! Printed lines as styled lines. The loop prints a few SGR codes (bold, dim,
//! a foreground color, and reset); they become styles, and every other escape
//! or control character is dropped, so nothing printed can move the cursor.
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// One printed line, without its newline.
pub(super) fn line(text: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut rest = text;
    while let Some(at) = rest.find('\x1b') {
        push(&mut spans, &rest[..at], style);
        rest = &rest[at + 1..];
        let Some(body) = rest.strip_prefix('[') else {
            continue;
        };
        // A control sequence runs to its final byte, one of @ through ~.
        let Some(end) = body.find(|c: char| ('@'..='~').contains(&c)) else {
            rest = "";
            break;
        };
        if body[end..].starts_with('m') {
            style = sgr(style, &body[..end]);
        }
        rest = &body[end + 1..];
    }
    push(&mut spans, rest, style);
    Line::from(spans)
}

fn push(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    let text: String = text
        .chars()
        .filter_map(|c| match c {
            '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    if !text.is_empty() {
        spans.push(Span::styled(text, style));
    }
}

fn sgr(mut style: Style, parameters: &str) -> Style {
    // An empty or unreadable parameter means 0, a reset.
    let mut codes = parameters
        .split(';')
        .map(|code| code.parse::<u8>().unwrap_or(0));
    while let Some(code) = codes.next() {
        style = match code {
            0 => Style::default(),
            1 => style.add_modifier(Modifier::BOLD),
            2 => style.add_modifier(Modifier::DIM),
            22 => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            30..=37 => style.fg(Color::Indexed(code - 30)),
            90..=97 => style.fg(Color::Indexed(code - 82)),
            39 => style.fg(Color::Reset),
            38 => match codes.next() {
                Some(5) => codes
                    .next()
                    .map_or(style, |index| style.fg(Color::Indexed(index))),
                Some(2) => match (codes.next(), codes.next(), codes.next()) {
                    (Some(red), Some(green), Some(blue)) => style.fg(Color::Rgb(red, green, blue)),
                    _ => style,
                },
                _ => style,
            },
            _ => style,
        };
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::turn::{DIM, RESET, TEAL};

    fn parts(line: &Line) -> Vec<(String, Style)> {
        line.spans
            .iter()
            .map(|span| (span.content.to_string(), span.style))
            .collect()
    }

    #[test]
    fn the_codes_the_loop_prints_become_styles() {
        let teal = Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Indexed(37));
        let printed = format!("  {TEAL}▶ Rail{RESET} 0.90 {DIM}Air{RESET}");
        assert_eq!(
            parts(&line(&printed)),
            [
                ("  ".into(), Style::default()),
                ("▶ Rail".into(), teal),
                (" 0.90 ".into(), Style::default()),
                ("Air".into(), Style::default().add_modifier(Modifier::DIM)),
            ]
        );
        assert_eq!(parts(&line("plain")), [("plain".into(), Style::default())]);
        assert!(line("").spans.is_empty());
    }

    #[test]
    fn other_escapes_and_control_characters_never_reach_the_screen() {
        let shown = line("a\x1b[2Jb\x1b]0;title\x07c\td\re\x1b[31");
        let text: String = shown
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        // The title sequence loses its escape, so its text stays readable.
        assert_eq!(text, "ab]0;titlec de");
        assert!(!text.contains(char::is_control));
        let rgb = line("\x1b[38;2;1;2;3mx");
        assert_eq!(rgb.spans[0].style.fg, Some(Color::Rgb(1, 2, 3)));
    }
}
