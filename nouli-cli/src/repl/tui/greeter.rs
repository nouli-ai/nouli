//! The brand kit's honey bee as coloured half-block cells, heading the transcript.
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// The transcript height that fits the bee with the row of breathing room
/// under it. Taller views also get the row above it.
pub(super) const ROWS: u16 = 15;
/// The bee's width. Narrower transcripts have no greeter.
const BEE_COLUMNS: usize = 25;
/// Where the copy beside the bee starts.
const COPY_COLUMN: usize = 28;
/// The narrowest transcript with room for the copy beside the bee.
const COPY_WIDTH: u16 = 67;
const FACE_COLUMN: usize = 9;
const FACE_COLUMNS: usize = 7;
const EYES_ROW: usize = 5;
const MOUTH_ROW: usize = 6;

/// The bee sampled from the brand kit's SVG, as two half-rows per text row.
/// Each character names a colour in [`rgb`] and `.` leaves a half-cell empty.
/// The rows below the head are symmetric about column 12, the face's axis.
const PIXELS: [&str; 28] = [
    ".........................",
    "........h.......h........",
    ".........................",
    "............l............",
    "..........gllllt.........",
    ".........llllllll........",
    ".......llllllllllll......",
    "......lllllllllhsss......",
    "......lllllssssssss......",
    "......llllhssssssss......",
    "...S..lllssssssssssMMMM..",
    "..MSSSlllssssssssssMMMMM.",
    ".MMMSSllsssssssssssMMMMM.",
    ".MMMMSlssssssssssssMMMMS.",
    ".MMMMMlssssssssssssMMSSS.",
    ".WWMMMMSssssssssslMSSSS..",
    "..WWWWWM.sssssss..SSMMM..",
    "..SSMMS.hhhssshhh.WMMMM..",
    "..MMMMMSssssssgggSSMMMMM.",
    "..MMMMWStttdddddd..SSSS..",
    "...MWW..sssgggggg........",
    "........ssggggggg........",
    "........tdddddddd........",
    "........ggggggggg........",
    "........ggggggggg........",
    "..........ggggg..........",
    "...........ggg...........",
    ".........................",
];

/// A pixel's brand-kit colour and its 256-colour equivalent.
fn rgb(pixel: u8) -> Option<(u8, u8, u8, u8)> {
    Some(match pixel {
        b'h' => (246, 187, 62, 221),
        b'g' => (224, 166, 30, 178),
        b'l' => (248, 197, 83, 221),
        b's' => (226, 176, 58, 179),
        b't' => (11, 110, 99, 29),
        b'd' => (0, 91, 81, 23),
        b'W' => (210, 241, 234, 195),
        b'M' => (168, 226, 214, 152),
        b'S' => (125, 208, 192, 116),
        _ => return None,
    })
}

/// How the terminal shows colours, if it does.
#[derive(Clone, Copy)]
enum Depth {
    Plain,
    Indexed,
    TrueColor,
}

impl Depth {
    fn color(self, (red, green, blue, indexed): (u8, u8, u8, u8)) -> Option<Color> {
        match self {
            Self::Plain => None,
            Self::Indexed => Some(Color::Indexed(indexed)),
            Self::TrueColor => Some(Color::Rgb(red, green, blue)),
        }
    }

    /// One text cell from its top and bottom half-cells.
    fn cell(self, top: u8, bottom: u8) -> Span<'static> {
        let (top, bottom) = (rgb(top), rgb(bottom));
        let glyph = match (top, bottom) {
            (None, None) => " ",
            (Some(top), Some(bottom)) if top == bottom => "█",
            (Some(_), Some(_)) if matches!(self, Self::Plain) => "█",
            (None, Some(_)) => "▄",
            (Some(_), _) => "▀",
        };
        let mut style = Style::new();
        if let Some(color) = top.or(bottom).and_then(|tone| self.color(tone)) {
            style = style.fg(color);
        }
        if let Some(color) = top.and(bottom).and_then(|tone| self.color(tone)) {
            style = style.bg(color);
        }
        Span::styled(glyph, style)
    }
}

/// The greeter as transcript rows, so it scrolls away as output arrives.
/// Narrow transcripts get none; wide ones also get the copy beside the bee.
pub(super) fn lines(width: u16, color: bool, drafting: bool) -> Vec<Line<'static>> {
    if usize::from(width) < BEE_COLUMNS {
        return Vec::new();
    }
    let depth = match (color, crate::palette::true_color()) {
        (false, _) => Depth::Plain,
        (true, false) => Depth::Indexed,
        (true, true) => Depth::TrueColor,
    };
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut face = bold;
    if let (Some(ink), Some(skin)) = (
        depth.color((18, 22, 21, 233)),
        rgb(b's').and_then(|tone| depth.color(tone)),
    ) {
        face = face.fg(ink).bg(skin);
    }
    let eyes = if drafting { " >   _ " } else { " >   < " };
    let copy: &[(usize, Span<'static>)] = &[
        (3, Span::styled("Let's make a decision.", bold)),
        (5, Span::raw("Add context below, then ask a question.")),
        (
            6,
            Span::styled(
                "/help for a hand · /sessions to pick up",
                Style::new().add_modifier(Modifier::DIM),
            ),
        ),
    ];
    let mut rows = vec![Line::default()];
    for (index, halves) in PIXELS.as_chunks::<2>().0.iter().enumerate() {
        let mut row = bee_row(
            index,
            halves,
            depth,
            match index {
                EYES_ROW => Some(Span::styled(eyes, face)),
                MOUTH_ROW => Some(Span::styled("   ▿   ", face)),
                _ => None,
            },
        );
        if width >= COPY_WIDTH
            && let Some((_, text)) = copy.iter().find(|(at, _)| *at == index)
        {
            row.spans
                .push(Span::raw(" ".repeat(COPY_COLUMN - BEE_COLUMNS)));
            row.spans.push(text.clone());
        }
        rows.push(row);
    }
    rows.push(Line::default());
    rows
}

