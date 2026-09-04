//! Where `luu` keeps *this machine's* state, and who decides that.
//!
//! The session store lives here, and by convention the approval key. What it is
//! deliberately not is anywhere near `luu.toml`: the policy file describes *this
//! project* and is committed with it, while everything under this directory is
//! one machine's history and belongs to nobody else's clone.
//!
//! There is no answer to "where" that is right on every machine. `~/.luu` is
//! what a dotfile-shaped tool does; `~/.config/luu` is what the XDG basedir
//! spec asks for; which one a person wants is a fact about their home directory
//! and not about us. So the first run **asks**, and every run after it finds the
//! directory that answer created:
//!
//! 1. `LUU_HOME`, if set — an explicit answer ends the question.
//! 2. `~/.luu`, if it exists.
//! 3. `$XDG_CONFIG_HOME/luu` (or `~/.config/luu`), if it exists.
//! 4. Otherwise ask, once, and create what was chosen.
//!
//! **The directory's own existence is the record of the choice**, which is why
//! there is no setting for it: a setting that says where the settings live has
//! to live somewhere, and that somewhere is the question. `~/.luu` is checked
//! before the XDG path because it is the deliberate one — nothing else puts a
//! directory there, while `~/.config` is full of directories other programs
//! made.
//!
//! Argued in `RECORD/2026-09-04.the-name-and-the-config-dir.completed.md`.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

/// The environment the resolution reads, named so the rules can be tested
/// without a real `$HOME` and without a terminal.
#[derive(Debug, Default, Clone)]
pub struct Env {
    /// `LUU_HOME`.
    pub explicit: Option<PathBuf>,
    /// `$HOME`.
    pub home: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<PathBuf>,
}

impl Env {
    /// What this process was started with. Empty variables count as unset: an
    /// exported-but-empty `HOME` is the shape a stripped environment has, and
    /// joining onto it would name a path at the filesystem root.
    pub fn from_process() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            explicit: var("LUU_HOME"),
            home: var("HOME"),
            xdg_config_home: var("XDG_CONFIG_HOME"),
        }
    }

    /// `~/.luu`.
    fn dotted(&self) -> Option<PathBuf> {
        self.home.as_ref().map(|home| home.join(".luu"))
    }

    /// `$XDG_CONFIG_HOME/luu`, falling back to the spec's own default.
    fn xdg(&self) -> Option<PathBuf> {
        match &self.xdg_config_home {
            Some(base) => Some(base.join("luu")),
            None => self
                .home
                .as_ref()
                .map(|home| home.join(".config").join("luu")),
        }
    }
}

/// What the rules can conclude before anyone is asked anything.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Settled: either named outright, or found on disk.
    Decided(PathBuf),
    /// Nothing has been chosen on this machine yet. Both candidates are carried
    /// so the asking half does not recompute them from a second reading of the
    /// environment.
    Undecided { dotted: PathBuf, xdg: PathBuf },
    /// No `$HOME` and no `LUU_HOME`, so there is no directory to have an
    /// opinion about. Said rather than guessed at: a run that picked a
    /// directory of its own would write history into it.
    Homeless,
}

/// The rules, over an environment and a way to ask whether a path exists.
///
/// Split from [`dir`] so both halves are testable: this one is pure, and the
/// one that prompts is the only part that needs a terminal.
pub fn resolve(env: &Env, exists: impl Fn(&Path) -> bool) -> Resolution {
    if let Some(explicit) = &env.explicit {
        return Resolution::Decided(explicit.clone());
    }
    let (Some(dotted), Some(xdg)) = (env.dotted(), env.xdg()) else {
        return Resolution::Homeless;
    };
    if exists(&dotted) {
        Resolution::Decided(dotted)
    } else if exists(&xdg) {
        Resolution::Decided(xdg)
    } else {
        Resolution::Undecided { dotted, xdg }
    }
}

