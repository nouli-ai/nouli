//! The model picker, drawn over the bottom of the transcript like the `@` and
//! `/` lists.
use super::{App, chrome::Theme};
use crate::connect;
use crate::repl::tui::model_picker::{Keyed, Picker, Row, Step};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const ROWS: usize = 8;
const GUTTER: usize = 5;

/// The picker's lines, and the column its cursor sits at on the first or
/// second row.
pub(super) fn lines(
    app: &App,
    picker: &Picker,
    width: usize,
    theme: &Theme,
) -> (Vec<Line<'static>>, (usize, usize)) {
    match &picker.step {
        Step::List => list(app, picker, width, theme),
        Step::Services => (services(picker, theme), (0, 0)),
        Step::Key(keyed) => key(keyed, picker, theme),
    }
}

fn key(keyed: &Keyed, picker: &Picker, theme: &Theme) -> (Vec<Line<'static>>, (usize, usize)) {
    let name = match keyed {
        Keyed::Model(choice) => choice.provider.as_str(),
        Keyed::Service(service) => service.as_str(),
    };
    let dots = picker.key.chars().count();
    (
        vec![
            Line::from(Span::styled(format!("API key for {name}"), theme.accent)),
            Line::from(format!("  {}", "•".repeat(dots))),
        ],
        (2 + dots, 1),
    )
}

fn services(picker: &Picker, theme: &Theme) -> Vec<Line<'static>> {
    let names: Vec<&str> = connect::quick_services()
        .iter()
        .map(|(name, _)| *name)
        .collect();
    let rows = names.into_iter().chain(["Other…"]);
    let mut lines = vec![Line::from(Span::styled("Add connection", theme.accent))];
    lines.extend(rows.enumerate().map(|(index, name)| {
        if index == picker.choice {
            Line::from(Span::styled(format!(" › {name}"), theme.accent))
        } else {
            Line::from(format!("   {name}"))
        }
    }));
    lines
}

fn list(
    app: &App,
    picker: &Picker,
    width: usize,
    theme: &Theme,
) -> (Vec<Line<'static>>, (usize, usize)) {
    let rows = app.picker_rows(&picker.query);
    let chosen = picker.choice.min(rows.len().saturating_sub(1));
    let first = chosen.saturating_sub(ROWS - 1);
    let shown = rows.iter().skip(first).take(ROWS);
    let current = app.current_choice();
    let longest = rows
        .iter()
        .filter_map(|row| match row {
            Row::Model(choice) => Some(choice.model.width()),
            Row::Typed(choice) => Some("Use ".width() + choice.model.width()),
            Row::AddConnection => None,
        })
        .max()
        .unwrap_or(0);
    let available = width.saturating_sub(GUTTER);
    let provider_width = rows
        .iter()
        .filter_map(|row| match row {
            Row::Model(choice) | Row::Typed(choice) => Some(choice.provider.width()),
            Row::AddConnection => None,
        })
        .max()
        .unwrap_or(0)
        .min(available / 2);
    let column = (longest + 3).min(available.saturating_sub(provider_width));
    let mut lines = vec![if picker.query.is_empty() {
        Line::from(vec![Span::raw("  "), Span::styled("Model", theme.dim)])
    } else {
        Line::from(format!("  {}", picker.query))
    }];
    for (index, row) in shown.enumerate() {
        let on = index + first == chosen;
        let style = if on { theme.accent } else { Style::new() };
        let lead = Span::styled(if on { " › " } else { "   " }, theme.accent);
        lines.push(match row {
            Row::Model(choice) => {
                let tick = Some(choice) == current.as_ref();
                Line::from(vec![
                    lead,
                    Span::styled(if tick { "✓ " } else { "  " }, theme.accent),
                    Span::styled(model_column(&choice.model, column), style),
                    Span::styled(choice.provider.clone(), theme.dim),
                ])
            }
            Row::Typed(choice) => Line::from(vec![
                lead,
                Span::raw("  "),
                Span::styled(
                    model_column(&format!("Use {}", choice.model), column),
                    style,
                ),
                Span::styled(choice.provider.clone(), theme.dim),
            ]),
            Row::AddConnection => Line::from(vec![
                lead,
                Span::raw("  "),
                Span::styled("Add connection", if on { theme.accent } else { theme.dim }),
            ]),
        });
    }
    (lines, (2 + picker.query.width(), 0))
}

/// Leave one cell between columns, clipping whole graphemes by display width.
fn model_column(text: &str, column: usize) -> String {
    let mut clipped = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let width = grapheme.width();
        if used + width > column.saturating_sub(1) {
            break;
        }
        clipped.push_str(grapheme);
        used += width;
    }
    clipped.push_str(&" ".repeat(column.saturating_sub(used)));
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tui::{overlay::Overlay, tests::*};

    #[test]
    fn long_and_wide_model_ids_keep_the_provider_in_a_separate_column() {
        for model in [
            "creator/a-very-long-model-name",
            "作者/模型模型模型模型",
            "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}",
        ] {
            let mut settings = settings();
            settings.config = nouli_core::config::Config::parse(&format!(
                "default_provider = \"local\"\ndefault_model = \"{model}\"\n[providers.local]\nprotocol = \"system-one\"\nbase_url = \"http://127.0.0.1:9\"\n"
            )).unwrap();
            let mut app = app(&settings);
            for query in ["", "arbitrary-long-model-id"] {
                app.open_model_picker(query);
                let Some(Overlay::Models(picker)) = &app.overlay else {
                    unreachable!()
                };
                for width in [16, 24, 40] {
                    let (rows, _) = lines(&app, picker, width, &Theme::new(false));
                    for row in rows.iter().skip(1).filter(|line| {
                        line.spans
                            .last()
                            .is_some_and(|span| span.content == "local")
                    }) {
                        let provider_at: usize = row.spans[..row.spans.len() - 1]
                            .iter()
                            .map(|span| span.content.width())
                            .sum();
                        assert!(provider_at + "local".width() <= width, "{row} at {width}");
                        assert!(row.spans[row.spans.len() - 2].content.ends_with(' '));
                    }
                }
            }
        }
    }
}