/// One row of the bee, with the face text over its cells and the antenna
/// stems as line glyphs that stay visible at this size.
fn bee_row(
    index: usize,
    halves: &[&str; 2],
    depth: Depth,
    mut face: Option<Span<'static>>,
) -> Line<'static> {
    let (top, bottom) = (halves[0].as_bytes(), halves[1].as_bytes());
    let mut spans = Vec::with_capacity(BEE_COLUMNS);
    let mut column = 0;
    while column < BEE_COLUMNS {
        if column == FACE_COLUMN
            && let Some(text) = face.take()
        {
            spans.push(text);
            column += FACE_COLUMNS;
            continue;
        }
        let stem = match (index, column) {
            (1, 9) => Some("╲"),
            (1, 15) => Some("╱"),
            _ => None,
        };
        spans.push(match stem {
            Some(glyph) => {
                let teal = rgb(b't').and_then(|tone| depth.color(tone));
                Span::styled(
                    glyph,
                    teal.map_or(Style::new(), |color| Style::new().fg(color)),
                )
            }
            None => depth.cell(top[column], bottom[column]),
        });
        column += 1;
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::{BEE_COLUMNS, COPY_COLUMN, COPY_WIDTH, EYES_ROW, MOUTH_ROW, PIXELS, lines, rgb};
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Modifier},
        text::Line,
        widgets::{Paragraph, Widget},
    };

    fn text(row: &Line<'_>) -> String {
        row.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn the_copy_sits_beside_the_bee_in_the_default_color() {
        let area = Rect::new(0, 0, 80, 16);
        let mut buffer = Buffer::empty(area);
        Paragraph::new(lines(80, true, false)).render(area, &mut buffer);
        let column = u16::try_from(COPY_COLUMN).unwrap();
        for (y, symbol) in [(4, "L"), (6, "A"), (7, "/")] {
            let cell = &buffer[(column, y)];
            assert_eq!((cell.symbol(), cell.fg), (symbol, Color::Reset), "row {y}");
        }
        assert!(buffer[(column, 4)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn narrow_transcripts_keep_the_bee_without_its_copy_until_it_cannot_fit() {
        assert!(
            lines(COPY_WIDTH - 1, true, false)
                .iter()
                .all(|row| row.width() == 0 || row.width() == BEE_COLUMNS)
        );
        assert!(
            lines(COPY_WIDTH, true, false)
                .iter()
                .any(|row| row.width() > 66)
        );
        assert_eq!(lines(25, true, false).len(), 16);
        assert_eq!(lines(24, true, false), Vec::<Line<'_>>::new());
    }

    #[test]
    fn every_half_row_is_the_bees_width_in_known_colors() {
        for row in PIXELS {
            assert_eq!(row.len(), BEE_COLUMNS, "{row}");
            for pixel in row.bytes() {
                assert!(pixel == b'.' || rgb(pixel).is_some(), "{row}");
            }
        }
    }

    #[test]
    fn the_body_tapers_symmetrically_about_the_face() {
        let axis = BEE_COLUMNS / 2;
        // Half-rows 20 and on are below the wings' lowest body contact.
        for row in &PIXELS[20..] {
            let body: Vec<_> = row
                .bytes()
                .enumerate()
                .filter(|(column, pixel)| *pixel != b'.' && (6..=18).contains(column))
                .map(|(column, _)| column)
                .collect();
            if let (Some(first), Some(last)) = (body.first(), body.last()) {
                assert_eq!(axis - first, last - axis, "{row}");
            }
        }
    }

    #[test]
    fn the_expression_changes_while_drafting_and_the_face_stays_centered() {
        let row = |drafting, index: usize| text(&lines(80, true, drafting)[index + 1]);
        let idle = row(false, EYES_ROW);
        assert!(idle.contains(">   <"), "{idle}");
        assert!(row(true, EYES_ROW).contains(">   _"));
        let axis = BEE_COLUMNS / 2;
        let chars: Vec<char> = idle.chars().collect();
        assert_eq!((chars[axis - 2], chars[axis + 2]), ('>', '<'));
        assert_eq!(
            row(false, MOUTH_ROW).chars().nth(axis),
            Some('▿'),
            "the mouth sits on the axis"
        );
    }

    #[test]
    fn the_antenna_stems_and_tips_stay_visible() {
        let top = text(&lines(80, true, false)[1]);
        assert_eq!(top.matches('▄').count(), 2, "{top}");
        let stems = text(&lines(80, true, false)[2]);
        assert!(stems.contains('╲') && stems.contains('╱'), "{stems}");
    }

    #[test]
    fn without_color_the_bee_is_a_plain_silhouette() {
        let rows = lines(80, false, false);
        for row in &rows {
            for span in &row.spans {
                assert_eq!((span.style.fg, span.style.bg), (None, None));
            }
        }
        assert!(text(&rows[EYES_ROW + 1]).contains(">   <"));
    }
}
