//! Opening a file the kernel will not let out of its tree.
//!
//! Every in-process file tool used to be two steps — canonicalize and compare,
//! then open by path — and between them the path is a *string*. Anything that
//! can put a symlink in that window gets the tool to open what the sandbox
//! refused, while the verdict, written before the open, says `allowed` about a
//! file nobody checked. Who can do that is not hypothetical: `write_file` is
//! granted inside the sandbox by design, so the model can make the symlink
//! itself.
//!
//! [`openat2(2)`](https://man7.org/linux/man-pages/man2/openat2.2.html) closes
//! it by moving the decision into the walk. `RESOLVE_BENEATH` refuses any step
//! that would leave the directory it was given — absolute paths, `..` above it,
//! and symlinks pointing out — and refuses it *during* resolution, so there is
//! no window between deciding and opening. `RESOLVE_NO_MAGICLINKS` goes with
//! it, because `/proc/self/fd/N` is a door out of any tree that does not say so.
//!
//! Linux 5.6 and up. Where the syscall is missing the open falls back to the
//! path exactly as before, and **the verdict says so** — see [`Opened::applied`]
//! and `luu-design.md` §Who enforced it is reported, never assumed.
//!
//! See `RECORD/2026-09-05.beneath-the-root.completed.md`.

use std::fs::File;
use std::path::Path;

use super::Applied;

/// What the caller wants to do with the file, which is the half of the open
/// flags that is not about resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Read,
    /// Create if absent, truncate if present — `write_file`'s contract, and
    /// `edit_file`'s once it has the new text.
    Write,
}

/// A file, and who kept it inside the tree.
#[derive(Debug)]
pub struct Opened {
    pub file: File,
    /// [`Applied::Kernel`] when `openat2` resolved it, [`Applied::Process`] when
    /// the fallback did. The difference is the whole reason this is reported
    /// rather than assumed: two machines running one policy get two guarantees,
    /// and the recording has to say which one this was.
    pub applied: Applied,
}

/// How `openat2` is named in a verdict, mask and all, because a reader months
/// later needs to know *which* resolution rules were asked for and not merely
/// that a modern syscall was involved.
pub const HOW: &str = "openat2(RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS)";

