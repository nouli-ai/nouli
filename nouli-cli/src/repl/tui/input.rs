//! Windows' console backend delivers bracketed-paste markers as key records.
//! Normalize them before application key bindings can interpret pasted content.
use crossterm::event::Event;
#[cfg(unix)]
use crossterm::event::EventStream;
#[cfg(unix)]
use futures_util::StreamExt;
use std::io;
#[cfg(any(windows, test))]
mod sequences;
#[cfg(windows)]
mod windows;

pub(super) struct Input {
    #[cfg(unix)]
    events: EventStream,
    #[cfg(windows)]
    events: windows::Events,
    #[cfg(windows)]
    paste: Paste,
}
impl Input {
    #[cfg_attr(
        unix,
        allow(
            clippy::unnecessary_wraps,
            reason = "the Windows input source is fallible"
        )
    )]
    pub(super) fn new() -> io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            events: EventStream::new(),
            #[cfg(windows)]
            events: windows::Events::new()?,
            #[cfg(windows)]
            paste: Paste::default(),
        })
    }
    pub(super) async fn next(&mut self) -> Option<io::Result<Event>> {
        #[cfg(unix)]
        {
            self.events.next().await
        }
        #[cfg(windows)]
        {
            loop {
                if !self.paste.pending()
                    && let Some(event) = self.paste.queued.pop_front()
                {
                    return Some(Ok(event));
                }
                let next = if self.paste.pending() {
                    let deadline = self.paste.deadline.expect("pending input has a deadline");
                    match tokio::time::timeout_at(deadline.into(), self.events.next()).await {
                        Ok(event) => event,
                        Err(_) => return Some(self.paste.expire()),
                    }
                } else {
                    self.events.next().await
                };
                match next {
                    Some(Ok(event)) => match self.paste.push(event) {
                        Ok(Some(event)) => return Some(Ok(event)),
                        Ok(None) => {}
                        Err(error) => return Some(Err(error)),
                    },
                    other => return other,
                }
            }
        }
    }
}

