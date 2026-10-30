//! Citrus accents identify question types across the form and saved results.
use nouli_core::decision::AnswerSpace;
use ratatui::style::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Noul,
    Choice,
    Score,
}

impl Tone {
    pub(crate) fn for_space(space: &AnswerSpace) -> Self {
        match space {
            AnswerSpace::Noul => Self::Noul,
            AnswerSpace::Choice { .. } => Self::Choice,
            AnswerSpace::Score { .. } => Self::Score,
        }
    }

    pub(crate) fn color(self) -> Color {
        if true_color() {
            match self {
                Self::Noul => Color::Rgb(163, 230, 53),
                Self::Choice => Color::Rgb(103, 232, 249),
                Self::Score => Color::Rgb(253, 164, 175),
            }
        } else {
            Color::Indexed(match self {
                Self::Noul => 155,
                Self::Choice => 87,
                Self::Score => 217,
            })
        }
    }

    pub(crate) fn ansi(self) -> &'static str {
        match (true_color(), self) {
            (true, Self::Noul) => "\x1b[1;38;2;163;230;53m",
            (true, Self::Choice) => "\x1b[1;38;2;103;232;249m",
            (true, Self::Score) => "\x1b[1;38;2;253;164;175m",
            (false, Self::Noul) => "\x1b[1;38;5;155m",
            (false, Self::Choice) => "\x1b[1;38;5;87m",
            (false, Self::Score) => "\x1b[1;38;5;217m",
        }
    }
}

fn true_color() -> bool {
    std::env::var("COLORTERM")
        .is_ok_and(|value| matches!(value.to_ascii_lowercase().as_str(), "truecolor" | "24bit"))
}
