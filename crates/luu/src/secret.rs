//! Reading something that is only worth anything while nobody else has it.
//!
//! Two of these now: the bearer token that guards `/ws`, and the API key a
//! hosted endpoint wants. Both are read from a **file**, and the reason is one
//! sentence in three parts — a flag is greppable in `ps`, an environment
//! variable is inherited by every child this process spawns, and a file's
//! exposure is a property the program can *check*. The middle one is not
//! theoretical here: [`crate::provider`] exists to point this agent at a hosted
//! model, `run_command` is where it deliberately runs code it did not write,
//! and nothing in the tree clears the environment between the two.
//!
//! So the check lives in one place and both callers pass through it. The
//! consequence sentence is a parameter because it is the only part that differs:
//! what a leaked token costs is not what a leaked key costs, and a message that
//! generalised over both would tell a person neither.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// The contents, trimmed, from a file whose mode says only its owner can read it.
///
/// `what` names the file in the errors (`the auth token file`), and
/// `consequence` finishes the sentence that says why the mode matters.
pub fn read(path: &Path, what: &str, consequence: &str) -> Result<String> {
    let secret = std::fs::read_to_string(path)
        .with_context(|| format!("reading {what} from {}", path.display()))?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        bail!("{what} {} is empty", path.display());
    }
    check_mode(path, what, consequence)?;
    Ok(secret)
}

/// What a settings page may say about a key without being told what it is.
///
/// **Derived from the file on every read, never stored.** A prefix and a suffix
/// kept in `config.toml` would be part of the key in every backup and every
/// `cat` of it — the thing [`read`]'s whole design avoids, in a smaller
/// quantity — and it would go stale the first time somebody edited the file by
/// hand. Reading it costs what one send already costs.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct Preview {
    /// `sk-…cdef`, or `None` for a key too short to show any of without
    /// showing most of it.
    pub mask: Option<String>,
    /// Why this key will not be used, for the two failures a page can show
    /// before a run hits them: no file, or a mode anyone can read.
    pub problem: Option<String>,
}

/// How much of each end is shown. Three at the front because on these keys it
/// is a public vendor prefix, four at the back because that is the part a
/// person recognises their own key by.
const HEAD: usize = 3;
const TAIL: usize = 4;
/// Below this, the two ends *are* the key.
const SHORTEST: usize = 12;

pub fn preview(path: &Path) -> Preview {
    let Ok(secret) = std::fs::read_to_string(path) else {
        return Preview {
            mask: None,
            problem: Some(format!("no file at {}", path.display())),
        };
    };
    let secret = secret.trim();
    let problem = match check_mode(
        path,
        "this key file",
        "hands the key to anyone who can read it",
    ) {
        Ok(()) => None,
        Err(error) => Some(error.to_string()),
    };
    let characters: Vec<char> = secret.chars().collect();
    Preview {
        mask: match characters.len() >= SHORTEST {
            false => None,
            true => Some(format!(
                "{}…{}",
                characters[..HEAD].iter().collect::<String>(),
                characters[characters.len() - TAIL..]
                    .iter()
                    .collect::<String>(),
            )),
        },
        problem: match secret.is_empty() {
            true => Some(format!("{} is empty", path.display())),
            false => problem,
        },
    }
}

/// A secret into a file only its owner can read, created with that mode rather
/// than chmod'd into it, so there is no moment it is anything else.
///
/// The counterpart to [`read`], and it exists because the two used to disagree
/// about who writes the file: this one checked the mode and nothing in the tree
/// could produce it except a person remembering `umask`. See
/// `RECORD/2026-10-20.a-key-typed-into-the-form.completed.md`.
pub fn write(path: &Path, secret: &str) -> Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    // `mode` above only applies to a file this call created, so a replaced one
    // keeps whatever it had until this narrows it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("setting the mode of {}", path.display()))?;
    }
    file.write_all(format!("{}\n", secret.trim()).as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn check_mode(path: &Path, what: &str, consequence: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path)
        .with_context(|| format!("the mode of {}", path.display()))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        bail!(
            "{what} {} is mode {mode:04o}: readable beyond its owner, which {consequence}. \
             `chmod 600` it.",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_mode(_path: &Path, _what: &str, _consequence: &str) -> Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn written(name: &str, contents: &str, mode: u32) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("luu-secret-{name}-{}", std::process::id()));
        std::fs::write(&path, contents).expect("writing the secret");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("setting the mode");
        path
    }

    #[test]
    fn reads_a_private_file_and_trims_it() {
        let path = written("ok", "s3cret\n", 0o600);
        let secret = read(&path, "the API key file", "hands the key to anyone here").unwrap();
        assert_eq!(secret, "s3cret");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn refuses_one_the_group_can_read() {
        let path = written("loose", "s3cret\n", 0o640);
        let error = read(&path, "the API key file", "hands the key to anyone here")
            .expect_err("0640 is readable beyond its owner");
        assert!(error.to_string().contains("readable beyond its owner"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn refuses_an_empty_one() {
        let path = written("empty", "\n", 0o600);
        let error = read(&path, "the API key file", "hands the key to anyone here")
            .expect_err("an empty secret is not a secret");
        assert!(error.to_string().contains("is empty"));
        std::fs::remove_file(&path).ok();
    }
}
