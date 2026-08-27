//! Everywhere that is not Linux.
//!
//! There is no level two here, and the honest thing is to say so rather than to
//! quietly return a sandbox that holds nothing. What happens next is
//! [`Enforcement`](super::Enforcement)'s decision: the default refuses to run a
//! subprocess at all, and `best-effort` runs it with this gap in the verdict.
//!
//! In-process tools are unaffected — they never had kernel enforcement to lose.

use super::Root;

pub(super) struct Prepared {
    pub(super) how: Option<String>,
    pub(super) missing: Option<String>,
}

pub(super) fn prepare(_roots: &[Root], _network: bool) -> Prepared {
    Prepared {
        how: None,
        missing: Some(format!(
            "landlock and seccomp are Linux-only, and this is {}",
            std::env::consts::OS
        )),
    }
}

impl Prepared {
    pub(super) fn install(self, _command: &mut std::process::Command) {}
}
