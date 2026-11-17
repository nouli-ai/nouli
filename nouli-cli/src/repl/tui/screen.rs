//! Terminal lifetime and mouse reporting.
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
};
use ratatui::DefaultTerminal;
use std::io;
/// The terminal in raw mode on the alternate screen, with bracketed paste,
/// restored when left or dropped.
pub(super) struct Screen {
    pub(super) terminal: DefaultTerminal,
    entered: bool,
    #[cfg(windows)]
    _modes: ConsoleModes,
}

impl Screen {
    pub(super) fn enter() -> io::Result<Self> {
        #[cfg(windows)]
        let modes = ConsoleModes::capture()?;
        let terminal = match ratatui::try_init() {
            Ok(terminal) => terminal,
            Err(error) => {
                // Raw mode may already be on.
                if let Err(restore) = ratatui::try_restore() {
                    tracing::warn!(error = %restore, "cannot restore the terminal");
                }
                return Err(error);
            }
        };
        let screen = Self {
            terminal,
            entered: true,
            #[cfg(windows)]
            _modes: modes,
        };
        virtual_input(true)?;
        execute!(io::stdout(), EnableBracketedPaste)?;
        capture_mouse(false)?;
        Ok(screen)
    }

    pub(super) fn leave(&mut self) -> io::Result<()> {
        if !std::mem::replace(&mut self.entered, false) {
            return Ok(());
        }
        let paste = execute!(io::stdout(), DisableBracketedPaste);
        let mouse = capture_mouse(false);
        let input = virtual_input(false);
        ratatui::try_restore().and(paste).and(mouse).and(input)
    }
}

#[cfg(windows)]
struct ConsoleModes {
    input: windows_sys::Win32::Foundation::HANDLE,
    output: windows_sys::Win32::Foundation::HANDLE,
    input_mode: u32,
    output_mode: u32,
}
#[cfg(windows)]
impl ConsoleModes {
    fn capture() -> io::Result<Self> {
        use windows_sys::Win32::System::Console::{
            ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
            STD_OUTPUT_HANDLE, SetConsoleMode,
        };
        // SAFETY: standard handles are borrowed; saved modes are restored on Drop.
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            let output = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut input_mode = 0;
            let mut output_mode = 0;
            if GetConsoleMode(input, &raw mut input_mode) == 0
                || GetConsoleMode(output, &raw mut output_mode) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let modes = Self {
                input,
                output,
                input_mode,
                output_mode,
            };
            if SetConsoleMode(output, output_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(modes)
        }
    }
}
#[cfg(windows)]
impl Drop for ConsoleModes {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::SetConsoleMode;
        // SAFETY: standard handles stay owned by the process through terminal cleanup.
        unsafe {
            let input = SetConsoleMode(self.input, self.input_mode);
            let output = SetConsoleMode(self.output, self.output_mode);
            if input == 0 || output == 0 {
                tracing::warn!("cannot restore Windows console modes");
            }
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if let Err(error) = self.leave() {
            tracing::warn!(%error, "cannot restore the terminal");
        }
    }
}

pub(super) fn capture_mouse(capture: bool) -> io::Result<()> {
    #[cfg(unix)]
    use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
    #[cfg(unix)]
    {
        if capture {
            execute!(io::stdout(), EnableMouseCapture)?;
        } else {
            execute!(io::stdout(), DisableMouseCapture)?;
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{
            ENABLE_EXTENDED_FLAGS, ENABLE_MOUSE_INPUT, ENABLE_QUICK_EDIT_MODE, ENABLE_WINDOW_INPUT,
            GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
        };
        // Preserve raw-mode bits; crossterm's Windows mouse commands replace
        // the whole mode and cannot disable capture before their first enable.
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0;
            if GetConsoleMode(input, &raw mut mode) == 0 {
                return Err(io::Error::last_os_error());
            }
            mode |= ENABLE_EXTENDED_FLAGS | ENABLE_WINDOW_INPUT;
            mode = if capture {
                (mode | ENABLE_MOUSE_INPUT) & !ENABLE_QUICK_EDIT_MODE
            } else {
                (mode & !ENABLE_MOUSE_INPUT) | ENABLE_QUICK_EDIT_MODE
            };
            if SetConsoleMode(input, mode) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}

/// Conhost strips bracketed-paste markers unless VT input is enabled, even
/// when events are read through `ReadConsoleInputW` rather than `ReadFile`.
#[cfg_attr(
    unix,
    allow(
        clippy::unnecessary_wraps,
        reason = "Windows console mode changes are fallible"
    )
)]
pub(super) fn virtual_input(enabled: bool) -> io::Result<()> {
    #[cfg(unix)]
    let _ = enabled;
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{
            ENABLE_VIRTUAL_TERMINAL_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
            SetConsoleMode,
        };
        // SAFETY: the standard input handle is borrowed and the mode is preserved.
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE);
            let mut mode = 0;
            if GetConsoleMode(input, &raw mut mode) == 0 {
                return Err(io::Error::last_os_error());
            }
            mode = if enabled {
                mode | ENABLE_VIRTUAL_TERMINAL_INPUT
            } else {
                mode & !ENABLE_VIRTUAL_TERMINAL_INPUT
            };
            if SetConsoleMode(input, mode) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(())
}
