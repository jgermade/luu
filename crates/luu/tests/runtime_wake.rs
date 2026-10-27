//! What a session does about a contained worker that will not do as it is —
//! driven through `luu tools`, with a stand-in `colima` first on `PATH`.
//!
//! **A runtime that is down** is started and the worker asked again. The
//! stand-in refuses everything until `colima start` has run, and then runs the
//! real `luu worker` where a container would have. No VM is involved, so this
//! says nothing about how long a real one takes to answer; it says the order
//! is right. See `RECORD/2026-10-27.runtimes-without-the-wait.completed.md`.
//!
//! **An image that is not this luu's worker** is rebuilt from the release when
//! luu built it from one and its version is what moved, and said otherwise.
//! The stand-in plays the old worker by answering a hello written in a file,
//! carries the image's labels in another, and a `build` removes the first and
//! labels the image `release`. See
//! `RECORD/2026-10-27.an-image-that-knows-its-luu.completed.md`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// `colima start` marks the VM up; `colima nerdctl -- …` refuses until it is,
/// the way the real one does (`colima is not running`), then answers `info`,
/// `image inspect` (with `labels` as the image's JSON), `build` and `run` — the
/// last as the old worker while `hello` exists, and as the real one after.
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
  image)
    if [ -f "$here/labels" ]; then cat "$here/labels"; else echo '[{}]'; fi
    exit 0 ;;
  build)
    echo "$*" > "$here/built"
    [ -n "$REFUSE_BUILD" ] && { echo "the stand-in build failed" >&2; exit 4; }
    rm -f "$here/hello"
    echo '[{"Config":{"Labels":{"luu.from":"release"}}}]' > "$here/labels"
    echo "the stand-in built it"; exit 0 ;;
  run)
    # An image from another luu: its hello, and then nothing until its input
    # closes, which is all an old worker does before a call.
    if [ -f "$here/hello" ]; then cat "$here/hello"; cat >/dev/null; exit 0; fi
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

    /// The VM up and the image there, holding a worker that answers `hello`
    /// and labelled `from` (or not at all).
    fn image(&self, hello: &str, from: Option<&str>) {
        std::fs::write(self.path("bin/up"), "").unwrap();
        std::fs::write(self.path("bin/hello"), format!("{hello}\n")).unwrap();
        if let Some(from) = from {
            let labels = format!(r#"[{{"Config":{{"Labels":{{"luu.from":"{from}"}}}}}}]"#);
            std::fs::write(self.path("bin/labels"), labels).unwrap();
        }
    }

    fn built(&self) -> Option<String> {
        std::fs::read_to_string(self.path("bin/built")).ok()
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

/// A hello from a worker of another luu: `version`, and `build` where it says
/// one.
fn hello(protocol: u32, version: &str, build: Option<&str>) -> String {
    let build = build
        .map(|build| format!(r#","build":"{build}""#))
        .unwrap_or_default();
    format!(
        r#"{{"kind":"hello","protocol":{protocol},"version":"{version}"{build},"commands":[]}}"#
    )
}

const OURS: &str = env!("CARGO_PKG_VERSION");

#[test]
fn a_release_image_of_another_version_is_rebuilt_from_this_one() {
    let machine = Machine::new("release-old");
    machine.image(
        &hello(2, "0.0.1", Some("00000000aaaaaaaa")),
        Some("release"),
    );
    let out = machine.tools(&[]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(
        stderr.contains("its worker is luu 0.0.1 (00000000)"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("rebuilding it from luu {OURS}'s release")),
        "{stderr}"
    );
    let built = machine.built().expect("a build ran");
    assert!(
        built.contains(&format!("--build-arg LUU_VERSION={OURS}"))
            && built.contains("--label luu.from=release")
            && built.contains("-f Containerfile.release"),
        "{built}"
    );
    // The worker it ended on is this luu's, so nothing is stale.
    assert!(stdout.contains(&format!("luu {OURS} (")), "{stdout}");
    assert!(!stdout.contains("stale"), "{stdout}");
}

#[test]
fn a_release_image_refused_for_its_protocol_is_rebuilt_too() {
    let machine = Machine::new("release-protocol");
    machine.image(&hello(1, "0.0.1", None), Some("release"));
    let out = machine.tools(&[]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stderr.contains("speaks protocol 1"), "{stderr}");
    assert!(machine.built().is_some(), "{stderr}");
    assert!(!stdout.contains("stale"), "{stdout}");
}

#[test]
fn an_image_luu_did_not_build_from_a_release_is_said_and_kept() {
    for from in [Some("source"), None] {
        let machine = Machine::new(&format!("kept-{}", from.unwrap_or("unlabelled")));
        machine.image(&hello(2, "0.0.1", Some("00000000aaaaaaaa")), from);
        let out = machine.tools(&[]);
        let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
        assert!(out.status.success(), "{stdout}\n{stderr}");
        assert!(machine.built().is_none(), "{from:?} was rebuilt: {stderr}");
        assert!(stderr.contains("rebuild it in Settings"), "{stderr}");
        assert!(
            stdout.contains("stale      its worker is luu 0.0.1 (00000000)"),
            "{stdout}"
        );
    }
}

#[test]
fn a_release_of_this_same_version_from_other_sources_is_not_rebuilt() {
    let machine = Machine::new("same-version");
    machine.image(&hello(2, OURS, Some("00000000aaaaaaaa")), Some("release"));
    let out = machine.tools(&[]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(machine.built().is_none(), "{stderr}");
    assert!(stderr.contains("built from other sources"), "{stderr}");
    assert!(
        stderr.contains("only a build from source can help"),
        "{stderr}"
    );
}

#[test]
fn a_rebuild_that_fails_keeps_the_worker_the_session_has() {
    let machine = Machine::new("rebuild-refused");
    machine.image(
        &hello(2, "0.0.1", Some("00000000aaaaaaaa")),
        Some("release"),
    );
    let out = machine.tools(&[("REFUSE_BUILD", "1")]);
    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(machine.built().is_some());
    assert!(
        stderr.contains("was not rebuilt, and the session keeps the worker it has"),
        "{stderr}"
    );
    assert!(stdout.contains("stale"), "{stdout}");
}
