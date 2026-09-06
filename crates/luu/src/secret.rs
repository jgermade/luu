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
