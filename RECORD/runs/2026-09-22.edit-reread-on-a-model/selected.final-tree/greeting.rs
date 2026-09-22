//! What the program says to whoever is in front of it.

/// Greets `name`.
///
/// The corpus edits this function twice and asks about it after each edit, so
/// a window that has seen all three versions carries three bodies for one
/// path. See `../README.md`.
pub fn greet(name: &str) -> String {
    format!("Hey, {name}")
}
