//! Owned native input preserves VK=0 control records that crossterm discards.
//! In particular, VT bracketed-paste wrappers must reach the paste normalizer.
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::{
    io,
    mem::zeroed,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
    sync::Arc,
    thread,
};
use tokio::sync::mpsc;
use windows_sys::Win32::{
    Foundation::{WAIT_FAILED, WAIT_OBJECT_0},
    System::{
        Console::{
            CONSOLE_SCREEN_BUFFER_INFO, FOCUS_EVENT, GetConsoleMode, GetConsoleScreenBufferInfo,
            GetStdHandle, INPUT_RECORD, KEY_EVENT, KEY_EVENT_RECORD, LEFT_ALT_PRESSED,
            LEFT_CTRL_PRESSED, MOUSE_EVENT, MOUSE_EVENT_RECORD, MOUSE_HWHEELED, MOUSE_MOVED,
            MOUSE_WHEELED, RIGHT_ALT_PRESSED, RIGHT_CTRL_PRESSED, ReadConsoleInputW, SHIFT_PRESSED,
            STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, WINDOW_BUFFER_SIZE_EVENT,
        },
        Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects},
    },
    UI::Input::KeyboardAndMouse::{
        VK_A, VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F24, VK_HOME, VK_INSERT,
        VK_LEFT, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP, VK_Z,
    },
};

