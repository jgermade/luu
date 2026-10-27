//! A runtime that is down is started when a session needs it, and the worker
//! asked again — driven through `luu tools`, with a stand-in `colima` first on
//! `PATH`. It refuses everything until `colima start` has run, and after that
//! it runs the real `luu worker` where a container would have.
//!
//! No VM is involved, so this says nothing about how long a real one takes to
//! answer. It says the order is right: the first start fails, the runtime is
//! started by its own line, and the second start is the one that answers. See
//! `RECORD/2026-10-27.runtimes-without-the-wait.completed.md`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// `colima start` marks the VM up; `colima nerdctl -- …` refuses until it is,
/// the way the real one does (`colima is not running`), then answers `info`
/// and runs what follows the image in a `run` as the worker.
const COLIMA: &str = r#"#!/bin/sh
here="$(dirname "$0")"
case "$1" in
  start) echo "starting the stand-in VM"; [ -n "$REFUSE_START" ] && exit 3; touch "$here/up"; exit 0 ;;
  nerdctl) shift; [ "$1" = "--" ] && shift ;;
  *) echo "the stand-in does not know $1" >&2; exit 2 ;;
esac
[ -f "$here/up" ] || { echo 'level=fatal msg="colima is not running"' >&2; exit 1; }
case "$1" in
  info) exit 0 ;;
  run)
    # Everything up to the image is the container's; after it comes the
    # worker's own line, `luu worker --command …`.
    while [ "$#" -gt 0 ] && [ "$1" != "luu-worker:test" ]; do shift; done
    shift 2  # the image, and the word `luu`
    exec "$LUU_BIN" "$@" ;;
esac
echo "the stand-in does not know nerdctl $1" >&2
exit 2
"#;

struct Machine {
    root: PathBuf,
}

impl Machine {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("luu-wake-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["bin", "project", "home"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let colima = root.join("bin/colima");
        std::fs::write(&colima, COLIMA).unwrap();
        std::fs::set_permissions(&colima, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            root.join("project/box.toml"),
            "[worker]\nruntime = \"colima\"\nimage = \"luu-worker:test\"\n",
        )
        .unwrap();
        let root = root.canonicalize().unwrap_or(root);
        Self { root }
    }

    fn path(&self, dir: &str) -> PathBuf {
        self.root.join(dir)
    }

    fn tools(&self, env: &[(&str, &str)]) -> Output {
        let path = format!(
            "{}:{}",
            self.path("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_luu"));
        command
            .args(["tools", "--sandbox", "box.toml"])
            .current_dir(self.path("project"))
            .env("PATH", path)
            .env("LUU_HOME", self.path("home"))
            .env("LUU_BIN", env!("CARGO_BIN_EXE_luu"));
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().expect("luu runs")
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn up(root: &Path) -> bool {
    root.join("bin/up").exists()
}

#[test]
fn a_runtime_that_is_down_is_started_and_the_worker_asked_again() {
    let machine = Machine::new("down");
    let out = machine.tools(&[]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(up(&machine.root), "colima start never ran: {stderr}");
    assert!(
        stderr.contains("colima: it does not answer, so it is being started"),
        "{stderr}"
    );
    assert!(
        stderr.contains("colima: $ colima start --runtime containerd"),
        "{stderr}"
    );
    assert!(stderr.contains("starting the stand-in VM"), "{stderr}");
    assert!(
        stdout.contains("worker     colima (luu-worker:test)"),
        "{stdout}"
    );
}

#[test]
fn a_runtime_that_answers_is_not_started() {
    let machine = Machine::new("answering");
    std::fs::write(machine.path("bin/up"), "").unwrap();
    let out = machine.tools(&[]);
    let stderr = text(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("being started"), "{stderr}");
}

#[test]
fn a_start_that_fails_is_said_beside_what_the_worker_said() {
    let machine = Machine::new("refused");
    let out = machine.tools(&[("REFUSE_START", "1")]);
    let stderr = text(&out.stderr);
    assert!(!out.status.success(), "it should not have run: {stderr}");
    assert!(stderr.contains("colima is not running"), "{stderr}");
    assert!(stderr.contains("starting colima did not help"), "{stderr}");
    assert!(!up(&machine.root));
}
