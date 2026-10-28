//! Shared decision engine for Nouli.
//!
//! The workspace foundation exposes build identity; decision behavior is added
//! through the portable contracts and engine modules.
//!
//! ```
//! assert!(!nouli_core::version().is_empty());
//! ```

pub mod codec;
pub mod config;
pub mod context;
pub mod data;
pub mod decision;
pub mod history;
pub mod identity;
pub mod limits;
pub mod outcome;
pub mod protocol;
pub mod provider;
pub mod storage;
pub mod transport;
pub mod workflow;

/// The version of the Nouli library in this build.
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_matches_package_metadata() {
        assert_eq!(super::version(), env!("CARGO_PKG_VERSION"));
    }
}
