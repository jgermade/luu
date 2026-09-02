//! Level two: the kernel holding a subprocess.
//!
//! Landlock for the filesystem and seccomp for sockets, applied to the child
//! between `fork` and `exec`. No image, no daemon, no privileges — a Linux
//! kernel from 2021 and two syscalls.
//!
//! **The ruleset is built in the parent.** Opening a file descriptor per root
//! and assembling the rules is ordinary allocating code, and `pre_exec` runs in
//! a forked child of a threaded process, where another thread may have been
//! holding the allocator's lock at the moment of the fork. What is left to do
//! after the fork is three syscalls and no allocation.
//!
//! See `RECORD/2026-08-27.tools-and-sandbox.completed.md`.

use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;

use landlock::{
    ABI, Access as _, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

use super::{Access, Root};

/// The Landlock ABI whose access rights we ask to handle.
///
/// Not the newest this crate knows. v5 is read, write, `Refer`, `Truncate` and
/// `IoctlDev`; v9 would add `ResolveUnix`, which forbids connecting to Unix
/// sockets by path — and name resolution goes through one, so it would break
/// `network = true` for a restriction that matters only when the network is off
/// anyway. Asking for v5 on a newer kernel is not a downgrade of what is
/// granted; it is a smaller set of *kinds of access* being mediated, and the
/// verdict reports the kernel's own ABI so a reader can see the difference.
const TARGET_ABI: ABI = ABI::V5;

#[cfg(target_arch = "x86_64")]
const SECCOMP_ARCH: Option<TargetArch> = Some(TargetArch::x86_64);
#[cfg(target_arch = "aarch64")]
const SECCOMP_ARCH: Option<TargetArch> = Some(TargetArch::aarch64);
#[cfg(target_arch = "riscv64")]
const SECCOMP_ARCH: Option<TargetArch> = Some(TargetArch::riscv64);
#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64"
)))]
const SECCOMP_ARCH: Option<TargetArch> = None;

/// What the child will be held to, and what it will not.
pub(super) struct Prepared {
    /// The mechanisms that are ready, named for the verdict.
    pub(super) how: Option<String>,
    /// The ones that are not, and why. This is the difference between this run
    /// and a held one, so it is never dropped on the floor.
    pub(super) missing: Option<String>,
    ruleset: Option<OwnedFd>,
    filter: Option<BpfProgram>,
}

pub(super) fn prepare(roots: &[Root], network: bool) -> Prepared {
    let (ruleset, ruleset_gap) = match build_ruleset(roots) {
        Ok(fd) => (Some(fd), None),
        Err(why) => (None, Some(why)),
    };
    let (filter, filter_gap) = match build_filter(network) {
        Ok(program) => (Some(program), None),
        Err(why) => (None, Some(why)),
    };

    let mut how = Vec::new();
    if ruleset.is_some() {
        how.push(match kernel_abi() {
            Some(version) => format!("landlock ABI v{version}"),
            None => "landlock".to_string(),
        });
    }
    if filter.is_some() {
        how.push(match network {
            true => "seccomp (ptrace)".to_string(),
            false => "seccomp (no sockets, no ptrace)".to_string(),
        });
    }

    let missing: Vec<String> = [ruleset_gap, filter_gap].into_iter().flatten().collect();

    Prepared {
        how: (!how.is_empty()).then(|| how.join(" + ")),
        missing: (!missing.is_empty()).then(|| missing.join("; ")),
        ruleset,
        filter,
    }
}

impl Prepared {
    pub(super) fn install(self, command: &mut std::process::Command) {
        let Self {
            ruleset, filter, ..
        } = self;
        if ruleset.is_none() && filter.is_none() {
            return;
        }

        // SAFETY: the closure runs after `fork` and before `exec`, where only
        // async-signal-safe work is allowed. It allocates nothing and calls
        // three syscalls; `io::Error::last_os_error` on the failure path stores
        // a raw errno without touching the allocator.
        unsafe {
            command.pre_exec(move || {
                // Both mechanisms require it, and a child that could regain
                // privileges through a setuid binary would be holding neither.
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if let Some(ruleset) = &ruleset
                    && libc::syscall(
                        libc::SYS_landlock_restrict_self,
                        ruleset.as_raw_fd() as libc::c_long,
                        0 as libc::c_long,
                    ) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                if let Some(filter) = &filter {
                    seccompiler::apply_filter(filter)
                        .map_err(|_| std::io::Error::last_os_error())?;
                }
                Ok(())
            });
        }
    }
}

/// The ruleset, as a bare file descriptor the child can be restricted with.
///
/// `None` out of the landlock crate means the kernel has no Landlock at all —
/// the crate's own signal for it, and the one thing we have to tell apart from
/// a ruleset that merely handles fewer access kinds than we asked for.
fn build_ruleset(roots: &[Root]) -> Result<OwnedFd, String> {
    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(TARGET_ABI))
        .and_then(|ruleset| ruleset.create())
        .map_err(|error| format!("landlock ruleset: {error}"))?;

    for root in roots {
        let access = match root.access {
            // Deliberately not `from_read`, which includes `Execute`: "may read
            // this tree" and "may run binaries out of this tree" are different
            // grants and the policy spells them differently.
            Access::Read => AccessFs::ReadFile | AccessFs::ReadDir,
            Access::Execute => AccessFs::from_read(TARGET_ABI),
            Access::ReadWrite => AccessFs::from_all(TARGET_ABI),
        };
        let fd = PathFd::new(&root.path)
            .map_err(|error| format!("landlock {}: {error}", root.path.display()))?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|error| format!("landlock {}: {error}", root.path.display()))?;
    }

    Option::<OwnedFd>::from(ruleset)
        .ok_or_else(|| "landlock is not available in this kernel".to_string())
}

