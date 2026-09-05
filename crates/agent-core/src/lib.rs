//! The brains. Knows nothing about the CLI, VSCode or the browser.
//!
//! See `AGENTS.md` for the design commitments this crate is expected to hold,
//! and `RECORD/` for how they were arrived at.

pub mod agent;
pub mod api;
pub mod approval;
pub mod backend;
pub mod context;
pub mod fragment;
pub mod job;
pub mod protocol;
pub mod rank;
pub mod record;
pub mod repo_map;
pub mod sandbox;
pub mod select;
pub use job as task; // Temporary alias for smooth migration
pub mod tools;
pub mod trace;
pub mod turn;
pub mod worker;
