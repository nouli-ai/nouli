//! Native platform terminal backends with the same bounded test interface.
#[path = "child.rs"]
#[allow(dead_code, reason = "terminal tests use a subset of fixture helpers")]
pub mod fixture;
#[cfg(unix)]
#[path = "pty/unix.rs"]
mod unix;
#[cfg(unix)]
pub use unix::Terminal;
#[cfg(windows)]
#[path = "pty/windows.rs"]
mod windows;
#[cfg(windows)]
pub use windows::Terminal;
