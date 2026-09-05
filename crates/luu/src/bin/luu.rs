//! The binary: a runtime, `luu::run`, and one thing `#[tokio::main]` cannot do.
//!
//! The runtime is built by hand rather than by the attribute so that shutdown
//! can be **bounded**. A tool call that was abandoned mid-syscall
//! (`RECORD/2026-09-05.a-clock-where-there-is-no-seam.completed.md`) leaves a
//! blocking thread parked in the kernel until the kernel answers, and dropping
//! a runtime waits for every one of them — so a wedged `read_file` that the
//! turn survived would come back as a process that will not exit. It is the
//! same hang, moved to the end of the program, which is a worse place for it:
//! by then there is nothing left to report it.
fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(luu::run());
    // Long enough for anything that is merely finishing, short enough that a
    // thread waiting on a filesystem that is never going to answer does not
    // hold the exit. What is left is abandoned with the process.
    runtime.shutdown_timeout(std::time::Duration::from_millis(200));
    result
}
