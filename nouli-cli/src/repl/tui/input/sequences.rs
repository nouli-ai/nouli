//! VT input that Conhost passes through while bracketed-paste input is enabled.
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

pub(super) enum Sequence {
    Pending,
    Complete(Option<Event>),
}
pub(super) fn parse(prefix: &str) -> Sequence {
    if prefix == "\x1b" || prefix == "\x1b[" || prefix == "\x1bO" {
        return Sequence::Pending;
    }
    let body = prefix
        .strip_prefix("\x1b[")
        .or_else(|| prefix.strip_prefix("\x1bO"));
    let Some(body) = body else {
        let mut characters = prefix.chars();
        characters.next();
        return Sequence::Complete(
            characters
                .next()
                .filter(|_| characters.next().is_none())
                .map(|character| {
                    Event::Key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::ALT))
                }),
        );
    };
    let Some(last) = body.chars().last() else {
        return Sequence::Pending;
    };
    if body.len() <= 64 && (last.is_ascii_digit() || matches!(last, ';' | '<')) {
        return Sequence::Pending;
    }
    let parameters = &body[..body.len() - last.len_utf8()];
    if let Some(mouse) = parameters.strip_prefix('<') {
        return Sequence::Complete(mouse_event(mouse, last));
    }
    let parameters: Option<Vec<u32>> = if parameters.is_empty() {
        Some(vec![])
    } else {
        parameters
            .split(';')
            .map(|part| part.parse().ok())
            .collect()
    };
    let Some(parameters) = parameters else {
        return Sequence::Complete(None);
    };
    let mut modifiers = modifiers(parameters.get(1).copied().unwrap_or(1));
    let code = match last {
        'A' => KeyCode::Up,
        'B' => KeyCode::Down,
        'C' => KeyCode::Right,
        'D' => KeyCode::Left,
        'H' => KeyCode::Home,
        'F' => KeyCode::End,
        'Z' => {
            modifiers |= KeyModifiers::SHIFT;
            KeyCode::BackTab
        }
        'P' => KeyCode::F(1),
        'Q' => KeyCode::F(2),
        'R' => KeyCode::F(3),
        'S' => KeyCode::F(4),
        '~' => match parameters.first().copied().unwrap_or(0) {
            1 | 7 => KeyCode::Home,
            2 => KeyCode::Insert,
            3 => KeyCode::Delete,
            4 | 8 => KeyCode::End,
            5 => KeyCode::PageUp,
            6 => KeyCode::PageDown,
            11..=15 => KeyCode::F(u8::try_from(parameters[0] - 10).unwrap()),
            17..=21 => KeyCode::F(u8::try_from(parameters[0] - 11).unwrap()),
            23..=24 => KeyCode::F(u8::try_from(parameters[0] - 12).unwrap()),
            _ => return Sequence::Complete(None),
        },
        _ => return Sequence::Complete(None),
    };
    Sequence::Complete(Some(Event::Key(KeyEvent::new(code, modifiers))))
}
fn modifiers(value: u32) -> KeyModifiers {
    let bits = value.saturating_sub(1);
    let mut result = KeyModifiers::NONE;
    if bits & 1 != 0 {
        result |= KeyModifiers::SHIFT;
    }
    if bits & 2 != 0 {
        result |= KeyModifiers::ALT;
    }
    if bits & 4 != 0 {
        result |= KeyModifiers::CONTROL;
    }
    result
}
fn mouse_event(parameters: &str, last: char) -> Option<Event> {
    if !matches!(last, 'M' | 'm') {
        return None;
    }
    let values: Vec<u16> = parameters
        .split(';')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let [code, x, y] = *values.as_slice() else {
        return None;
    };
    // Back/Forward and further buttons have no crossterm MouseButton variant.
    if code >= 128 {
        return None;
    }
    let mut modifiers = KeyModifiers::NONE;
    if code & 4 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if code & 8 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if code & 16 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    let button = match code & 3 {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    };
    let kind = if code & 64 != 0 {
        match code & 3 {
            0 => MouseEventKind::ScrollUp,
            1 => MouseEventKind::ScrollDown,
            2 => MouseEventKind::ScrollLeft,
            _ => MouseEventKind::ScrollRight,
        }
    } else if code & 32 != 0 {
        button.map_or(MouseEventKind::Moved, MouseEventKind::Drag)
    } else if last == 'm' {
        MouseEventKind::Up(button?)
    } else if last == 'M' {
        MouseEventKind::Down(button?)
    } else {
        return None;
    };
    Some(Event::Mouse(MouseEvent {
        kind,
        column: x.checked_sub(1)?,
        row: y.checked_sub(1)?,
        modifiers,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_modifiers_and_sgr_mouse_are_decoded() {
        for (text, code, modifiers) in [
            ("\x1b[A", KeyCode::Up, KeyModifiers::NONE),
            ("\x1b[1;5D", KeyCode::Left, KeyModifiers::CONTROL),
            ("\x1b[Z", KeyCode::BackTab, KeyModifiers::SHIFT),
            ("\x1b[17~", KeyCode::F(6), KeyModifiers::NONE),
            ("\x1bOP", KeyCode::F(1), KeyModifiers::NONE),
        ] {
            let Sequence::Complete(Some(Event::Key(key))) = parse(text) else {
                panic!("{text:?}");
            };
            assert_eq!(key, KeyEvent::new(code, modifiers));
        }
        let Sequence::Complete(Some(Event::Mouse(mouse))) = parse("\x1b[<0;89;1M") else {
            panic!()
        };
        assert_eq!(
            (mouse.column, mouse.row, mouse.kind),
            (88, 0, MouseEventKind::Down(MouseButton::Left))
        );
        assert!(matches!(parse("\x1b[<0;89;"), Sequence::Pending));
        assert!(matches!(
            parse("\x1b[<128;10;10M"),
            Sequence::Complete(None)
        ));
    }
}