pub(super) struct Events {
    receiver: mpsc::Receiver<io::Result<Event>>,
    stop: Arc<OwnedHandle>,
    reader: Option<thread::JoinHandle<()>>,
}
impl Events {
    pub(super) fn new() -> io::Result<Self> {
        // SAFETY: standard input is borrowed for the reader's lifetime; the
        // stop event is independently owned and closed only after joining it.
        let (input, stop) = unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0;
            if GetConsoleMode(input, &raw mut mode) == 0 {
                return Err(io::Error::last_os_error());
            }
            let event = CreateEventW(ptr::null(), 1, 0, ptr::null());
            if event.is_null() {
                return Err(io::Error::last_os_error());
            }
            (
                input as usize,
                Arc::new(OwnedHandle::from_raw_handle(event)),
            )
        };
        let (sender, receiver) = mpsc::channel(256);
        let signal = stop.clone();
        let reader = thread::spawn(move || {
            if let Err(error) = read_events(input, &signal, &sender) {
                let _ = sender.blocking_send(Err(error));
            }
        });
        Ok(Self {
            receiver,
            stop,
            reader: Some(reader),
        })
    }
    pub(super) async fn next(&mut self) -> Option<io::Result<Event>> {
        self.receiver.recv().await
    }
}
impl Drop for Events {
    fn drop(&mut self) {
        // Closing the receiver also unblocks a reader waiting on a full queue.
        self.receiver.close();
        // SAFETY: the stop handle is live through the following join.
        unsafe {
            SetEvent(self.stop.as_raw_handle());
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn read_events(
    input: usize,
    stop: &OwnedHandle,
    sender: &mpsc::Sender<io::Result<Event>>,
) -> io::Result<()> {
    let handles = [stop.as_raw_handle(), input as _];
    let mut decoder = Decoder::default();
    loop {
        // SAFETY: both handles remain live; only this thread reads console input.
        let status = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
        if status == WAIT_OBJECT_0 {
            return Ok(());
        }
        if status == WAIT_FAILED {
            return Err(io::Error::last_os_error());
        }
        if status != WAIT_OBJECT_0 + 1 {
            return Err(io::Error::other("unexpected console wait result"));
        }
        let mut records: [INPUT_RECORD; 64] = unsafe { zeroed() };
        let mut count = 0;
        // SAFETY: the input has one reader and is signaled; the array has 64 slots.
        if unsafe { ReadConsoleInputW(input as _, records.as_mut_ptr(), 64, &raw mut count) } == 0 {
            return Err(io::Error::last_os_error());
        }
        for record in &records[..count as usize] {
            let event = decoder.event(record);
            let repeat = if u32::from(record.EventType) == KEY_EVENT {
                // SAFETY: EventType selects the initialized key union member.
                unsafe { record.Event.KeyEvent.wRepeatCount.max(1) }
            } else {
                1
            };
            if let Some(event) = event {
                for _ in 0..repeat {
                    if sender.blocking_send(Ok(event.clone())).is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct Decoder {
    surrogate: Option<u16>,
    buttons: u32,
}
impl Decoder {
    fn event(&mut self, record: &INPUT_RECORD) -> Option<Event> {
        // SAFETY: the console initializes the union member selected by EventType.
        unsafe {
            match u32::from(record.EventType) {
                KEY_EVENT => self.key(&record.Event.KeyEvent),
                MOUSE_EVENT => self.mouse(&record.Event.MouseEvent),
                WINDOW_BUFFER_SIZE_EVENT => {
                    let size = record.Event.WindowBufferSizeEvent.dwSize;
                    Some(Event::Resize(
                        size.X.try_into().ok()?,
                        size.Y.try_into().ok()?,
                    ))
                }
                FOCUS_EVENT => Some(if record.Event.FocusEvent.bSetFocus != 0 {
                    Event::FocusGained
                } else {
                    Event::FocusLost
                }),
                _ => None,
            }
        }
    }
    fn key(&mut self, key: &KEY_EVENT_RECORD) -> Option<Event> {
        // SAFETY: Unicode input uses the UnicodeChar member of KEY_EVENT_RECORD.
        let unicode = unsafe { key.uChar.UnicodeChar };
        let composed = key.wVirtualKeyCode == VK_MENU && key.bKeyDown == 0 && unicode != 0;
        if key.bKeyDown == 0 && !composed {
            return None;
        }
        if !(0xdc00..=0xdfff).contains(&unicode) {
            self.surrogate = None;
        }
        let mut modifiers = modifiers(key.dwControlKeyState);
        if composed {
            modifiers.remove(KeyModifiers::ALT | KeyModifiers::CONTROL);
        }
        let code = if (VK_A..=VK_Z).contains(&key.wVirtualKeyCode)
            && unicode < 0x20
            && modifiers.contains(KeyModifiers::CONTROL)
            && !modifiers.contains(KeyModifiers::ALT)
        {
            KeyCode::Char(char::from_u32(u32::from(key.wVirtualKeyCode) + 0x20)?)
        } else {
            match key.wVirtualKeyCode {
                VK_ESCAPE => KeyCode::Esc,
                VK_RETURN => KeyCode::Enter,
                VK_TAB if modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
                VK_TAB => KeyCode::Tab,
                VK_BACK => KeyCode::Backspace,
                VK_LEFT => KeyCode::Left,
                VK_RIGHT => KeyCode::Right,
                VK_UP => KeyCode::Up,
                VK_DOWN => KeyCode::Down,
                VK_HOME => KeyCode::Home,
                VK_END => KeyCode::End,
                VK_PRIOR => KeyCode::PageUp,
                VK_NEXT => KeyCode::PageDown,
                VK_INSERT => KeyCode::Insert,
                VK_DELETE => KeyCode::Delete,
                VK_F1..=VK_F24 => KeyCode::F(u8::try_from(key.wVirtualKeyCode - VK_F1 + 1).ok()?),
                _ => {
                    let character = match unicode {
                        0xd800..=0xdbff => {
                            self.surrogate = Some(unicode);
                            return None;
                        }
                        0xdc00..=0xdfff => char::decode_utf16([self.surrogate.take()?, unicode])
                            .next()?
                            .ok()?,
                        value => {
                            self.surrogate = None;
                            char::from_u32(u32::from(value))?
                        }
                    };
                    match character {
                        '\x1b' => KeyCode::Esc,
                        '\r' | '\n' => KeyCode::Enter,
                        '\t' => KeyCode::Tab,
                        '\x08' => KeyCode::Backspace,
                        '\x01'..='\x1a' => {
                            modifiers |= KeyModifiers::CONTROL;
                            KeyCode::Char(char::from_u32(u32::from(character) + 0x60)?)
                        }
                        '\0' if key.wVirtualKeyCode == VK_SPACE => {
                            modifiers |= KeyModifiers::CONTROL;
                            KeyCode::Char(' ')
                        }
                        '\0' => return None, // modifier/dead keys have no character yet
                        character => {
                            // AltGr's printable Unicode is text, not a Ctrl-Alt binding.
                            if key.dwControlKeyState & RIGHT_ALT_PRESSED != 0
                                && key.dwControlKeyState & LEFT_CTRL_PRESSED != 0
                            {
                                modifiers.remove(KeyModifiers::ALT | KeyModifiers::CONTROL);
                            }
                            KeyCode::Char(character)
                        }
                    }
                }
            }
        };
        Some(Event::Key(KeyEvent::new(code, modifiers)))
    }
    fn mouse(&mut self, mouse: &MOUSE_EVENT_RECORD) -> Option<Event> {
        let buttons = mouse.dwButtonState & 0xffff;
        let button = |mask| match mask {
            1 => Some(MouseButton::Left),
            2 => Some(MouseButton::Right),
            4 => Some(MouseButton::Middle),
            _ => None,
        };
        let kind = match mouse.dwEventFlags {
            MOUSE_WHEELED => {
                if mouse.dwButtonState.cast_signed() < 0 {
                    MouseEventKind::ScrollDown
                } else {
                    MouseEventKind::ScrollUp
                }
            }
            MOUSE_HWHEELED => {
                if mouse.dwButtonState.cast_signed() < 0 {
                    MouseEventKind::ScrollLeft
                } else {
                    MouseEventKind::ScrollRight
                }
            }
            MOUSE_MOVED => button(buttons.isolate_lowest_one())
                .map_or(MouseEventKind::Moved, MouseEventKind::Drag),
            _ => {
                let changed = self.buttons ^ buttons;
                let first = changed.isolate_lowest_one();
                self.buttons = buttons;
                if buttons & first != 0 {
                    MouseEventKind::Down(button(first)?)
                } else {
                    MouseEventKind::Up(button(first)?)
                }
            }
        };
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { zeroed() };
        // SAFETY: this only queries the borrowed output console for its viewport.
        let top = if unsafe {
            GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &raw mut info)
        } != 0
        {
            info.srWindow.Top
        } else {
            0
        };
        Some(Event::Mouse(MouseEvent {
            kind,
            column: mouse.dwMousePosition.X.try_into().ok()?,
            row: mouse
                .dwMousePosition
                .Y
                .saturating_sub(top)
                .try_into()
                .ok()?,
            modifiers: modifiers(mouse.dwControlKeyState),
        }))
    }
}
fn modifiers(state: u32) -> KeyModifiers {
    let mut result = KeyModifiers::NONE;
    if state & SHIFT_PRESSED != 0 {
        result |= KeyModifiers::SHIFT;
    }
    if state & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
        result |= KeyModifiers::CONTROL;
    }
    if state & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 {
        result |= KeyModifiers::ALT;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(virtual_key: u16, unicode: u16, state: u32) -> KEY_EVENT_RECORD {
        let mut key: KEY_EVENT_RECORD = unsafe { zeroed() };
        key.bKeyDown = 1;
        key.wRepeatCount = 1;
        key.wVirtualKeyCode = virtual_key;
        key.uChar.UnicodeChar = unicode;
        key.dwControlKeyState = state;
        key
    }
    #[test]
    fn controls_surrogates_and_altgr_keep_their_meaning() {
        let mut decoder = Decoder::default();
        let Event::Key(escape) = decoder.key(&key(0, 27, 0)).unwrap() else {
            panic!()
        };
        assert_eq!(escape.code, KeyCode::Esc);
        let Event::Key(control) = decoder.key(&key(0, 19, 0)).unwrap() else {
            panic!()
        };
        assert_eq!(control.code, KeyCode::Char('s'));
        assert!(control.modifiers.contains(KeyModifiers::CONTROL));
        assert!(decoder.key(&key(0, 0xd83d, 0)).is_none());
        let Event::Key(emoji) = decoder.key(&key(0, 0xde80, 0)).unwrap() else {
            panic!()
        };
        assert_eq!(emoji.code, KeyCode::Char('🚀'));
        let Event::Key(altgr) = decoder
            .key(&key(0, 0x20ac, RIGHT_ALT_PRESSED | LEFT_CTRL_PRESSED))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(altgr.code, KeyCode::Char('€'));
        assert!(altgr.modifiers.is_empty());
    }

    #[test]
    fn control_h_deletes_one_character_and_alt_codes_are_text() {
        let mut decoder = Decoder::default();
        let Event::Key(control_h) = decoder
            .key(&key(b'H'.into(), 8, LEFT_CTRL_PRESSED))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(control_h.code, KeyCode::Char('h'));
        let mut form = super::super::super::form::Form::default();
        form.paste("two words");
        form.edit(&control_h);
        assert_eq!(form.text(), "two word");
        let Event::Key(backspace) = decoder.key(&key(VK_BACK, 8, LEFT_CTRL_PRESSED)).unwrap()
        else {
            panic!()
        };
        assert_eq!(backspace.code, KeyCode::Backspace);
        let mut composed = key(VK_MENU, 0x20ac, 0);
        composed.bKeyDown = 0;
        let Event::Key(euro) = decoder.key(&composed).unwrap() else {
            panic!()
        };
        assert_eq!(euro.code, KeyCode::Char('€'));
    }

    fn assert_reader_stops(full: bool) {
        use std::{sync::mpsc as sync, time::Duration};
        // SAFETY: the manual-reset event is owned until the worker is joined.
        let stop = unsafe {
            let handle = CreateEventW(ptr::null(), 1, 0, ptr::null());
            assert!(!handle.is_null());
            Arc::new(OwnedHandle::from_raw_handle(handle))
        };
        let (sender, receiver) = mpsc::channel(256);
        let (ready, waiting) = sync::channel();
        let signal = stop.clone();
        let reader = thread::spawn(move || {
            if full {
                for _ in 0..256 {
                    sender.blocking_send(Ok(Event::FocusGained)).unwrap();
                }
                ready.send(()).unwrap();
                assert!(sender.blocking_send(Ok(Event::FocusGained)).is_err());
            } else {
                ready.send(()).unwrap();
                let handles = [signal.as_raw_handle()];
                // SAFETY: the stop event remains live throughout this wait.
                assert_eq!(
                    unsafe { WaitForMultipleObjects(1, handles.as_ptr(), 0, INFINITE) },
                    WAIT_OBJECT_0
                );
            }
        });
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        let events = Events {
            receiver,
            stop,
            reader: Some(reader),
        };
        let (done, finished) = sync::channel();
        let cleanup = thread::spawn(move || {
            drop(events);
            done.send(()).unwrap();
        });
        finished
            .recv_timeout(Duration::from_secs(2))
            .expect("input reader cleanup is bounded");
        cleanup.join().unwrap();
    }

    #[test]
    fn dropping_an_idle_native_reader_joins_it() {
        assert_reader_stops(false);
    }

    #[test]
    fn dropping_a_full_input_queue_unblocks_and_joins_its_reader() {
        assert_reader_stops(true);
    }
}
