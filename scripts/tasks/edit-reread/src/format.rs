//! Padding a name out to a fixed width.
//!
//! The control: the corpus reads this file twice and never edits it, so rule A
//! collapses the two renders into one and nothing is reported.

/// The width every rendered name is padded to.
pub const WIDTH: usize = 12;

/// Pads `name` to [`WIDTH`], leaving it alone when it is already that long.
/// An empty name renders as [`WIDTH`] spaces.
pub fn render(name: &str) -> String {
    format!("{name:<WIDTH$}")
}
