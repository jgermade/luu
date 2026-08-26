//! The brains. Knows nothing about the CLI, VSCode or the browser.
//!
//! See `AGENTS.md` for the design commitments this crate is expected to hold,
//! and `RECORD/` for how they were arrived at.

pub mod backend;
pub mod protocol;
pub mod record;
pub mod trace;
pub mod turn;
