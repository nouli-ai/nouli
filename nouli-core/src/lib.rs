//! Shared decision engine for Nouli.
//!
//! The workspace foundation exposes build identity; decision behavior is added
//! through the portable contracts and engine modules.
//!
//! ```
//! assert!(!nouli_core::version().is_empty());
//! ```

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
