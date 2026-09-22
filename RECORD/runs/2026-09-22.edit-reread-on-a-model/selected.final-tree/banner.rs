//! The line printed above everything else.
//!
//! The blind spot: the corpus reads this file once, edits it later and never
//! asks about it again, so the window keeps sending bytes that are no longer
//! on disk and nothing contradicts them.

/// The banner, printed once at the top of a session.
pub fn banner() -> String {
    "luu 0.2".to_string()
}
