//! The `kubernetes` runtime, driven end to end with no cluster: a `kubectl`
//! that reads the pod `luu` wrote into `--overrides` and runs its container's
//! command here, in its working directory, with its environment — the real
//! `luu worker` behind the real wait-for-a-line. What it checks is what a
//! cluster would not let a test see: that the pod luu writes is one a
//! container can start from, that the worker waits for the line and greets
//! after it, and that a call through it answers the bytes a host call does.
//!
//! What it cannot check is the cluster: the attach, the scheduler, the claim.
//! See `RECORD/2026-11-03.a-pod-per-session.WIP.md`, *Still open*.

use std::path::{Path, PathBuf};

use agent_core::sandbox::{Access, PathRule, Sandbox, SandboxPolicy};
use agent_core::tools::{ToolCall, Tools};
use agent_core::worker::kubernetes::Kube;
use agent_core::worker::{Executor, PROTOCOL, Runtime, Worker, WorkerSpec};

/// A fake `kubectl run`: refuses anything that is not attached, removed and
/// never restarted, then becomes the container — `luu` in its args swapped for
/// this build's binary. Python, because the pod is JSON.
const FAKE: &str = r#"#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
if not args or args[0] != "run":
    sys.stderr.write("fake kubectl: only `run`\n"); sys.exit(2)
for flag in ("--rm", "-i", "--restart=Never", "--override-type=merge"):
    if flag not in args:
        sys.stderr.write("fake kubectl: missing " + flag + "\n"); sys.exit(2)
pod = json.loads(next(a for a in args if a.startswith("--overrides="))[len("--overrides="):])
container = pod["spec"]["containers"][0]
if not (container["stdin"] and container["stdinOnce"]):
    sys.stderr.write("fake kubectl: the container does not take stdin\n"); sys.exit(2)
work = container["volumeMounts"][0]
if work["mountPath"] != container["workingDir"]:
    sys.stderr.write("fake kubectl: the work mount is not the working directory\n"); sys.exit(2)
env = dict(os.environ)
for pair in container.get("env", []):
    env[pair["name"]] = pair["value"]
argv = [LUU if word == "luu" else word for word in container["args"]]
os.chdir(container["workingDir"])
os.execvpe(container["command"][0], container["command"] + argv, env)
"#;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("luu-kube-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("work/u1/project/src")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("work/u1/project/src/main.rs"), "fn main() {}\n").unwrap();
        let kubectl = root.join("bin/kubectl");
        // Written by `sh`, for `worker_seam`'s reason: a file this process
        // holds open for writing is one a forked child may still hold when
        // it is exec'd, and the kernel refuses that (ETXTBSY).
        let script = FAKE.replace(
            "LUU",
            &format!("{:?}", env!("CARGO_BIN_EXE_luu")).replace('"', "'"),
        );
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("cat > \"$1\" && chmod 0755 \"$1\"")
            .arg("sh")
            .arg(&kubectl)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child
                    .stdin
                    .take()
                    .expect("stdin")
                    .write_all(script.as_bytes())?;
                child.wait()
            })
            .expect("writing the fake kubectl");
        assert!(status.success());
        Self {
            root: root.canonicalize().unwrap_or(root),
        }
    }

    fn base(&self) -> PathBuf {
        self.root.join("work/u1/project")
    }

    fn spec(&self) -> WorkerSpec {
        let root = self.root.clone();
        let kube = Kube::from_lookup(|name| match name {
            "LUU_KUBE_CLAIM" => Some("luu-work".into()),
            "LUU_KUBE_CLAIM_ROOT" => Some(root.join("work").display().to_string()),
            "LUU_KUBECTL" => Some(root.join("bin/kubectl").display().to_string()),
            _ => None,
        })
        .expect("the pod settings");
        WorkerSpec::new(Runtime::Kubernetes, self.base())
            .with_image(Some("ghcr.io/jgermade/luu-worker:test".into()))
            .with_home(Some(self.root.join("home")))
            .with_kube(Some(kube))
    }

    fn sandbox(&self) -> Sandbox {
        let policy = SandboxPolicy {
            paths: vec![PathRule::new(self.base(), Access::ReadWrite)],
            ..SandboxPolicy::default()
        };
        Sandbox::new(&policy, &self.base()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn python() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        name: name.to_string(),
        arguments,
    }
}

#[tokio::test]
async fn a_pod_waits_for_its_line_then_greets_and_answers_like_the_host() {
    if !python() {
        eprintln!("skipped: the fake kubectl needs python3");
        return;
    }
    let fixture = Fixture::new("seam");
    let sandbox = fixture.sandbox();
    let worker = Worker::start(&fixture.spec(), &["ls".into()])
        .await
        .expect("the pod's worker started and greeted after its line");
    assert_eq!(worker.hello().protocol, PROTOCOL);
    assert_eq!(worker.hello().version, env!("CARGO_PKG_VERSION"));
    let described = worker.describe().expect("a worker says where it is");
    assert!(described.contains("kubernetes"), "{described}");

    let tools = Tools::standard();
    for call in [
        call("read_file", serde_json::json!({"path": "src/main.rs"})),
        call("list_dir", serde_json::json!({"path": "."})),
        call("read_file", serde_json::json!({"path": "/etc/passwd"})),
    ] {
        let here = Executor::call(&tools, &call, &sandbox).await;
        let there = worker.call(&call, &sandbox).await;
        assert_eq!(there.verdict, here.verdict, "{}", call.name);
        assert_eq!(there.output, here.output, "{}", call.name);
        assert_eq!(there.error, here.error, "{}", call.name);
    }
}

#[tokio::test]
async fn a_base_outside_the_claim_is_refused_before_anything_starts() {
    let fixture = Fixture::new("outside");
    let spec = fixture.spec();
    let outside = WorkerSpec {
        base: Path::new("/somewhere/else").to_path_buf(),
        ..spec
    };
    let Err(error) = Worker::start(&outside, &[]).await else {
        panic!("a base outside the claim started a worker");
    };
    assert!(error.to_string().contains("not under"), "{error}");
}