#[cfg(any(windows, test))]
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
#[cfg(any(windows, test))]
#[derive(Default)]
struct Paste {
    prefix: String,
    original: Vec<Event>,
    content: Option<String>,
    queued: std::collections::VecDeque<Event>,
    deadline: Option<std::time::Instant>,
}
#[cfg(any(windows, test))]
impl Paste {
    fn pending(&self) -> bool {
        !self.prefix.is_empty() || self.content.is_some()
    }
    fn defer_auxiliary(&mut self, event: &Event) -> bool {
        // Auxiliary records must never split a marker or activate paste content.
        match event {
            Event::Mouse(_) => return true,
            Event::Resize(..) => self
                .queued
                .retain(|event| !matches!(event, Event::Resize(..))),
            Event::FocusGained | Event::FocusLost => self
                .queued
                .retain(|event| !matches!(event, Event::FocusGained | Event::FocusLost)),
            _ => return false,
        }
        self.queued.push_back(event.clone());
        true
    }
    fn push(&mut self, event: Event) -> io::Result<Option<Event>> {
        const START: &str = "\x1b[200~";
        const END: &str = "\x1b[201~";
        if self.pending() && self.defer_auxiliary(&event) {
            return Ok(None);
        }
        let character = match &event {
            Event::Key(key) if key.kind == KeyEventKind::Release => return Ok(None),
            Event::Key(key) => match key.code {
                KeyCode::Esc => Some('\x1b'),
                KeyCode::Enter => Some('\r'),
                KeyCode::Tab => Some('\t'),
                KeyCode::Backspace => Some('\x08'),
                KeyCode::Char(character) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT)
                        && character.is_ascii_alphabetic()
                    {
                        char::from_u32(u32::from(character.to_ascii_uppercase()) & 0x1f)
                    } else {
                        Some(character)
                    }
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(content) = &mut self.content {
            if let Some(character) = character {
                self.deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
                content.push(character);
                if content.len() > 16 * 1024 * 1024 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "paste exceeds 16 MiB",
                    ));
                }
                if content.ends_with(END) {
                    self.deadline = None;
                    content.truncate(content.len() - END.len());
                    let text = self
                        .content
                        .take()
                        .unwrap()
                        .replace("\r\n", "\n")
                        .replace('\r', "\n");
                    return Ok(Some(Event::Paste(text)));
                }
            } else if matches!(
                event,
                Event::Resize(..) | Event::FocusGained | Event::FocusLost
            ) {
                self.queued.push_back(event);
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unsupported key inside paste",
                ));
            }
            return Ok(None);
        }
        if self.prefix.is_empty() && character != Some('\x1b') {
            return Ok(Some(event));
        }
        self.original.push(event);
        self.deadline = Some(std::time::Instant::now() + std::time::Duration::from_millis(30));
        if let Some(character) = character {
            self.prefix.push(character);
        } else {
            self.prefix.push('\0');
        }
        if self.prefix == START {
            self.prefix.clear();
            self.original.clear();
            self.content = Some(String::new());
            self.deadline = Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
            return Ok(None);
        }
        if START.starts_with(&self.prefix) {
            return Ok(None);
        }
        match sequences::parse(&self.prefix) {
            sequences::Sequence::Pending => Ok(None),
            sequences::Sequence::Complete(event) => {
                self.prefix.clear();
                self.deadline = None;
                self.original.clear();
                Ok(event)
            }
        }
    }
    fn expire(&mut self) -> io::Result<Event> {
        self.deadline = None;
        if self.content.take().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "incomplete bracketed paste",
            ));
        }
        self.prefix.clear();
        self.queued.extend(self.original.drain(..));
        self.queued
            .pop_front()
            .ok_or_else(|| io::Error::other("no pending terminal input"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    fn key(character: char) -> Event {
        Event::Key(KeyEvent::new(
            match character {
                '\x1b' => KeyCode::Esc,
                '\r' => KeyCode::Enter,
                _ => KeyCode::Char(character),
            },
            KeyModifiers::NONE,
        ))
    }
    #[test]
    fn pasted_commands_and_control_keys_are_one_literal_event() {
        let mut paste = Paste::default();
        let text = "/logout local\r\n\x13旅行";
        let mut output = Vec::new();
        for character in format!("\x1b[200~{text}\x1b[201~").chars() {
            if let Some(event) = paste.push(key(character)).unwrap() {
                output.push(event);
            }
        }
        assert_eq!(output, vec![Event::Paste("/logout local\n\x13旅行".into())]);
        assert!(!paste.pending());
    }
    #[test]
    fn escape_and_unrelated_sequences_keep_their_original_keys() {
        let mut paste = Paste::default();
        assert!(paste.push(key('\x1b')).unwrap().is_none());
        assert_eq!(paste.expire().unwrap(), key('\x1b'));
        let mut output = None;
        for c in "\x1b[A".chars() {
            output = paste.push(key(c)).unwrap();
        }
        assert_eq!(
            output,
            Some(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)))
        );
    }

    #[test]
    fn mouse_resize_and_focus_cannot_break_paste_markers_or_execute_controls() {
        use crossterm::event::{MouseEvent, MouseEventKind};
        let mut paste = Paste::default();
        let mouse = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 2,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });
        assert!(paste.push(key('\x1b')).unwrap().is_none());
        assert!(paste.push(mouse.clone()).unwrap().is_none());
        assert!(paste.push(Event::Resize(80, 24)).unwrap().is_none());
        for character in "[200~".chars() {
            assert!(paste.push(key(character)).unwrap().is_none());
        }
        assert!(paste.push(mouse).unwrap().is_none());
        assert!(paste.push(Event::FocusLost).unwrap().is_none());
        let mut output = Vec::new();
        for character in "/logout local\x13\x1b[201~".chars() {
            if let Some(event) = paste.push(key(character)).unwrap() {
                output.push(event);
            }
        }
        assert_eq!(output, [Event::Paste("/logout local\x13".into())]);
        assert_eq!(paste.queued.pop_front(), Some(Event::Resize(80, 24)));
        assert_eq!(paste.queued.pop_front(), Some(Event::FocusLost));
    }
}