/// The state directory for this machine, asking on the first run.
///
/// `None` is the homeless case, and every caller treats it the way `--no-store`
/// is treated: carry on without one, having said so.
pub fn dir() -> Option<PathBuf> {
    let env = Env::from_process();
    let chosen = match resolve(&env, |path| path.exists()) {
        Resolution::Decided(path) => path,
        Resolution::Homeless => return None,
        Resolution::Undecided { dotted, xdg } => ask(&dotted, &xdg),
    };
    // Created here rather than by the first writer, because the directory is
    // the record of the choice: a run that chose and then wrote nothing would
    // ask again next time.
    if let Err(error) = std::fs::create_dir_all(&chosen) {
        eprintln!("warning: could not create {}: {error}", chosen.display());
        return None;
    }
    Some(chosen)
}

/// The first-run question.
///
/// **Only on a terminal.** A prompt nobody can answer is a hang, and a hang in
/// `luu stdio` is a parent process waiting forever on a pipe — so a
/// non-interactive run takes the XDG path, says so, and names the flag that
/// would have said otherwise. The machines that cannot answer are the scripted
/// ones, and that is the convention a script expects.
fn ask(dotted: &Path, xdg: &Path) -> PathBuf {
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "luu: no state directory yet, so this run made one at {} \
             (set LUU_HOME to keep it elsewhere).",
            xdg.display()
        );
        return xdg.to_path_buf();
    }

    eprintln!("luu has no state directory on this machine yet. Where should it keep");
    eprintln!("sessions and keys?");
    eprintln!();
    eprintln!("  1) {}", dotted.display());
    eprintln!("  2) {}   (XDG)", xdg.display());
    eprintln!();

    // Three tries, then the default. An answer that keeps not arriving is a
    // pipe that closed or a person who wants the default, and neither is
    // improved by asking a fourth time.
    for _ in 0..3 {
        eprint!("Choose [1/2] (default 2): ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).unwrap_or(0) == 0 {
            break;
        }
        match answer.trim() {
            "1" => return dotted.to_path_buf(),
            "" | "2" => return xdg.to_path_buf(),
            other => eprintln!("'{other}' is not 1 or 2."),
        }
    }
    xdg.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Env {
        Env {
            explicit: None,
            home: Some(PathBuf::from("/home/p")),
            xdg_config_home: None,
        }
    }

    #[test]
    fn an_explicit_luu_home_ends_the_question() {
        let env = Env {
            explicit: Some(PathBuf::from("/elsewhere")),
            ..env()
        };
        assert_eq!(
            resolve(&env, |_| true),
            Resolution::Decided(PathBuf::from("/elsewhere")),
            "named outright, so nothing on disk gets a vote",
        );
    }

    #[test]
    fn a_directory_that_exists_is_the_choice_already_made() {
        let dotted = PathBuf::from("/home/p/.luu");
        assert_eq!(
            resolve(&env(), |path| *path == dotted),
            Resolution::Decided(dotted),
        );

        let xdg = PathBuf::from("/home/p/.config/luu");
        assert_eq!(
            resolve(&env(), |path| *path == xdg),
            Resolution::Decided(xdg),
        );
    }

    #[test]
    fn the_deliberate_one_wins_when_both_are_there() {
        assert_eq!(
            resolve(&env(), |_| true),
            Resolution::Decided(PathBuf::from("/home/p/.luu")),
            "nothing but us puts a directory at ~/.luu; ~/.config is full of them",
        );
    }

    #[test]
    fn xdg_config_home_is_obeyed_when_it_is_set() {
        let env = Env {
            xdg_config_home: Some(PathBuf::from("/somewhere/config")),
            ..env()
        };
        assert_eq!(
            resolve(&env, |_| false),
            Resolution::Undecided {
                dotted: PathBuf::from("/home/p/.luu"),
                xdg: PathBuf::from("/somewhere/config/luu"),
            },
        );
    }

    #[test]
    fn nothing_on_disk_is_a_question_rather_than_a_default() {
        assert_eq!(
            resolve(&env(), |_| false),
            Resolution::Undecided {
                dotted: PathBuf::from("/home/p/.luu"),
                xdg: PathBuf::from("/home/p/.config/luu"),
            },
        );
    }

    #[test]
    fn no_home_is_said_rather_than_guessed_at() {
        assert_eq!(resolve(&Env::default(), |_| true), Resolution::Homeless);
    }
}
