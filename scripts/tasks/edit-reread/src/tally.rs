//! How many turns have gone by, and when that is worth saying out loud.

/// The number of turns after which the tally is worth reporting.
pub const THRESHOLD: usize = 16;

/// A count of turns, and nothing else.
///
/// The overlap case: the corpus grounds the head of this file before editing
/// [`THRESHOLD`] and a longer range of it after, so both ranges carry line 4
/// and disagree about it while the detector counts two paths. See
/// `../README.md`.
#[derive(Debug, Default)]
pub struct Tally {
    seen: usize,
}

impl Tally {
    /// Counts one more turn.
    pub fn saw(&mut self) {
        self.seen += 1;
    }

    /// Whether the count has reached [`THRESHOLD`].
    pub fn worth_saying(&self) -> bool {
        self.seen >= THRESHOLD
    }
}
