//! The brains. Knows nothing about the CLI, VSCode or the browser.
//!
//! See `AGENTS.md` for the design commitments this crate is expected to hold,
//! and `RECORD/` for how they were arrived at.

pub mod agent;
pub mod api;
pub mod backend;
pub mod context;
pub mod fragment;
pub mod protocol;
pub mod record;
pub mod repo_map;
pub mod sandbox;
pub mod task;
pub mod tools;
pub mod trace;
pub mod turn;
pub mod worker;