#[cfg(target_os = "linux")]
pub fn open(root: &Path, relative: &Path, mode: Mode) -> std::io::Result<Opened> {
    use std::os::fd::{AsRawFd, FromRawFd};

    // `O_PATH`: the root is a handle to resolve against, never something this
    // process reads. It is opened per call rather than cached on the sandbox —
    // a `Sandbox` that owned a descriptor would change what cloning one and
    // narrowing one mean, for an open of a directory that is already in the
    // dentry cache.
    let directory = std::fs::File::open(root)?;

    let path = std::ffi::CString::new(relative.as_os_str().as_encoded_bytes())
        .map_err(|_| std::io::Error::other("a path with a NUL in it is not a path"))?;

    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = match mode {
        Mode::Read => (libc::O_RDONLY | libc::O_CLOEXEC) as u64,
        Mode::Write => (libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC) as u64,
    };
    if mode == Mode::Write {
        // Only consulted when O_CREAT makes a file, and then the umask applies
        // on top as it does everywhere else.
        how.mode = 0o666;
    }
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_MAGICLINKS;

    // SAFETY: `openat2` reads `how` for the size given and writes nothing of
    // ours; `path` is a valid NUL-terminated string that outlives the call, and
    // `directory` outlives it too. It returns a descriptor or -1, never a trap.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory.as_raw_fd(),
            path.as_ptr(),
            std::ptr::addr_of!(how),
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if fd >= 0 {
        // SAFETY: a descriptor the kernel just gave us and nothing else owns.
        let file = unsafe { File::from_raw_fd(fd as std::os::fd::RawFd) };
        return Ok(Opened {
            file,
            applied: Applied::Kernel {
                how: HOW.to_string(),
            },
        });
    }

    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        // The syscall is not here (pre-5.6, or a seccomp profile that does not
        // pass it through), or this filesystem will not take the mask. Not a
        // denial: a denial is EXDEV or ELOOP below, and those are the answer.
        Some(libc::ENOSYS) | Some(libc::EPERM) | Some(libc::EOPNOTSUPP) => {
            fallback(root.join(relative).as_path(), mode)
        }
        _ => Err(error),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn open(root: &Path, relative: &Path, mode: Mode) -> std::io::Result<Opened> {
    // No `openat2`, and the honest thing is the same one `sandbox::fallback`
    // does for level two: say what is missing rather than quietly returning
    // something that reads as held.
    fallback(root.join(relative).as_path(), mode)
}

/// The old two-step, kept for where the kernel cannot do better — and labelled
/// [`Applied::Process`], which is what it is.
fn fallback(path: &Path, mode: Mode) -> std::io::Result<Opened> {
    let file = match mode {
        Mode::Read => File::open(path)?,
        Mode::Write => File::create(path)?,
    };
    Ok(Opened {
        file,
        applied: Applied::Process,
    })
}

/// Whether a failure to open was the resolution rules refusing, rather than the
/// file merely not being there.
///
/// `EXDEV` is what `RESOLVE_BENEATH` answers when a step would leave the tree,
/// and `ELOOP` is what it answers for a symlink it will not follow. They are
/// **denials**, and a tool that reported them as "no such file" would send a
/// model looking for a typo instead of telling it the sandbox said no.
pub fn is_escape(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EXDEV) | Some(libc::ELOOP))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    struct Tree(std::path::PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "luu-beneath-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id(),
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("inside")).unwrap();
            std::fs::write(root.join("inside/kept.txt"), "kept\n").unwrap();
            std::fs::write(root.join("outside.txt"), "not yours\n").unwrap();
            Self(root.canonicalize().unwrap_or(root))
        }

        fn inside(&self) -> std::path::PathBuf {
            self.0.join("inside")
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn read(opened: Opened) -> String {
        use std::io::Read;
        let mut text = String::new();
        let mut file = opened.file;
        file.read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn an_ordinary_file_opens_and_the_kernel_is_what_held_it() {
        let tree = Tree::new("plain");
        let opened = open(&tree.inside(), std::path::Path::new("kept.txt"), Mode::Read)
            .expect("a file inside the root opens");
        assert_eq!(
            opened.applied,
            Applied::Kernel {
                how: HOW.to_string()
            },
            "on a kernel with openat2 this is no longer an in-process check",
        );
        assert_eq!(read(opened), "kept\n");
    }

    /// The window. The symlink is created **after** any check would have run
    /// and before this open — which is the whole of the race, staged rather
    /// than raced because a test that loses a race on purpose is a test that
    /// passes on a quiet machine and fails on a busy one.
    #[test]
    fn a_symlink_that_leaves_the_root_is_refused_during_resolution() {
        let tree = Tree::new("escape");
        std::os::unix::fs::symlink(tree.0.join("outside.txt"), tree.inside().join("way-out"))
            .unwrap();

        let error = open(&tree.inside(), std::path::Path::new("way-out"), Mode::Read)
            .expect_err("RESOLVE_BENEATH refuses a link that leaves the root");
        assert!(
            is_escape(&error),
            "and it is a denial rather than a missing file: {error:?}",
        );
    }

    #[test]
    fn a_symlink_that_stays_inside_is_followed_because_it_is_ordinary() {
        // `RESOLVE_NO_SYMLINKS` would have refused this, and every checkout
        // that has one. The rule is about where a path lands.
        let tree = Tree::new("inside-link");
        std::os::unix::fs::symlink("kept.txt", tree.inside().join("alias.txt")).unwrap();
        let opened = open(
            &tree.inside(),
            std::path::Path::new("alias.txt"),
            Mode::Read,
        )
        .expect("a link that stays inside is not an escape");
        assert_eq!(read(opened), "kept\n");
    }

    #[test]
    fn dotdot_and_an_absolute_path_are_both_refused() {
        let tree = Tree::new("updots");
        for escape in ["../outside.txt", "/etc/hostname"] {
            let error = open(&tree.inside(), std::path::Path::new(escape), Mode::Read)
                .expect_err("{escape} left the root");
            assert!(is_escape(&error), "{escape}: {error:?}");
        }
    }

    #[test]
    fn a_write_creates_beneath_the_root_and_nowhere_else() {
        let tree = Tree::new("write");
        let opened = open(
            &tree.inside(),
            std::path::Path::new("made.txt"),
            Mode::Write,
        )
        .expect("a new file inside the root");
        use std::io::Write;
        let mut file = opened.file;
        file.write_all(b"written\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(tree.inside().join("made.txt")).unwrap(),
            "written\n",
        );

        std::os::unix::fs::symlink(tree.0.join("outside.txt"), tree.inside().join("out-link"))
            .unwrap();
        let error = open(
            &tree.inside(),
            std::path::Path::new("out-link"),
            Mode::Write,
        )
        .expect_err("a write through a link that leaves the root");
        assert!(is_escape(&error), "{error:?}");
        assert_eq!(
            std::fs::read_to_string(tree.0.join("outside.txt")).unwrap(),
            "not yours\n",
            "and the file outside was not touched",
        );
    }
}