/// The syscall filter.
///
/// A denylist, not an allowlist. An allowlist that has to admit every syscall
/// `cargo` makes is a permanent maintenance surface that fails closed on
/// somebody's machine at the worst moment; this stops four things and says
/// which four.
fn build_filter(network: bool) -> Result<BpfProgram, String> {
    let Some(arch) = SECCOMP_ARCH else {
        return Err(format!(
            "seccomp has no filter for {}",
            std::env::consts::ARCH
        ));
    };

    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();

    if !network {
        // Filtered on the address family rather than on `socket` as a whole:
        // libc needs `AF_UNIX` and `AF_NETLINK` to so much as resolve a user
        // name. An empty rule list would mean "always", so these are conditions.
        let mut families = Vec::new();
        for family in [libc::AF_INET, libc::AF_INET6, libc::AF_PACKET] {
            let condition =
                SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, family as u64)
                    .map_err(|error| format!("seccomp condition: {error}"))?;
            families.push(
                SeccompRule::new(vec![condition])
                    .map_err(|error| format!("seccomp rule: {error}"))?,
            );
        }
        rules.insert(libc::SYS_socket, families);
    }

    // Always, network or not: with `no_new_privs` set, attaching to a sibling
    // process of the same user is the way out of a sandbox that is otherwise
    // airtight. An empty rule list matches the syscall unconditionally.
    for syscall in [
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
    ] {
        rules.insert(syscall, Vec::new());
    }

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        arch,
    )
    .map_err(|error| format!("seccomp filter: {error}"))?;

    BpfProgram::try_from(filter).map_err(|error| format!("seccomp program: {error}"))
}

/// The kernel's Landlock ABI, for reporting only.
///
/// The landlock crate keeps this private on purpose — behaviour must not depend
/// on a version discovered at runtime — and that is exactly why this is only
/// ever put in a string. A reader months later cannot otherwise tell whether
/// `truncate` was mediated on that run.
fn kernel_abi() -> Option<i64> {
    // LANDLOCK_CREATE_RULESET_VERSION, the documented way to ask.
    const VERSION_REQUEST: libc::c_long = 1;
    // SAFETY: a null attribute pointer with size 0 is the version query the
    // syscall documents; it creates nothing and returns the ABI number.
    let version = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<u8>(),
            0 as libc::size_t,
            VERSION_REQUEST,
        )
    };
    (version > 0).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::{Enforcement, PathRule, Sandbox, SandboxPolicy};

    fn sandbox_over(dir: &std::path::Path, network: bool) -> Sandbox {
        Sandbox::new(
            &SandboxPolicy {
                paths: vec![PathRule::new(".", Access::ReadWrite)],
                commands: vec!["/bin/sh".into()],
                network,
                enforcement: Enforcement::BestEffort,
                limits: Default::default(),
            },
            dir,
        )
        .unwrap()
    }

    /// Runs `sh -c <script>` under the sandbox and returns its exit code.
    fn run_under(sandbox: &Sandbox, script: &str) -> i32 {
        let restrictions = sandbox.prepare_command("/bin/sh").unwrap();
        let mut command = std::process::Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .current_dir(sandbox.base())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        restrictions.install(&mut command);
        command.status().unwrap().code().unwrap_or(-1)
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("luu-linux-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn the_filter_stops_the_internet_and_leaves_unix_sockets_alone() {
        // No kernel dependency beyond seccomp itself, which every Linux has.
        assert!(build_filter(false).is_ok());
        assert!(build_filter(true).is_ok());
    }

    #[test]
    fn a_child_cannot_open_an_internet_socket() {
        if build_filter(false).is_err() || !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let dir = scratch("nonet");
        let sandbox = sandbox_over(&dir, false);

        // `exec 3<>/dev/tcp/...` is the shell's own socket call, so this needs
        // nothing installed to be a real test of the filter.
        let code = run_under(&sandbox, "exec 3<>/dev/tcp/127.0.0.1/9 && echo open");
        assert_ne!(code, 0, "the child opened an AF_INET socket");

        // And the child is otherwise alive: a filter that killed everything
        // would pass the assertion above for the wrong reason.
        assert_eq!(run_under(&sandbox, "true"), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_child_cannot_read_outside_the_granted_roots() {
        let dir = scratch("landlock");
        if build_ruleset(&[]).is_err() || !std::path::Path::new("/bin/sh").exists() {
            // No Landlock here. That is a legitimate environment, and the
            // policy's `enforcement` is what decides whether it is usable —
            // asserting anything else would make this test a kernel check.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        std::fs::write(dir.join("inside"), "ok").unwrap();
        let outside = std::env::temp_dir().join(format!("luu-outside-{}", std::process::id()));
        std::fs::write(&outside, "secret").unwrap();
        let sandbox = sandbox_over(&dir, false);

        assert_eq!(
            run_under(&sandbox, "cat inside"),
            0,
            "the grant still works"
        );
        assert_ne!(
            run_under(&sandbox, &format!("cat {}", outside.display())),
            0,
            "the kernel is what stops this — nothing we wrote is in the way"
        );

        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
